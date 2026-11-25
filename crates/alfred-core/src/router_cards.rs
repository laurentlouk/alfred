//! The app cards' everyday phrasings: music, YouTube videos, Claude Code, texts through Messages, and
//! finding emails. Like the rest of the router, a phrasing it isn't sure of goes to the model.

use super::*;
use crate::action::MusicCommand;
use crate::tools::is_just_music;

pub(super) fn route(t: &str, raw: &str, catalog: &Catalog) -> Option<Route> {
    claude(t, catalog).or_else(|| video(t)).or_else(|| music(t)).or_else(|| text_message(t, raw)).or_else(|| mail_search(t))
}

/// `t` is one of `phrases`, ignoring case, accents and punctuation.
fn is_one_of(t: &str, phrases: &[&str]) -> bool {
    let f = fold(t);
    phrases.iter().any(|p| fold(p) == f)
}

/// The words of `t`, folded.
fn folded_words(t: &str) -> Vec<String> {
    fold(t).split(' ').filter(|w| !w.is_empty()).map(str::to_string).collect()
}

// ---- music ----------------------------------------------------------------------------------------

const PAUSE: &[&str] = &[
    "pause",
    "pause the music",
    "pause music",
    "pause the song",
    "stop the music",
    "stop music",
    "stop playing",
    "stop playing music",
    "stop the song",
    "mets en pause",
    "pause la musique",
    "arrête la musique",
    "coupe la musique",
];

const RESUME: &[&str] = &[
    "play",
    "resume",
    "resume the music",
    "resume music",
    "resume playing",
    "unpause",
    "continue the music",
    "play music",
    "play some music",
    "play the music",
    "play my music",
    "start the music",
    "start playing music",
    "put on some music",
    "put some music on",
    "mets de la musique",
    "remets la musique",
    "joue de la musique",
    "lance la musique",
    "reprends la musique",
];

const NEXT: &[&str] = &[
    "next song",
    "next track",
    "skip",
    "skip this song",
    "skip the song",
    "skip this track",
    "skip the track",
    "skip song",
    "skip track",
    "play the next song",
    "play the next track",
    "skip to the next one",
    "skip to the next song",
    "skip to the next track",
    "skip ahead",
    "chanson suivante",
    "morceau suivant",
    "passe à la suivante",
];

const PREVIOUS: &[&str] = &[
    "previous song",
    "previous track",
    "go back a song",
    "go back a track",
    "play the previous song",
    "play the previous track",
    "chanson précédente",
    "morceau précédent",
];

const NOW_PLAYING: &[&str] = &[
    "what's playing",
    "what is playing",
    "what song is this",
    "what's this song",
    "what is this song",
    "which song is this",
    "what am I listening to",
    "who sings this",
    "who is singing",
    "who is this singer",
    "who's this singer",
    "who is this artist",
    "who's this artist",
    "who's singing",
    "what's this track",
    "quelle est cette chanson",
    "c'est quoi cette chanson",
];

const PLAYLISTS: &[&str] = &[
    "what playlists do I have",
    "which playlists do I have",
    "list my playlists",
    "show my playlists",
    "read my playlists",
    "my playlists",
    "what are my playlists",
    "tell me my playlists",
    "quelles sont mes playlists",
    "mes playlists",
];

/// Words that make "play …" about something other than music.
const NOT_MUSIC: &[&str] =
    &["video", "videos", "youtube", "episode", "podcast", "game", "film", "movie", "minuteur", "timer", "alarm", "alarme", "reveil"];

/// Music apps Alfred doesn't drive: "play lofi on Spotify" goes to the model.
const OTHER_PLAYERS: &[&str] = &["spotify", "deezer", "soundcloud", "tidal", "youtube music"];

