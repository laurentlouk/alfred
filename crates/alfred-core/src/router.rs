//! The deterministic fast path: common requests become actions with no model at all.
//!
//! A 0.6B model picks the right tool but often mangles its arguments, so everyday phrasings ("open Notes",
//! "save a note: buy milk", "search Google for Rune", "remind me in 10 minutes to stretch") are parsed
//! here, exactly and instantly. Anything the router doesn't recognize goes to the LLM. The router errs on
//! the side of `None`: a wrong action is worse than a slower right one.

use crate::action::Action;
use crate::catalog::{Catalog, fold};
use crate::normalize::{app_name, browser_name, capitalize, is_known_app, site_url, strip_prefix_ci, strip_suffix_ci, title_from, url};

/// What the router decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// Do this.
    Act(Action),
    /// The request is clear but its content is missing: optionally do `first`, ask `question`, and treat
    /// the user's next words as the `slot`.
    Ask { question: String, slot: Slot, first: Option<Action> },
}

/// A piece of a request Alfred is waiting for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot {
    NoteBody,
    ReminderTitle,
    SearchQuery {
        browser: Option<String>,
    },
    /// What to say in a reply to the newest email (from `from`).
    ReplyText {
        from: Option<String>,
    },
}

impl Slot {
    /// The action the user's answer completes, or `None` if the answer is empty or a "never mind".
    pub fn fill(&self, answer: &str) -> Option<Action> {
        let a = clean(answer);
        let lower = a.to_lowercase();
        if a.is_empty() || ["never mind", "nevermind", "cancel", "nothing", "forget it", "no"].contains(&lower.as_str()) {
            return None;
        }
        Some(match self {
            Slot::NoteBody => note_action(None, &a),
            Slot::ReminderTitle => reminder_action(&a)?,
            Slot::SearchQuery { browser } => Action::WebSearch { query: a, browser: browser.clone() },
            // The answer is the reply itself: kept as said, with its punctuation.
            Slot::ReplyText { from } => Action::DraftReply { from: from.clone(), text: capitalize(answer.trim()) },
        })
    }
}

/// Route a request, or `None` to hand it to the LLM.
pub fn route(text: &str) -> Option<Route> {
    route_with(text, &Catalog::default())
}

/// Route a request knowing what this Mac offers (the user's shortcuts).
pub fn route_with(text: &str, catalog: &Catalog) -> Option<Route> {
    let t = clean(text);
    if t.is_empty() {
        return None;
    }
    if is_time_question(&t.to_lowercase()) {
        return Some(Route::Act(Action::GetTime));
    }
    shortcut(&t, catalog)
        .or_else(|| mail(&t, text))
        .or_else(|| note(&t))
        .or_else(|| reminder(&t))
        .or_else(|| cards::route(&t, text, catalog))
        .or_else(|| search(&t))
        .or_else(|| quit(&t))
        .or_else(|| open(&t))
}

// ---- shortcuts ------------------------------------------------------------------------------------

/// A request that names one of the user's shortcuts: "run Bonjour", "run the Réveil shortcut", "lance
/// démarre la musique", or a name of two words or more said on its own ("Focus Timer"). A one-word name
/// needs the verb, so "bonjour" said as a greeting never runs a shortcut called "Bonjour".
fn shortcut(t: &str, catalog: &Catalog) -> Option<Route> {
    if catalog.shortcuts.is_empty() {
        return None;
    }
    const HEADS: &[&str] = &[
        "run the shortcut",
        "run my shortcut",
        "run shortcut",
        "start the shortcut",
        "start shortcut",
        "lance le raccourci",
        "lancer le raccourci",
        "execute le raccourci",
        "exécute le raccourci",
        "shortcut",
        "raccourci",
        "run",
        "lance",
        "lancer",
        "exécute",
        "execute",
        "start",
    ];
    if let Some(rest) = head(t, HEADS) {
        let rest = strip_suffix_ci(rest, " shortcut").or_else(|| strip_suffix_ci(rest, " raccourci")).unwrap_or(rest);
        let rest = strip_words(rest.trim().trim_matches(['"', '“', '”', '«', '»']).trim(), &["the", "my", "le", "la", "mon", "ma"]);
        if let Some(name) = catalog.find_shortcut(rest) {
            return Some(Route::Act(Action::RunShortcut { name: name.to_string(), input: None }));
        }
    }
    let name = catalog.exact_shortcut(t)?;
    (name.split_whitespace().count() >= 2).then(|| Route::Act(Action::RunShortcut { name: name.to_string(), input: None }))
}

