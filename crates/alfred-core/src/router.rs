//! The deterministic fast path: common requests become actions with no model at all.
//!
//! A 0.6B model picks the right tool but often mangles its arguments, so everyday phrasings ("open Notes",
//! "save a note: buy milk", "search Google for Rune", "remind me in 10 minutes to stretch") are parsed
//! here, exactly and instantly. Anything the router doesn't recognize goes to the LLM. The router errs on
//! the side of `None`: a wrong action is worse than a slower right one.

use crate::action::Action;
use crate::normalize::{app_name, browser_name, capitalize, site_url, strip_prefix_ci, strip_suffix_ci, title_from, url};

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
    SearchQuery { browser: Option<String> },
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
        })
    }
}

/// Route a request, or `None` to hand it to the LLM.
pub fn route(text: &str) -> Option<Route> {
    let t = clean(text);
    if t.is_empty() {
        return None;
    }
    if is_time_question(&t.to_lowercase()) {
        return Some(Route::Act(Action::GetTime));
    }
    note(&t).or_else(|| reminder(&t)).or_else(|| search(&t)).or_else(|| quit(&t)).or_else(|| open(&t))
}

/// Drop wake words, politeness and trailing punctuation: "Hey Alfred, could you open Notes for me?" →
/// "open Notes".
pub fn clean(text: &str) -> String {
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
        s = s.trim().trim_end_matches(['.', '!', '?', ',', ' ']).trim_start_matches([',', ' ', '-']).to_string();
        for p in LEADING {
            if let Some(rest) = strip_prefix_ci(&s, p)
                && (rest.is_empty() || rest.starts_with([' ', ',']))
            {
                s = rest.to_string();
                break;
            }
        }
        for p in TRAILING {
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
    let (q, browser) = trailing_browser(rest);
    Some(search_route(q, browser))
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
    if rest.is_empty() || rest.contains(" and ") || rest.split_whitespace().count() > 4 {
        return None;
    }
    let (target, browser) = trailing_browser_for_link(rest);
    if let Some(u) = url(target).or_else(|| site_url(target)) {
        return Some(Route::Act(Action::OpenUrl { url: u, browser }));
    }
    if link_only || strip_prefix_ci(t, "start").is_some() && (target.starts_with("a ") || target.starts_with("an ")) {
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

#[cfg(test)]
#[path = "router_tests.rs"]
mod tests;