fn music(t: &str) -> Option<Route> {
    let command = |c| Some(Route::Act(Action::Music { command: c, what: None }));
    for (phrases, c) in [
        (PAUSE, MusicCommand::Pause),
        (RESUME, MusicCommand::Play),
        (NEXT, MusicCommand::Next),
        (PREVIOUS, MusicCommand::Previous),
        (NOW_PLAYING, MusicCommand::NowPlaying),
        (PLAYLISTS, MusicCommand::Playlists),
    ] {
        if is_one_of(t, phrases) {
            return command(c);
        }
    }
    // "mets" and "lance" take any object ("mets un minuteur"): only with a music word.
    let (rest, needs_music_word) = match head(t, &["play", "put on", "listen to", "joue", "écoute", "ecoute"]) {
        Some(r) => (r, false),
        None => (head(t, &["mets", "lance"])?, true),
    };
    let words = folded_words(rest);
    if needs_music_word && !words.iter().any(|w| ["musique", "playlist", "chanson", "album", "morceau"].contains(&w.as_str())) {
        return None;
    }
    if words.iter().any(|w| NOT_MUSIC.contains(&w.as_str())) || points_at_a_page(rest) || is_placeholder(rest) && !rest.is_empty() {
        return None;
    }
    let padded = format!(" {} ", words.join(" "));
    if OTHER_PLAYERS.iter().any(|p| padded.contains(&format!(" on {p} ")) || padded.contains(&format!(" sur {p} "))) {
        return None;
    }
    let mut what = rest.trim();
    for s in [" on apple music", " in apple music", " on music", " in music", " from my library", " in my library", " sur apple music"] {
        what = strip_suffix_ci(what, s).unwrap_or(what);
    }
    const ARTICLES: &[&str] = &["some", "the", "my", "a", "an", "ma", "mon", "mes", "la", "le", "les", "du", "des"];
    const KINDS: &[&str] = &[
        "songs by",
        "music by",
        "something by",
        "anything by",
        "tracks by",
        "the playlist called",
        "the playlist named",
        "playlist called",
        "playlist named",
        "the playlist",
        "playlist",
        "the album",
        "album",
        "the song",
        "song",
        "the artist",
        "artist",
        "l'album",
        "la chanson",
    ];
    what = strip_words(strip_words(strip_words(what, ARTICLES), KINDS), ARTICLES);
    for s in [" playlist", " album", " songs", " music"] {
        what = strip_suffix_ci(what, s).unwrap_or(what);
    }
    let what = what.trim().trim_matches(['"', '“', '”']).trim();
    let what = (!what.is_empty() && !is_just_music(what)).then(|| what.to_string());
    Some(Route::Act(Action::Music { command: MusicCommand::Play, what }))
}

// ---- YouTube --------------------------------------------------------------------------------------

const FIND: &[&str] = &[
    "find me",
    "find",
    "search for",
    "search",
    "look up",
    "look for",
    "show me",
    "play",
    "watch",
    "cherche",
    "trouve moi",
    "trouve",
    "regarde",
];

const VIDEO_HEADS: &[&str] = &[
    "find me a video",
    "find me videos",
    "find a video",
    "find videos",
    "show me a video",
    "show me videos",
    "look for a video",
    "look for videos",
    "search for a video",
    "search for videos",
    "search youtube for",
    "search on youtube for",
    "search youtube",
    "youtube search for",
    "youtube search",
    "youtube",
    "play a video",
    "watch a video",
    "trouve moi une vidéo",
    "trouve une vidéo",
    "cherche une vidéo",
    "montre moi une vidéo",
];

/// What comes before the topic: "an explanation of", "a video about", "explaining"…
const TOPIC_LEAD: &[&str] = &[
    "an explanation of",
    "an explanation about",
    "explanations of",
    "explanations about",
    "explanation of",
    "explanation about",
    "a video about",
    "a video on",
    "a video explaining",
    "a video of",
    "videos about",
    "videos on",
    "videos explaining",
    "a video",
    "videos",
    "something about",
    "explaining",
    "that explains",
    "which explains",
    "that shows",
    "showing",
    "about",
    "on",
    "of",
    "for",
    "sur",
    "qui explique",
    "expliquant",
    "à propos de",
    "a propos de",
];

fn video(t: &str) -> Option<Route> {
    // "on YouTube, find …", "open YouTube and find …"
    if let Some(rest) = head(t, &["on youtube", "sur youtube", "open youtube and", "go to youtube and", "ouvre youtube et"]) {
        return video_route(head(rest, FIND).unwrap_or(rest));
    }
    // "find explanations about … on YouTube", "play lofi beats on YouTube"
    if let Some(before) = [" on youtube", " in youtube", " from youtube", " sur youtube"].iter().find_map(|s| strip_suffix_ci(t, s)) {
        return video_route(head(before, FIND)?);
    }
    video_route(head(t, VIDEO_HEADS)?)
}

fn video_route(q: &str) -> Option<Route> {
    let q = strip_words(q, TOPIC_LEAD).trim().trim_matches(['"', '“', '”']).trim();
    if q.is_empty() || is_placeholder(q) {
        return None;
    }
    Some(Route::Act(Action::FindVideo { query: q.to_string() }))
}

// ---- Claude and Claude Code -----------------------------------------------------------------------

const CODE_HEADS: &[&str] = &[
    "open claude code and ask it to",
    "open claude code and tell it to",
    "open claude code and",
    "open a new claude code session and ask it to",
    "open a new claude code session to",
    "open a new claude code session and",
    "open a new claude code session",
    "start a new claude code session to",
    "start a new claude code session and",
    "start a new claude code session",
    "start a claude code session to",
    "start a claude code session and",
    "start a claude code session",
    "open a claude code session to",
    "open a claude code session",
    "new claude code session",
    "ask claude code to",
    "ask claude code",
    "tell claude code to",
    "tell claude code",
    "get claude code to",
    "have claude code",
    "prompt claude code with",
    "prompt claude code to",
    "prompt claude code",
    "in claude code",
    "open claude code",
    "start claude code",
    "launch claude code",
    "claude code",
    "demande à claude code de",
    "demande a claude code de",
    "ouvre claude code et",
    "ouvre claude code",
];