/// Drop wake words, politeness and trailing punctuation: "Hey Alfred, could you open Notes for me?" →
/// "open Notes".
pub fn clean(text: &str) -> String {
    clean_with(text, true)
}

/// Only the start of [`clean`]: the end of a request can be text to keep as said ("…saying thanks!").
fn clean_start(text: &str) -> String {
    clean_with(text, false)
}

fn clean_with(text: &str, end: bool) -> String {
    const LEADING: &[&str] = &[
        "hey alfred",
        "hi alfred",
        "ok alfred",
        "okay alfred",
        "alfred",
        "please",
        "kindly",
        "can you",
        "could you",
        "would you",
        "will you",
        "can u",
        "i want you to",
        "i'd like you to",
        "i would like you to",
        "i need you to",
        "go ahead and",
        "now",
        "just",
    ];
    const TRAILING: &[&str] = &["please", "for me", "thanks", "thank you", "now", "right now"];
    let mut s = text.trim().to_string();
    loop {
        let before = s.clone();
        s = s.trim().trim_start_matches([',', ' ', '-']).to_string();
        if end {
            s = s.trim_end_matches(['.', '!', '?', ',', ' ']).to_string();
        }
        for p in LEADING {
            if let Some(rest) = strip_prefix_ci(&s, p)
                && (rest.is_empty() || rest.starts_with([' ', ',']))
            {
                s = rest.to_string();
                break;
            }
        }
        for p in TRAILING.iter().filter(|_| end) {
            if let Some(rest) = strip_suffix_ci(&s, p)
                && (rest.is_empty() || rest.ends_with([' ', ',']))
            {
                s = rest.to_string();
                break;
            }
        }
        if s == before {
            return s.trim().to_string();
        }
    }
}

fn is_time_question(lower: &str) -> bool {
    const EXACT: &[&str] = &["time", "the time", "date", "the date", "current time", "today's date"];
    const ASKS: &[&str] = &[
        "what time is it",
        "what's the time",
        "whats the time",
        "what is the time",
        "tell me the time",
        "what's the date",
        "what is the date",
        "what's today's date",
        "what is today's date",
        "what day is it",
        "what day is today",
        "what's the date today",
    ];
    // "what time is it in Tokyo" is a question about elsewhere: leave it to the model.
    EXACT.contains(&lower) || ASKS.iter().any(|a| lower == *a || lower.starts_with(&format!("{a} ")) && !lower.contains(" in "))
}

/// The head word of `t` when it is followed by a word boundary; returns the rest.
fn head<'a>(t: &'a str, heads: &[&str]) -> Option<&'a str> {
    heads.iter().find_map(|h| {
        let rest = strip_prefix_ci(t, h)?;
        (rest.is_empty() || rest.starts_with([' ', ':', ',', '-'])).then(|| rest.trim_start_matches([' ', ':', ',', '-']))
    })
}

fn strip_words<'a>(mut s: &'a str, words: &[&str]) -> &'a str {
    loop {
        let before = s;
        for w in words {
            if let Some(rest) = strip_prefix_ci(s, w)
                && (rest.is_empty() || rest.starts_with(' '))
            {
                s = rest.trim_start();
            }
        }
        if s == before {
            return s;
        }
    }
}

const PLACEHOLDERS: &[&str] = &["something", "something down", "it", "this", "that", "a note", "stuff", "anything"];

fn is_placeholder(s: &str) -> bool {
    let l = s.trim().to_lowercase();
    l.is_empty() || PLACEHOLDERS.contains(&l.as_str())
}