const CHAT_HEADS: &[&str] = &[
    "ask claude to",
    "ask claude about",
    "ask claude",
    "prompt claude with",
    "prompt claude to",
    "prompt claude",
    "tell claude to",
    "tell claude",
    "demande à claude de",
    "demande a claude de",
    "demande à claude",
    "demande a claude",
];

/// Words that make a request to "Claude" coding work, for Claude Code.
const CODING: &[&str] = &[
    "code",
    "bug",
    "bugs",
    "test",
    "tests",
    "repo",
    "repository",
    "commit",
    "commits",
    "refactor",
    "function",
    "compile",
    "build",
    "branch",
    "crate",
    "module",
    "debug",
    "lint",
];

/// The last words naming a folder: "in the alfred project", "in my alfred repo".
const FOLDER_WORDS: &[&str] = &["project", "folder", "repo", "repository", "directory", "projet", "dossier"];

fn claude(t: &str, catalog: &Catalog) -> Option<Route> {
    if !catalog.claude {
        return None;
    }
    let lead = |rest: &str| strip_words(rest, &["ask it to", "tell it to", "to", "and", "with", "de"]).to_string();
    if let Some(rest) = head(t, CODE_HEADS) {
        return Some(code_route(&lead(rest)));
    }
    let rest = lead(head(t, CHAT_HEADS)?);
    if rest.trim().is_empty() {
        return None;
    }
    let (_, folder) = split_folder(&rest);
    if folder.is_some() || folded_words(&rest).iter().any(|w| CODING.contains(&w.as_str())) {
        return Some(code_route(&rest));
    }
    Some(Route::Act(Action::SendMessage { app: "Claude".into(), to: None, text: capitalize(rest.trim()) }))
}

fn code_route(rest: &str) -> Route {
    let (prompt, folder) = split_folder(rest);
    let prompt = prompt.trim();
    let prompt = if is_placeholder(prompt) { String::new() } else { capitalize(prompt) };
    Route::Act(Action::ClaudeCode { prompt, folder })
}

/// "fix the tests in the alfred project" → ("fix the tests", Some("alfred")).
fn split_folder(s: &str) -> (String, Option<String>) {
    let words: Vec<&str> = s.split_whitespace().collect();
    let Some(last) = words.last() else { return (String::new(), None) };
    if words.len() < 3 || !FOLDER_WORDS.contains(&fold(last).as_str()) {
        return (s.to_string(), None);
    }
    // Back from the folder word to "in", "on", "for" or "dans", at most four words before it.
    let end = words.len() - 1;
    for at in (end.saturating_sub(5)..end).rev() {
        if ["in", "on", "for", "dans"].contains(&fold(words[at]).as_str()) {
            let mut name = &words[at + 1..end];
            if name.first().is_some_and(|w| ["the", "my", "our", "le", "la", "mon", "notre"].contains(&fold(w).as_str())) {
                name = &name[1..];
            }
            if name.is_empty() {
                return (s.to_string(), None);
            }
            return (words[..at].join(" "), Some(name.join(" ")));
        }
    }
    (s.to_string(), None)
}

// ---- texts through Messages -----------------------------------------------------------------------

const TEXT_HEADS: &[&str] = &[
    "send a text message to",
    "send a text to",
    "send an sms to",
    "send a sms to",
    "send sms to",
    "send an imessage to",
    "send a imessage to",
    "text message",
    "imessage",
    "text",
    "sms",
    "envoie un sms à",
    "envoie un sms a",
    "envoie un texto à",
    "envoie un texto a",
];

/// Where the person ends and the message begins; the earliest one wins.
const TEXT_SEPARATORS: &[&str] = &[
    " pour lui dire que ",
    " pour lui dire ",
    " pour dire que ",
    " en disant ",
    " that ",
    " saying ",
    ": ",
    " to say ",
    ", ",
    " telling them ",
    " telling her ",
    " telling him ",
    " que ",
];

/// Apps a text can't go through: "text Ada on WhatsApp saying hi" is the model's to decide.
const OTHER_CHATS: &[&str] = &["whatsapp", "telegram", "signal", "slack", "discord", "teams", "messenger", "instagram"];