// ---- mail -----------------------------------------------------------------------------------------

/// "read my last email", "who sent my latest email", "lis mon dernier mail", "reply to my last email
/// saying I'll be there at 3", "réponds au dernier mail de Marie : merci". Only the newest email (from
/// someone, when named) is reachable this way; "the email about the invoice" goes to the model.
///
/// `t` is the cleaned request; the reply's text is taken from `raw` as said, with its punctuation and a
/// closing "thanks" that [`clean`] would drop.
fn mail(t: &str, raw: &str) -> Option<Route> {
    let f = fold(t);
    const WHO_LAST: &[&str] = &[
        "who emailed me last",
        "who emailed me most recently",
        "who wrote to me last",
        "qui m a ecrit en dernier",
        "qui m a envoye un mail en dernier",
    ];
    // "anything new in my inbox?": the newest email answers it. qwen3:0.6b chatted instead (and claimed
    // the inbox was empty), so these are the router's.
    const INBOX: &[&str] = &[
        "anything new in my inbox",
        "what s new in my inbox",
        "what s in my inbox",
        "check my inbox",
        "check my email",
        "check my emails",
        "check my mail",
        "any new email",
        "any new emails",
        "any new mail",
        "do i have new email",
        "do i have new emails",
        "do i have any new email",
        "do i have any new emails",
        "do i have new mail",
        "do i have any new mail",
        "did i get any new email",
        "did i get any email",
        "have i got new mail",
        "j ai des nouveaux mails",
        "est ce que j ai des nouveaux mails",
        "des nouveaux mails",
        "quoi de neuf dans ma boite mail",
    ];
    if WHO_LAST.contains(&f.as_str()) || INBOX.contains(&f.as_str()) {
        return Some(Route::Act(Action::ReadMail { from: None }));
    }
    let said = drop_draft_words(&clean_start(raw));
    let t = said.as_str();
    const REPLY_HEADS: &[&str] = &[
        "draft a reply to",
        "write a reply to",
        "draft a response to",
        "write a response to",
        "prepare a reply to",
        "write back to",
        "reply back to",
        "reply to",
        "respond to",
        "answer",
        "rédige une réponse à",
        "écris une réponse à",
        "réponds à",
        "réponds au",
        "répondre à",
        "répondre au",
        "reponds a",
        "reponds au",
    ];
    if let Some(rest) = head(t, REPLY_HEADS) {
        let (object, text) = split_reply_text(rest);
        let from = latest_mail(&clean(object))?;
        return Some(reply_route(from, text, None));
    }
    const READ_HEADS: &[&str] = &[
        "read me",
        "read out",
        "read",
        "what's in",
        "what is in",
        "what's",
        "whats",
        "what is",
        "what does",
        "what did",
        "who sent me",
        "who sent",
        "who wrote",
        "check",
        "show me",
        "tell me about",
        "lis-moi",
        "lis moi",
        "lis",
        "quel est",
        "c'est quoi",
        "qui a envoyé",
        "qui a écrit",
    ];
    let rest = head(t, READ_HEADS)?;
    // "read my last email and reply saying …"
    let (object, then) = match find_ci(rest, &[" and ", " et ", ", then ", " then "]) {
        Some((at, sep)) => (&rest[..at], Some(rest[at + sep.len()..].trim())),
        None => (rest, None),
    };
    let from = latest_mail(&clean(object))?;
    match then {
        None => Some(Route::Act(Action::ReadMail { from })),
        Some(then) => {
            let rest = head(then, &["draft a reply", "write a reply", "reply", "respond", "answer", "réponds", "répondre"])?;
            let rest = strip_words(rest, &["to it", "to that", "to them", "to him", "to her", "lui", "y"]);
            let text =
                head(rest, &["saying", "and say", "and tell them", "to say", "telling them", "with", "en disant", "pour dire", "et dis"])
                    .unwrap_or(rest);
            let text = Some(text).filter(|s| !s.trim().is_empty());
            Some(reply_route(from.clone(), text, Some(Action::ReadMail { from })))
        }
    }
}

/// "… as a draft", "… and don't send it": Alfred only ever drafts, so these words add nothing.
fn drop_draft_words(t: &str) -> String {
    const WORDS: &[&str] = &[
        ", and leave it as a draft",
        " and leave it as a draft",
        " and leave it in drafts",
        ", but don't send it",
        " but don't send it",
        " and don't send it",
        " without sending it",
        " sans l'envoyer",
        " en tant que brouillon",
        " en brouillon",
        " as a draft",
        " in a draft",
        " in draft",
        " as draft",
    ];
    let mut s = t.to_string();
    for w in WORDS {
        while let Some((at, _)) = find_ci(&s, &[*w]) {
            s.replace_range(at..at + w.len(), "");
        }
    }
    s.trim().to_string()
}

/// The first of `needles` found in `hay` (ASCII case-insensitive): its byte position and the needle.
fn find_ci<'n>(hay: &str, needles: &[&'n str]) -> Option<(usize, &'n str)> {
    (0..hay.len())
        .filter(|&i| hay.is_char_boundary(i))
        .find_map(|i| needles.iter().find(|n| strip_prefix_ci(&hay[i..], n).is_some()).map(|n| (i, *n)))
}

/// Split "my last email saying I'll be late" into the email ("my last email") and the reply's text.
fn split_reply_text(s: &str) -> (&str, Option<&str>) {
    const SAYS: &[&str] = &[
        " saying ",
        " and say ",
        " and tell them ",
        " and tell him ",
        " and tell her ",
        " to say ",
        " telling them ",
        " with ",
        " en disant ",
        " pour dire ",
        " et dis-lui ",
        " et dis ",
        ": ",
        " : ",
    ];
    match find_ci(s, SAYS) {
        Some((at, sep)) => (s[..at].trim(), Some(s[at + sep.len()..].trim())),
        None => (s.trim(), None),
    }
}

/// Whether `object` means the newest email: "my last email", "the latest e-mail from Ada", "mon dernier
/// mail de Marie". `Some(from)` when it does (with the sender, if named); `None` for anything else, such as
/// "the email about the invoice" or "my last note".
fn latest_mail(object: &str) -> Option<Option<String>> {
    let (what, from) = match find_ci(object, &[" from ", " de ", " d'", " d’"]) {
        Some((at, sep)) => (&object[..at], Some(object[at + sep.len()..].trim())),
        None => (object, None),
    };
    const NOUNS: &[&str] = &["email", "emails", "mail", "mails", "courriel", "courriels"];
    const RECENT: &[&str] = &["last", "latest", "newest", "recent", "dernier", "derniere"];
    const FILLER: &[&str] = &[
        "my", "the", "to", "me", "most", "e", "mon", "ma", "mes", "le", "la", "les", "au", "a", "l", "plus", "say", "says", "about", "in",
        "inbox",
    ];
    let words: Vec<String> = fold(what).split(' ').map(String::from).collect();
    let has = |set: &[&str]| words.iter().any(|w| set.contains(&w.as_str()));
    let only = words.iter().all(|w| NOUNS.contains(&w.as_str()) || RECENT.contains(&w.as_str()) || FILLER.contains(&w.as_str()));
    if !(has(NOUNS) && has(RECENT) && only) {
        return None;
    }
    let from = from
        .map(|f| strip_words(f, &["the", "le", "la"]).trim().trim_matches(['"', '“', '”']).trim().to_string())
        .filter(|f| !f.is_empty());
    Some(from)
}

/// A reply with its text, or a question for it (after `first`, such as reading the email aloud).
fn reply_route(from: Option<String>, text: Option<&str>, first: Option<Action>) -> Route {
    let text = text.map(|s| s.trim().trim_matches(['"', '“', '”', '«', '»']).trim()).filter(|s| !is_placeholder(s));
    match text {
        Some(s) => Route::Act(Action::DraftReply { from, text: capitalize(s) }),
        None => Route::Ask { question: "What shall I say in the reply?".into(), slot: Slot::ReplyText { from }, first },
    }
}