fn text_message(t: &str, raw: &str) -> Option<Route> {
    let rest = head(t, TEXT_HEADS)?;
    let (who, message) = split_text(rest)?;
    let who = strip_words(who.trim(), &["to"]).trim();
    let who_words = folded_words(who);
    let digits = who.chars().filter(char::is_ascii_digit).count();
    if who.is_empty()
        || (who_words.len() > 4 && digits < 6)
        || who_words.iter().any(|w| OTHER_CHATS.contains(&w.as_str()))
        || ["me", "myself", "moi"].contains(&fold(who).as_str())
    {
        return None;
    }
    // The message as said, with its punctuation: the same split on the request before its end was cleaned.
    let said = clean_start(raw);
    let message = head(&said, TEXT_HEADS).and_then(split_text).map(|(_, m)| m).unwrap_or(message);
    let mut message = message.trim();
    for s in [" on messages", " on imessage", " by sms", " via sms", " by text"] {
        message = strip_suffix_ci(message, s).unwrap_or(message);
    }
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    let mut who = who;
    for s in [" on messages", " on imessage", " by sms", " via sms"] {
        who = strip_suffix_ci(who, s).unwrap_or(who);
    }
    Some(Route::Act(Action::SendMessage { app: "Messages".into(), to: Some(who.trim().to_string()), text: capitalize(message) }))
}

/// "Ada saying see you" → ("Ada", "see you"), at the earliest separator.
fn split_text(s: &str) -> Option<(&str, &str)> {
    let lower = s.to_lowercase();
    let (at, sep) = TEXT_SEPARATORS.iter().filter_map(|sep| lower.find(sep).map(|at| (at, *sep))).min_by_key(|(at, _)| *at)?;
    // Lowercasing keeps byte offsets for the ASCII separators and for the letters the requests use.
    let (who, message) = (s.get(..at)?, s.get(at + sep.len()..)?);
    Some((who, message))
}

// ---- finding emails -------------------------------------------------------------------------------

const MAIL_HEADS: &[&str] = &[
    "find the emails",
    "find the email",
    "find the mails",
    "find the mail",
    "find my emails",
    "find emails",
    "find email",
    "find mails",
    "search my emails for",
    "search my email for",
    "search my mail for",
    "search my mails for",
    "search my inbox for",
    "search the mail for",
    "search mail for",
    "search email for",
    "search emails for",
    "look for the emails",
    "look for the email",
    "look for emails",
    "look for an email",
    "do i have any emails",
    "do i have any email",
    "do i have an email",
    "do i have emails",
    "did i get any emails",
    "did i get any email",
    "did i get an email",
    "is there an email",
    "are there any emails",
    "any emails",
    "any email",
    "cherche les mails",
    "cherche le mail",
    "cherche les e-mails",
    "cherche un mail",
    "trouve les mails",
    "trouve le mail",
    "est-ce que j'ai un mail",
    "est ce que j'ai un mail",
    "j'ai un mail",
    "j'ai des mails",
];

const FROM_MARKS: &[&str] = &[" from ", " de ", " d'"];
const ABOUT_MARKS: &[&str] =
    &[" about ", " regarding ", " concerning ", " on ", " with ", " for ", " sur ", " concernant ", " à propos de ", " au sujet de "];

fn mail_search(t: &str) -> Option<Route> {
    let rest = head(t, MAIL_HEADS)?.trim();
    if rest.is_empty() {
        return None;
    }
    // Pieces between the markers, each for the marker before it; what comes first is the subject.
    let padded = format!(" {rest} ");
    let lower = padded.to_lowercase();
    let mut marks: Vec<(usize, usize, bool)> = Vec::new();
    for (list, from) in [(FROM_MARKS, true), (ABOUT_MARKS, false)] {
        for m in list {
            marks.extend(lower.match_indices(m).map(|(at, _)| (at, m.len(), from)));
        }
    }
    marks.sort();
    let (mut query, mut sender) = (None::<String>, None::<String>);
    let mut put = |piece: &str, from: bool| {
        let piece = strip_words(piece.trim(), &["the", "a", "an", "my", "le", "la", "les", "un", "une", "des"]).trim();
        if piece.is_empty() {
            return;
        }
        let slot = if from { &mut sender } else { &mut query };
        slot.get_or_insert_with(|| piece.to_string());
    };
    let first = marks.first().map_or(padded.len(), |m| m.0);
    put(padded.get(..first)?, false);
    for (i, &(at, len, from)) in marks.iter().enumerate() {
        let end = marks.get(i + 1).map_or(padded.len(), |m| m.0);
        if at + len <= end {
            put(padded.get(at + len..end)?, from);
        }
    }
    (query.is_some() || sender.is_some()).then_some(Route::Act(Action::SearchMail { query, from: sender }))
}

#[cfg(test)]
#[path = "router_cards_tests.rs"]
mod tests;