// ---- notes ----------------------------------------------------------------------------------------

fn note(t: &str) -> Option<Route> {
    // "open Notes and save …": the context is Notes, so plain verbs count as note verbs.
    if let Some(rest) = head(t, &["open notes and", "open the notes app and", "open up notes and", "go to notes and", "launch notes and"]) {
        let body = head(rest, &["save", "write", "add", "type", "put", "note", "jot down", "write down"]).unwrap_or(rest);
        return Some(note_route(strip_notes_suffix(body), true));
    }
    const HEADS: &[&str] = &[
        "create a new note",
        "write a new note",
        "take a note",
        "make a note",
        "create a note",
        "add a note",
        "save a note",
        "write a note",
        "start a note",
        "new note",
        "take note",
        "make note",
        "note down",
        "jot down",
        "write down",
        "remember that",
        "note",
    ];
    if let Some(rest) = head(t, HEADS) {
        return Some(note_route(strip_notes_suffix(rest), false));
    }
    // "save buy milk in my notes", "add eggs to notes"
    if let Some(rest) = head(t, &["save", "add", "put", "write", "store", "keep"]) {
        let body = strip_notes_suffix(rest);
        if body.len() < rest.len() {
            return Some(note_route(body, false));
        }
    }
    None
}

fn strip_notes_suffix(s: &str) -> &str {
    for suffix in
        [" in my notes", " to my notes", " in apple notes", " to apple notes", " in notes", " to notes", " as a note", " in a note"]
    {
        if let Some(rest) = strip_suffix_ci(s, suffix) {
            return rest.trim();
        }
    }
    s.trim()
}

fn note_route(rest: &str, open_first: bool) -> Route {
    let rest = strip_words(rest, &["that", "saying", "which says", "to say", "about", "of", "with", "down", "in notes"]);
    // "called Groceries with milk and eggs" / "titled Ideas: a launcher"
    if let Some(named) = head(rest, &["called", "titled", "named"]) {
        let (title, body) = split_title(named);
        if !is_placeholder(title) {
            return Route::Act(note_action(Some(title), body.unwrap_or(title)));
        }
    }
    if is_placeholder(rest) {
        return Route::Ask {
            question: "What would you like me to write down?".into(),
            slot: Slot::NoteBody,
            first: open_first.then(|| Action::OpenApp { name: "Notes".into() }),
        };
    }
    Route::Act(note_action(None, rest))
}

fn split_title(s: &str) -> (&str, Option<&str>) {
    for sep in [" with ", " saying ", ": ", " that says "] {
        if let Some((a, b)) = s.split_once(sep)
            && !b.trim().is_empty()
        {
            return (a.trim(), Some(b.trim()));
        }
    }
    (s.trim(), None)
}

fn note_action(title: Option<&str>, body: &str) -> Action {
    let body = capitalize(body.trim());
    let title = title.map(|t| capitalize(t.trim())).unwrap_or_else(|| title_from(&body, 6));
    Action::CreateNote { title, body }
}

// ---- reminders ------------------------------------------------------------------------------------

fn reminder(t: &str) -> Option<Route> {
    const HEADS: &[&str] = &[
        "remind me",
        "set a reminder",
        "add a reminder",
        "create a reminder",
        "make a reminder",
        "new reminder",
        "set reminder",
        "reminder",
        "remember to",
    ];
    let rest = head(t, HEADS)?;
    let remember_to = strip_prefix_ci(t, "remember to").is_some();
    let rest = if remember_to { rest } else { strip_words(rest, &["to", "about", "that", "for", "of"]) };
    match reminder_action(rest) {
        Some(a) => Some(Route::Act(a)),
        None => Some(Route::Ask { question: "What shall I remind you about?".into(), slot: Slot::ReminderTitle, first: None }),
    }
}

fn reminder_action(rest: &str) -> Option<Action> {
    let (title, minutes) = extract_delay(rest);
    let title = strip_words(title.trim().trim_matches([',', '.']), &["to", "about", "that"]).trim();
    if is_placeholder(title) {
        return None;
    }
    Some(Action::CreateReminder { title: capitalize(title), minutes })
}

/// Pull "in 10 minutes" / "in an hour" / "in half an hour" out of the text.
fn extract_delay(s: &str) -> (String, Option<u32>) {
    let words: Vec<&str> = s.split_whitespace().collect();
    for i in 0..words.len() {
        if !words[i].eq_ignore_ascii_case("in") {
            continue;
        }
        // "in half an hour" is 0.5 of the unit that follows.
        let (n, used) = match words.get(i + 1).map(|w| w.to_lowercase()) {
            Some(w) if w == "half" && words.get(i + 2).is_some_and(|x| x.eq_ignore_ascii_case("an")) => (0.5, 3),
            Some(w) => match number(&w) {
                Some(n) => (n, 2),
                None => continue,
            },
            None => continue,
        };
        let unit_at = i + used;
        let mult = match words.get(unit_at).map(|u| u.trim_matches([',', '.']).to_lowercase()) {
            Some(u) if ["minute", "minutes", "min", "mins"].contains(&u.as_str()) => 1.0,
            Some(u) if ["hour", "hours", "hr", "hrs"].contains(&u.as_str()) => 60.0,
            _ => continue,
        };
        let minutes = (n * mult).round() as u32;
        let mut rest: Vec<&str> = words[..i].to_vec();
        rest.extend_from_slice(&words[unit_at + 1..]);
        return (rest.join(" "), (minutes > 0).then_some(minutes));
    }
    (s.to_string(), None)
}

fn number(w: &str) -> Option<f64> {
    const WORDS: &[&str] = &[
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
    ];
    match w {
        "a" | "an" => Some(1.0),
        "thirty" => Some(30.0),
        "forty" => Some(40.0),
        "forty-five" => Some(45.0),
        "sixty" => Some(60.0),
        _ => w.parse::<f64>().ok().filter(|n| *n >= 0.0).or_else(|| WORDS.iter().position(|x| *x == w).map(|p| p as f64)),
    }
}

// ---- web search -----------------------------------------------------------------------------------

fn search(t: &str) -> Option<Route> {
    // "open Safari and search for …", "open Google and look up …"
    for opener in ["open", "launch", "go to", "open up"] {
        let Some(rest) = head(t, &[opener]) else { continue };
        let Some((target, tail)) = rest.split_once(" and ") else { continue };
        let browser = if site_url(target).is_some_and(|u| u.contains("google.")) {
            None
        } else {
            match browser_name(target) {
                Some(b) => b,
                None => continue,
            }
        };
        let Some(q) = head(tail, &["search the web for", "search google for", "search for", "search", "look up", "google", "find"]) else {
            continue;
        };
        return Some(search_route(q, browser));
    }
    const HEADS: &[&str] = &[
        "do a google search for",
        "do a web search for",
        "search the internet for",
        "search the web for",
        "search online for",
        "search on google for",
        "search google for",
        "search on google",
        "search google",
        "search the web",
        "search for",
        "web search",
        "google search",
        "search",
        "google",
        "look up",
        "lookup",
    ];
    let rest = head(t, HEADS)?;
    if points_at_a_page(rest) {
        return None;
    }
    let (q, browser) = trailing_browser(rest);
    Some(search_route(q, browser))
}

/// Words that point at something on a page ("the second story", "the first link", "this site", "here").
/// The router doesn't know which page the user is on; the model does, so these are left to it.
pub(crate) fn points_at_a_page(words: &str) -> bool {
    const PLACES: &[&str] =
        &["this site", "this page", "this website", "this tab", "on it", "here", "ce site", "cette page", "cet onglet", "ici"];
    const ORDINALS: &[&str] = &[
        "first",
        "second",
        "third",
        "fourth",
        "fifth",
        "next",
        "previous",
        "last",
        "top",
        "latest",
        "premier",
        "premiere",
        "deuxieme",
        "seconde",
        "troisieme",
        "suivant",
        "suivante",
        "dernier",
        "derniere",
    ];
    const THINGS: &[&str] = &[
        "story",
        "stories",
        "link",
        "links",
        "result",
        "results",
        "video",
        "videos",
        "article",
        "articles",
        "post",
        "posts",
        "comment",
        "comments",
        "image",
        "images",
        "tab",
        "button",
        "item",
        "lien",
        "liens",
        "resultat",
        "resultats",
        "bouton",
    ];
    let padded = format!(" {} ", crate::catalog::fold(words));
    let has = |list: &[&str]| list.iter().any(|w| padded.contains(&format!(" {w} ")));
    has(PLACES) || (has(ORDINALS) && has(THINGS))
}

/// "rune ui in safari" → ("rune ui", Some("Safari")); "rune on google" → ("rune", None).
fn trailing_browser(q: &str) -> (&str, Option<String>) {
    for sep in [" in ", " on ", " using ", " with "] {
        if let Some((a, b)) = q.rsplit_once(sep) {
            if let Some(browser) = browser_name(b) {
                return (a, browser);
            }
            if ["google", "the web", "the internet", "web", "internet"].contains(&b.to_lowercase().as_str()) {
                return (a, None);
            }
        }
    }
    for s in [" online", " on the web"] {
        if let Some(a) = strip_suffix_ci(q, s) {
            return (a, None);
        }
    }
    (q, None)
}

fn search_route(q: &str, browser: Option<String>) -> Route {
    let q = q.trim().trim_matches(['"', '“', '”']).trim();
    if is_placeholder(q) {
        return Route::Ask { question: "What shall I search for?".into(), slot: Slot::SearchQuery { browser }, first: None };
    }
    Route::Act(Action::WebSearch { query: q.to_string(), browser })
}

// ---- apps and links -------------------------------------------------------------------------------

fn quit(t: &str) -> Option<Route> {
    let rest = head(t, &["quit", "close", "exit"])?;
    let lower = rest.to_lowercase();
    if ["window", "the window", "this window", "tab", "the tab", "this tab", "it", "this", "that", "everything", "all", "all apps"]
        .contains(&lower.as_str())
        || rest.split_whitespace().count() > 3
    {
        return None;
    }
    app_name(rest).map(|name| Route::Act(Action::QuitApp { name }))
}

fn open(t: &str) -> Option<Route> {
    const LINK_HEADS: &[&str] = &["go to", "visit", "navigate to", "browse to"];
    const APP_HEADS: &[&str] = &["open up", "open", "launch", "fire up", "bring up", "switch to", "start"];
    let (rest, link_only) = match head(t, LINK_HEADS) {
        Some(r) => (r, true),
        None => (head(t, APP_HEADS)?, false),
    };
    let rest = rest.trim();
    if rest.is_empty() || rest.contains(" and ") || rest.split_whitespace().count() > 4 || points_at_a_page(rest) {
        return None;
    }
    let (target, browser) = trailing_browser_for_link(rest);
    if let Some(u) = url(target).or_else(|| site_url(target)) {
        return Some(Route::Act(Action::OpenUrl { url: u, browser }));
    }
    if link_only {
        return None;
    }
    // "start" and "fire up" take any object ("start the vacuum", "start a timer"): only apps Alfred knows
    // by name count. "open", "launch" and the rest name an app whatever it is.
    let loose = ["start", "fire up"].iter().any(|h| strip_prefix_ci(t, h).is_some());
    if loose && !is_known_app(target) {
        return None;
    }
    app_name(target).map(|name| Route::Act(Action::OpenApp { name }))
}

fn trailing_browser_for_link(s: &str) -> (&str, Option<String>) {
    for sep in [" in ", " with ", " using "] {
        if let Some((a, b)) = s.rsplit_once(sep)
            && let Some(browser) = browser_name(b)
        {
            return (a, browser);
        }
    }
    (s, None)
}

#[path = "router_cards.rs"]
mod cards;

#[cfg(test)]
#[path = "router_tests.rs"]
mod tests;
