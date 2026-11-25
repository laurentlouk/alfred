//! The tools the LLM may call, their JSON schema, and turning a (possibly mangled) tool call back into an
//! [`Action`].
//!
//! Small models pick the right tool but bend its arguments: a string comes back as `{"body": "…"}`, the
//! JSON arrives as a string inside a string, a parameter is renamed (`content` for `body`, `app` for
//! `name`). [`action_from_call`] repairs what it safely can and refuses the rest with a message the model
//! can act on.

use serde_json::{Value, json};

use crate::action::{Action, MusicCommand, host_of};
use crate::catalog::Catalog;
use crate::normalize::{app_name, browser_name, capitalize, site_url, title_from, url};

/// A tool as the model sees it.
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub params: &'static [Param],
}

pub struct Param {
    pub name: &'static str,
    pub kind: &'static str,
    pub description: &'static str,
    pub required: bool,
}

const fn p(name: &'static str, kind: &'static str, description: &'static str, required: bool) -> Param {
    Param { name, kind, description, required }
}

/// Every tool Alfred exposes to the model.
pub const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "open_app",
        description: "Open a Mac application by name, for example Notes, Safari or Spotify.",
        params: &[p("name", "string", "The application's name, such as \"Notes\".", true)],
    },
    ToolSpec {
        name: "quit_app",
        description: "Quit a running Mac application. The user is asked to confirm first.",
        params: &[p("name", "string", "The application's name.", true)],
    },
    ToolSpec {
        name: "create_note",
        description: "Save a new note in Apple Notes and show it.",
        params: &[
            p("title", "string", "A short title of a few words.", false),
            p("body", "string", "The full text of the note, as plain text.", true),
        ],
    },
    ToolSpec {
        name: "create_reminder",
        description: "Add a reminder to Apple Reminders, optionally due after a number of minutes.",
        params: &[
            p("title", "string", "What to be reminded about.", true),
            p("minutes", "integer", "Minutes from now until it is due; omit for no due time.", false),
        ],
    },
    ToolSpec {
        name: "web_search",
        description: "Search the web and show the results, or answer a question that needs current or online information (weather, news, prices, scores, opening hours, who won, how far, the time in another city) from them.",
        params: &[p("query", "string", "What to search for: the question, in a few words.", true)],
    },
    ToolSpec {
        name: "open_url",
        description: "Open a web page in the browser.",
        params: &[p("url", "string", "The address, such as https://example.com.", true)],
    },
    ToolSpec { name: "get_time", description: "Tell the current local time and date.", params: &[] },
    ToolSpec {
        name: "read_mail",
        description: "Read the newest email in Mail's inbox: who sent it, when, its subject and how it begins.",
        params: &[p("from", "string", "Only if the user names the sender: part of their name or address.", false)],
    },
    ToolSpec {
        name: "reply_mail",
        description: "Open a reply to the newest email in Mail with the given text, as a draft the user checks and sends. It never sends.",
        params: &[
            p("text", "string", "What the reply says, in the user's words.", true),
            p("from", "string", "Only if the user names the sender of the email to answer.", false),
        ],
    },
    ToolSpec {
        name: "search_mail",
        description: "Find emails in Mail's inbox by what they are about or who sent them, and say the newest.",
        params: &[
            p("query", "string", "Words from the subject, such as \"invoice\".", false),
            p("from", "string", "Only if the user names the sender: part of their name or address.", false),
        ],
    },
    ToolSpec {
        name: "music",
        description: "Play, pause or skip music in Apple Music, play a playlist, song, artist or album, say what's playing, or list the user's playlists.",
        params: &[
            p("command", "string", "One of play, pause, next, previous, now_playing, playlists.", true),
            p("text", "string", "What to play, only if the user named it: a playlist, song, artist or album.", false),
        ],
    },
    ToolSpec {
        name: "find_video",
        description: "Find a new video on YouTube that explains or shows a topic, and play it. Not for a video on a page that is already open.",
        params: &[p("query", "string", "What the video should be about, in a few words.", true)],
    },
];

/// The `tools` array in the shape Ollama and OpenAI-compatible servers both accept.
pub fn tools_json() -> Value {
    tools_json_for(&Catalog::default())
}

/// The tools for this Mac: the built-in ones, plus `run_shortcut` (its names as an enum), `use_app` and
/// `use_browser` when the catalog offers them.
pub fn tools_json_for(catalog: &Catalog) -> Value {
    let mut tools = base_tools_json();
    if let Value::Array(list) = &mut tools {
        if !catalog.shortcuts.is_empty() {
            list.push(json!({
                "type": "function",
                "function": {
                    "name": "run_shortcut",
                    "description": "Run one of the user's Shortcuts by its exact name.",
                    "parameters": { "type": "object", "properties": {
                        "shortcut": { "type": "string", "enum": catalog.shortcuts, "description": "The shortcut's name." },
                        "text": { "type": "string", "description": "Input text, only if the user gave some." },
                    }, "required": ["shortcut"] },
                }
            }));
        }
        if catalog.hands {
            list.push(json!({
                "type": "function",
                "function": {
                    "name": "send_message",
                    "description": SEND_MESSAGE_DESCRIPTION,
                    "parameters": { "type": "object", "properties": {
                        "app": { "type": "string", "description": "The chat app, such as WhatsApp, Messages, Slack or Claude." },
                        "to": { "type": "string", "description": "The person or chat, only if the user named one; empty to reply to the latest message." },
                        "text": { "type": "string", "description": "The message itself, as it should be sent." },
                    }, "required": ["app", "text"] },
                }
            }));
            list.push(json!({
                "type": "function",
                "function": {
                    "name": "use_app",
                    "description": "Operate another app on screen to do something no other tool can, such as changing a setting in an app.",
                    "parameters": { "type": "object", "properties": {
                        "app": { "type": "string", "description": "The app's name, such as Mail." },
                        "text": { "type": "string", "description": "The task, in one sentence." },
                    }, "required": ["app", "text"] },
                }
            }));
        }
        if catalog.claude {
            list.push(json!({
                "type": "function",
                "function": {
                    "name": "claude_code",
                    "description": CLAUDE_CODE_DESCRIPTION,
                    "parameters": { "type": "object", "properties": {
                        "text": { "type": "string", "description": "The prompt, as the user wants it sent to Claude Code." },
                        "folder": { "type": "string", "description": "The project or folder, only if the user named one." },
                    }, "required": ["text"] },
                }
            }));
        }
        if catalog.browser {
            list.push(json!({
                "type": "function",
                "function": {
                    "name": "use_browser",
                    "description": BROWSER_DESCRIPTION,
                    "parameters": { "type": "object", "properties": {
                        "text": { "type": "string", "description": "The task, in one sentence." },
                        "url": { "type": "string", "description": "Only if the user gave a site's address: where to start." },
                        "tab": { "type": "string", "enum": ["alfred", "yours"], "description": "yours: the page the user is looking at (this page, here); alfred (the default): Alfred's own tab, which stays on the last site it opened." },
                    }, "required": ["text"] },
                }
            }));
        }
    }
    tools
}

const SEND_MESSAGE_DESCRIPTION: &str = "Write a message in a chat app (WhatsApp, Messages, Slack, Telegram, Signal, Discord, Teams) to someone, or reply to the latest message, or write a prompt in an assistant app such as Claude. Alfred types it and asks before sending.";

const CLAUDE_CODE_DESCRIPTION: &str = "Start a new Claude Code session with a prompt, for coding work in a project: fixing, building, reviewing or explaining code. The prompt waits for the user to send it.";

const BROWSER_DESCRIPTION: &str = "Do a task on websites that needs more than one step, such as finding something on a site, reading a page, going through several results or comparing prices. A plain search is web_search, and opening a site is open_url.";

fn base_tools_json() -> Value {
    Value::Array(
        TOOLS
            .iter()
            .map(|t| {
                let props: serde_json::Map<String, Value> =
                    t.params.iter().map(|p| (p.name.to_string(), json!({ "type": p.kind, "description": p.description }))).collect();
                let required: Vec<&str> = t.params.iter().filter(|p| p.required).map(|p| p.name).collect();
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": { "type": "object", "properties": props, "required": required },
                    }
                })
            })
            .collect(),
    )
}

/// The schema for structured decisions: one `action` from an enum of the tools plus "chat", and flat
/// fields. Decoding under this grammar is what makes a 0.6B model dependable: it cannot answer with
/// prose where a tool belongs, or invent a tool.
pub fn decision_schema() -> Value {
    decision_schema_for(&Catalog::default())
}

/// The decision schema for this Mac: `run_shortcut` (with a `shortcut` field limited to the real names),
/// `use_app` and `use_browser` join the actions when the catalog offers them.
pub fn decision_schema_for(catalog: &Catalog) -> Value {
    let mut actions: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
    if !catalog.shortcuts.is_empty() {
        actions.push("run_shortcut");
    }
    if catalog.hands {
        actions.push("send_message");
        actions.push("use_app");
    }
    if catalog.claude {
        actions.push("claude_code");
    }
    if catalog.browser {
        actions.push("use_browser");
    }
    actions.push("chat");
    let mut schema = json!({
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": actions },
            "app": { "type": "string", "maxLength": 60 },
            "text": { "type": "string", "maxLength": 1500 },
            "query": { "type": "string", "maxLength": 300 },
            "url": { "type": "string", "maxLength": 500 },
            "minutes": { "type": "integer" },
            "from": { "type": "string", "maxLength": 100 },
            "reply": { "type": "string", "maxLength": 600 },
            "hard": { "type": "boolean" },
            "tab": { "type": "string", "enum": ["alfred", "yours"] },
            "to": { "type": "string", "maxLength": 100 },
            "command": { "type": "string", "enum": ["play", "pause", "next", "previous", "now_playing", "playlists"] },
            "folder": { "type": "string", "maxLength": 100 },
        },
        "required": ["action"],
    });
    if !catalog.shortcuts.is_empty() {
        schema["properties"]["shortcut"] = json!({ "type": "string", "enum": catalog.shortcuts });
    }
    schema
}

/// The decision guide for this Mac: [`DECISION_GUIDE`] plus the lines for the optional actions.
pub fn decision_guide_for(catalog: &Catalog) -> String {
    let mut guide = DECISION_GUIDE.to_string();
    if !catalog.shortcuts.is_empty() {
        guide.push_str(&format!(
            "\nrun_shortcut: run one of the user's Shortcuts (field shortcut: its exact name; field text: input only if the user gave some). The shortcuts: {}.",
            catalog.shortcuts.join("; ")
        ));
    }
    if catalog.hands {
        guide.push_str("\nsend_message: write a message in a chat app (WhatsApp, Messages, Slack, Telegram, Signal, Discord, Teams), reply to the latest message there, or write a prompt in an assistant app such as Claude, even when the prompt asks for news or facts (field app: the app; field to: the person or chat only if the user named one; field text: the message itself). Alfred asks before sending.");
        guide.push_str("\nuse_app: do something inside another app on screen when no action above fits, such as changing a setting or reading what an app shows (field app: the app; field text: the task in one sentence).");
    }
    if catalog.claude {
        guide.push_str("\nclaude_code: start a Claude Code session with a prompt for coding work in a project, such as fixing, building, reviewing or explaining code (field text: the prompt as the user wants it sent; field folder: the project or folder only if the user named one).");
    }
    if catalog.browser {
        guide.push_str("\nuse_browser: a task on websites that needs more than one step, such as finding something on a site, reading a page or comparing prices, and any question or step about a page that is open (field text: the task in one sentence, with what the user wants from the page; field url only if the user gave the site's address; field tab: yours for the page the user is looking at, alfred for Alfred's own tab, which stays on the last site it opened). A plain search stays web_search, and opening a site stays open_url.");
    }
    guide
}

/// How to fill the decision, appended to the system prompt in structured mode.
pub const DECISION_GUIDE: &str = "Decide the single best action for the user's latest request and fill only its fields.
open_app: open an application (field app, such as Music, Calendar, Mail or Terminal).
quit_app: quit an application (field app).
create_note: save a note in Apple Notes (field text: the note's content).
create_reminder: add a reminder (field text: what to do; field minutes only if the user gave a delay).
web_search: search the web, or answer a question that needs current or online information, such as the weather, news, prices, scores, opening hours, who won, how far, or the time in another city (field query: the question in a few words).
open_url: open a website (field url).
read_mail: the user asks about their email or inbox (field from only if they name a sender).
reply_mail: answer the newest email (field text: what the reply says; field from only if they name a sender).
search_mail: find emails by what they are about or who sent them (field query: words from the subject; field from: the sender only if they name one).
music: play, pause or skip music in Apple Music, play a playlist, song, artist or album, say what's playing or who sings it, or list the playlists (field command: play, pause, next, previous, now_playing or playlists; field text: what to play, only if the user named it).
find_video: find a new video on YouTube that explains or shows a topic, and play it (field query: the topic in a few words). A video on a page that is open (“the first video”, “this video”) is not this: it is a step on that page.
get_time: the user asks the local time, the date or the day (the time in another city is web_search).
chat: anything else; answer in one or two short sentences in field reply, and set hard to true when a good answer needs reasoning (a calculation, logic, a plan, a comparison, or explaining how or why).";

/// A structured decision as a tool call, or `None` for "chat".
pub fn decision_to_call(decision: &Value) -> Option<(String, Value)> {
    let action = decision.get("action").and_then(Value::as_str).unwrap_or("chat").trim().to_string();
    if action == "chat" || action.is_empty() {
        return None;
    }
    let mut args = decision.clone();
    if let Some(m) = args.as_object_mut() {
        m.remove("action");
        m.remove("reply");
        m.remove("hard");
    }
    Some((action, args))
}

/// Arguments as an object, whatever shape they arrived in: an object, a JSON string, a JSON string
/// wrapped in a code fence or prose, or nothing.
pub fn parse_arguments(raw: &Value) -> Value {
    match raw {
        Value::Object(_) => raw.clone(),
        Value::String(s) => {
            let s = s.trim();
            if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(s) {
                return v;
            }
            // `{...}` somewhere inside the text
            if let (Some(a), Some(b)) = (s.find('{'), s.rfind('}'))
                && a < b
                && let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(&s[a..=b])
            {
                return v;
            }
            Value::Object(Default::default())
        }
        _ => Value::Object(Default::default()),
    }
}

/// A string argument by name or alias, unwrapping the shapes small models produce.
pub fn string_arg(args: &Value, names: &[&str]) -> Option<String> {
    names.iter().find_map(|n| args.get(*n).and_then(as_text)).filter(|s| !s.trim().is_empty())
}

fn as_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(_) | Value::Null => None,
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().filter_map(as_text).collect();
            (!parts.is_empty()).then(|| parts.join("\n"))
        }
        Value::Object(map) => ["text", "body", "value", "content", "string", "name", "title", "query"]
            .iter()
            .find_map(|k| map.get(*k).and_then(as_text))
            .or_else(|| map.values().find_map(|v| if let Value::String(s) = v { Some(s.clone()) } else { None })),
    }
}

/// An integer argument: a number, a numeric string, or "10 minutes".
pub fn int_arg(args: &Value, names: &[&str]) -> Option<i64> {
    names.iter().find_map(|n| match args.get(*n)? {
        Value::Number(x) => x.as_f64().map(|f| f.round() as i64),
        Value::String(s) => s.split_whitespace().next()?.parse::<f64>().ok().map(|f| f.round() as i64),
        _ => None,
    })
}

/// Turn a tool call into an action. `utterance` is what the user said, used as a last resort for
/// content-bearing fields (a note body the model forgot to fill in).
pub fn action_from_call(name: &str, raw_args: &Value, utterance: &str) -> Result<Action, String> {
    action_from_call_in(name, raw_args, utterance, &Catalog::default())
}

/// [`action_from_call`] knowing this Mac's shortcuts and whether apps can be operated on screen.
pub fn action_from_call_in(name: &str, raw_args: &Value, utterance: &str, catalog: &Catalog) -> Result<Action, String> {
    let args = parse_arguments(raw_args);
    let name = name.trim().to_lowercase().replace(['-', ' '], "_");
    match name.as_str() {
        "open_app" | "open_application" | "launch_app" => {
            let raw = string_arg(&args, &["name", "app", "app_name", "application", "application_name"])
                .ok_or("open_app needs the app's name in \"name\".")?;
            if let Some(u) = url(&raw).or_else(|| site_url(&raw)) {
                return Ok(Action::OpenUrl { url: u, browser: None });
            }
            app_name(&raw).map(|name| Action::OpenApp { name }).ok_or_else(|| format!("\"{raw}\" is not an app name."))
        }
        "quit_app" | "close_app" => {
            let raw = string_arg(&args, &["name", "app", "app_name", "application"]).ok_or("quit_app needs the app's name in \"name\".")?;
            app_name(&raw).map(|name| Action::QuitApp { name }).ok_or_else(|| format!("\"{raw}\" is not an app name."))
        }
        "create_note" | "save_note" | "add_note" | "new_note" | "note" => {
            let body = string_arg(&args, &["body", "content", "text", "note", "message", "description"])
                .or_else(|| string_arg(&args, &["title"]))
                .or_else(|| (!utterance.trim().is_empty()).then(|| utterance.trim().to_string()))
                .ok_or("create_note needs the note's text in \"body\".")?;
            let title =
                string_arg(&args, &["title", "name", "subject", "heading"]).map(|t| capitalize(&t)).unwrap_or_else(|| title_from(&body, 6));
            Ok(Action::CreateNote { title, body: capitalize(&body) })
        }
        "create_reminder" | "add_reminder" | "set_reminder" | "remind" | "reminder" => {
            let title = string_arg(&args, &["title", "text", "task", "reminder", "name", "body", "content"])
                .ok_or("create_reminder needs what to remind about in \"title\".")?;
            let minutes = int_arg(&args, &["minutes", "in_minutes", "delay", "delay_minutes", "due_in_minutes"])
                .filter(|m| (1..=60 * 24 * 365).contains(m))
                .map(|m| m as u32);
            Ok(Action::CreateReminder { title: capitalize(&title), minutes })
        }
        "web_search" | "search" | "google" | "search_web" | "google_search" => {
            let query = string_arg(&args, &["query", "q", "search", "text", "term", "keywords"]).ok_or("web_search needs \"query\".")?;
            let browser = string_arg(&args, &["browser"]).and_then(|b| browser_name(&b)).flatten();
            Ok(Action::WebSearch { query, browser })
        }
        "open_url" | "open_link" | "open_website" | "browse" => {
            let raw = string_arg(&args, &["url", "link", "website", "address", "site", "page"]).ok_or("open_url needs \"url\".")?;
            let u = url(&raw).or_else(|| site_url(&raw)).ok_or_else(|| format!("\"{raw}\" is not an http(s) address."))?;
            Ok(Action::OpenUrl { url: u, browser: None })
        }
        "get_time" | "time" | "get_date" | "current_time" => Ok(Action::GetTime),
        "read_mail" | "read_email" | "read_last_email" | "read_latest_email" | "check_mail" | "check_email" => {
            Ok(Action::ReadMail { from: string_arg(&args, &["from", "sender", "who"]) })
        }
        "reply_mail" | "reply_email" | "reply_to_email" | "draft_reply" | "answer_email" | "reply" => {
            // An empty text opens the reply for the user to write; it is never filled from the request itself,
            // which is an instruction to Alfred, not the reply.
            let text = string_arg(&args, &["text", "body", "message", "content"]).map(|t| capitalize(&t)).unwrap_or_default();
            Ok(Action::DraftReply { from: string_arg(&args, &["from", "sender", "to", "recipient"]), text })
        }
        "run_shortcut" | "shortcut" | "run_shortcuts" | "shortcuts" if !catalog.shortcuts.is_empty() => {
            let raw = string_arg(&args, &["shortcut", "name", "title"]).ok_or("run_shortcut needs the shortcut's name in \"shortcut\".")?;
            let name = catalog
                .find_shortcut(&raw)
                .ok_or_else(|| format!("There is no shortcut called \"{raw}\". The shortcuts are: {}.", catalog.shortcuts.join("; ")))?
                .to_string();
            let input = string_arg(&args, &["text", "input"]).filter(|t| crate::catalog::fold(t) != crate::catalog::fold(&name));
            Ok(Action::RunShortcut { name, input })
        }
        "send_message" | "message" | "send_chat" | "write_message" | "reply_message" | "send" if catalog.hands => {
            let raw = string_arg(&args, &["app", "application", "service"]).ok_or("send_message needs the app's name in \"app\".")?;
            let app = app_name(&raw).ok_or_else(|| format!("\"{raw}\" is not an app name."))?;
            let text = string_arg(&args, &["text", "message", "body", "content"]).ok_or("send_message needs the message in \"text\".")?;
            // A recipient only when the user named them: a model's guess would open someone else's chat.
            let said = crate::catalog::fold(utterance);
            let to = string_arg(&args, &["to", "recipient", "contact", "chat", "name"])
                .filter(|t| !t.trim().is_empty() && said.contains(&crate::catalog::fold(t)));
            Ok(Action::SendMessage { app, to, text })
        }
        "search_mail" | "find_mail" | "find_email" | "find_emails" | "search_email" | "search_emails" | "search_inbox" => {
            let query = string_arg(&args, &["query", "q", "subject", "about", "text", "keywords"]);
            let from = string_arg(&args, &["from", "sender", "who"]);
            if query.is_none() && from.is_none() {
                return Err("search_mail needs words from the subject in \"query\" or a sender in \"from\".".into());
            }
            Ok(Action::SearchMail { query, from })
        }
        "music" | "play_music" | "apple_music" | "play" | "pause_music" | "stop_music" | "next_track" | "previous_track"
        | "now_playing" | "playlists" | "list_playlists" | "play_playlist" | "play_song" => {
            let command = string_arg(&args, &["command", "action", "do", "control"])
                .and_then(|c| MusicCommand::from_word(&c))
                .or_else(|| MusicCommand::from_word(name.trim_end_matches("_music")))
                .unwrap_or(MusicCommand::Play);
            let what = string_arg(&args, &["text", "what", "query", "song", "playlist", "artist", "album", "name", "title"])
                .filter(|w| command == MusicCommand::Play && !is_just_music(w));
            Ok(Action::Music { command, what })
        }
        "find_video" | "youtube" | "video" | "search_youtube" | "youtube_search" | "play_video" | "find_youtube_video" => {
            // "Play the first video" is a step on the page that is open, not a new search.
            if catalog.browser && crate::router::points_at_a_page(utterance) {
                return Err("The user means a video on the page that is open: use use_browser on that page instead.".into());
            }
            let query = string_arg(&args, &["query", "q", "text", "topic", "search", "about"])
                .or_else(|| (!utterance.trim().is_empty()).then(|| utterance.trim().to_string()))
                .ok_or("find_video needs the topic in \"query\".")?;
            Ok(Action::FindVideo { query })
        }
        "claude_code" | "ask_claude_code" | "claude" | "start_claude_code" | "open_claude_code" if catalog.claude => {
            let prompt = string_arg(&args, &["text", "prompt", "message", "task", "query"])
                .or_else(|| (!utterance.trim().is_empty()).then(|| utterance.trim().to_string()))
                .ok_or("claude_code needs the prompt in \"text\".")?;
            // A folder only when the user named it: a model's guess would open the wrong project.
            let said = crate::catalog::fold(utterance);
            let folder = string_arg(&args, &["folder", "project", "repo", "repository", "directory", "dir"])
                .filter(|f| !crate::catalog::fold(f).is_empty() && said.contains(&crate::catalog::fold(f)));
            Ok(Action::ClaudeCode { prompt, folder })
        }
        "use_app" | "operate_app" | "use" if catalog.hands => {
            let raw = string_arg(&args, &["app", "name", "application"]).ok_or("use_app needs the app's name in \"app\".")?;
            let app = app_name(&raw).ok_or_else(|| format!("\"{raw}\" is not an app name."))?;
            let task = string_arg(&args, &["text", "task", "goal", "instruction"])
                .or_else(|| (!utterance.trim().is_empty()).then(|| utterance.trim().to_string()))
                .ok_or("use_app needs the task in \"text\".")?;
            Ok(Action::UseApp { app, task })
        }
        "use_browser" | "browser" | "use_web" | "web_task" | "browse_site" if catalog.browser => {
            let task = string_arg(&args, &["text", "task", "goal", "instruction", "query"])
                .or_else(|| (!utterance.trim().is_empty()).then(|| utterance.trim().to_string()))
                .ok_or("use_browser needs the task in \"text\".")?;
            // The page the user is looking at: their own tab, never with an address.
            let theirs = string_arg(&args, &["tab", "where", "on"]).is_some_and(|t| {
                matches!(crate::catalog::fold(&t).as_str(), "yours" | "your" | "user" | "users" | "theirs" | "their" | "user's")
            });
            if theirs {
                return Ok(Action::UseTheirTab { task });
            }
            // The address the model gave, when the user said it; else one the user said outright ("on
            // en.wikipedia.org, find…"), which models sometimes leave in the task.
            let url = string_arg(&args, &["url", "link", "website", "site", "address", "page"])
                .and_then(|raw| said_url(&raw, utterance))
                .or_else(|| utterance.split_whitespace().find_map(url));
            Ok(Action::UseBrowser { task, url })
        }
        other => Err(format!(
            "There is no tool called \"{other}\". The tools are: {}.",
            TOOLS.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// "music", "some music", "a song": what to play when nothing in particular was named.
pub fn is_just_music(what: &str) -> bool {
    const ANY: &[&str] = &[
        "music",
        "some music",
        "the music",
        "my music",
        "a song",
        "some songs",
        "songs",
        "a track",
        "something",
        "anything",
        "de la musique",
        "musique",
        "la musique",
    ];
    ANY.contains(&crate::catalog::fold(what).as_str())
}

/// A starting address for the browser, kept only when it comes from the user's words: an http(s) address
/// whose host they said ("on amazon.fr …"), or one on a site they named ("on Wikipedia …" keeps
/// `en.wikipedia.org`). A model guessing an address, or repeating one it read somewhere, gets nothing:
/// the run starts from the page already open instead.
fn said_url(raw: &str, said: &str) -> Option<String> {
    let u = url(raw).or_else(|| site_url(raw))?;
    let host = host_of(&u).to_lowercase();
    if says_host(&said.to_lowercase(), &host) {
        return Some(u);
    }
    // Sites said by name: every one- and two-word run of the request that names a known site.
    let words: Vec<String> = crate::catalog::fold(said).split(' ').map(str::to_string).collect();
    let named =
        (0..words.len()).flat_map(|i| [words[i].clone(), words[i..(i + 2).min(words.len())].join(" ")]).filter_map(|w| site_url(&w));
    for site in named {
        let site_host = host_of(&site).to_lowercase();
        if host == site_host || host.ends_with(&format!(".{site_host}")) {
            return Some(u);
        }
    }
    None
}

/// `said` contains `host` as a whole address: "x.com" is in "go to x.com/home" but not in "fix.com".
fn says_host(said: &str, host: &str) -> bool {
    let part = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '-');
    !host.is_empty()
        && said.match_indices(host).any(|(i, _)| !part(said[..i].chars().next_back()) && !part(said[i + host.len()..].chars().next()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_reaches_the_app_cards_but_can_never_send_a_text_itself() {
        let c = Catalog { claude: true, ..Catalog::default() };
        let call = |name: &str, args: Value, said: &str| action_from_call_in(name, &args, said, &c);
        assert_eq!(
            call("music", json!({"command": "play", "text": "my running playlist"}), "put my running playlist on").unwrap(),
            Action::Music { command: MusicCommand::Play, what: Some("my running playlist".into()) }
        );
        assert_eq!(call("pause_music", json!({}), "hush").unwrap(), Action::Music { command: MusicCommand::Pause, what: None });
        assert_eq!(
            call("music", json!({"command": "play", "text": "some music"}), "music please").unwrap(),
            Action::Music { command: MusicCommand::Play, what: None }
        );
        assert_eq!(
            call("music", json!({"command": "next", "text": "Daft Punk"}), "next").unwrap(),
            Action::Music { command: MusicCommand::Next, what: None },
            "only play takes something to play"
        );
        assert_eq!(
            call("search_mail", json!({"query": "invoice"}), "any invoices?").unwrap(),
            Action::SearchMail { query: Some("invoice".into()), from: None }
        );
        assert!(call("search_mail", json!({}), "emails").is_err());
        assert_eq!(call("find_video", json!({"query": "black holes"}), "x").unwrap(), Action::FindVideo { query: "black holes".into() });
        assert_eq!(
            call("claude_code", json!({"text": "Fix the tests", "folder": "alfred"}), "ask claude code to fix the tests in alfred")
                .unwrap(),
            Action::ClaudeCode { prompt: "Fix the tests".into(), folder: Some("alfred".into()) }
        );
        assert_eq!(
            call("claude_code", json!({"text": "Fix the tests", "folder": "secret-project"}), "ask claude code to fix the tests").unwrap(),
            Action::ClaudeCode { prompt: "Fix the tests".into(), folder: None },
            "a folder the user never named is dropped"
        );
        assert!(action_from_call("claude_code", &json!({"text": "x"}), "x").is_err(), "offered only with Claude installed");
        let web = Catalog { browser: true, ..c.clone() };
        let e = action_from_call_in("find_video", &json!({"query": "first video"}), "play the first video", &web).unwrap_err();
        assert!(e.contains("use_browser"), "a video on the open page is a step there: {e}");
        let text = json!({"name": "Ada", "label": "mobile", "handle": "+33612345678", "text": "hi"});
        assert!(call("send_text", text, "text Ada hi").is_err(), "only a found contact's confirmation sends");
    }

    #[test]
    fn questions_about_the_world_now_go_to_web_search() {
        for words in ["weather", "news", "prices", "scores", "opening hours", "who won", "how far", "the time in another city"] {
            assert!(DECISION_GUIDE.contains(words), "{words}");
        }
        let search = TOOLS.iter().find(|t| t.name == "web_search").unwrap();
        assert!(search.description.contains("weather") && search.description.contains("who won"));
        assert!(DECISION_GUIDE.contains("the time in another city is web_search"));
    }

    #[test]
    fn schema_lists_every_tool_with_required_fields() {
        let tools = tools_json();
        let arr = tools.as_array().unwrap();
        assert_eq!(arr.len(), TOOLS.len());
        let note = arr.iter().find(|t| t["function"]["name"] == "create_note").unwrap();
        assert_eq!(note["function"]["parameters"]["required"], json!(["body"]));
        assert_eq!(note["function"]["parameters"]["properties"]["title"]["type"], "string");
    }

    #[test]
    fn repairs_the_object_in_a_string_field_seen_from_qwen3_0_6b() {
        // The exact shape qwen3:0.6b produced on 2026-11-24.
        let args = json!({
            "title": "Save a note to buy milk tomorrow",
            "body": { "body": "I need to save a note that I need to buy milk tomorrow.", "type": "string" }
        });
        let a = action_from_call("create_note", &args, "save a note that I need to buy milk tomorrow").unwrap();
        assert_eq!(
            a,
            Action::CreateNote {
                title: "Save a note to buy milk tomorrow".into(),
                body: "I need to save a note that I need to buy milk tomorrow.".into()
            }
        );
    }

    #[test]
    fn parses_arguments_given_as_json_strings_and_prose() {
        assert_eq!(parse_arguments(&json!("{\"name\": \"Notes\"}")), json!({"name": "Notes"}));
        assert_eq!(parse_arguments(&json!("Sure! ```json\n{\"query\": \"rune\"}\n```")), json!({"query": "rune"}));
        assert_eq!(parse_arguments(&json!("not json")), json!({}));
        assert_eq!(parse_arguments(&Value::Null), json!({}));
    }

    #[test]
    fn accepts_renamed_parameters_and_tools() {
        assert_eq!(action_from_call("open_app", &json!({"app": "safari"}), "").unwrap(), Action::OpenApp { name: "Safari".into() });
        assert_eq!(
            action_from_call("search", &json!({"q": "kokoro"}), "").unwrap(),
            Action::WebSearch { query: "kokoro".into(), browser: None }
        );
        assert_eq!(
            action_from_call("set_reminder", &json!({"task": "stretch", "minutes": "10"}), "").unwrap(),
            Action::CreateReminder { title: "Stretch".into(), minutes: Some(10) }
        );
        assert_eq!(action_from_call("get_time", &json!({}), "").unwrap(), Action::GetTime);
    }

    #[test]
    fn a_site_passed_to_open_app_becomes_a_link() {
        assert_eq!(
            action_from_call("open_app", &json!({"name": "YouTube"}), "").unwrap(),
            Action::OpenUrl { url: "https://www.youtube.com".into(), browser: None }
        );
    }

    #[test]
    fn falls_back_to_the_utterance_for_an_empty_note() {
        let a = action_from_call("create_note", &json!({}), "the plumber comes at 3").unwrap();
        assert_eq!(a, Action::CreateNote { title: "The plumber comes at 3".into(), body: "The plumber comes at 3".into() });
    }

    #[test]
    fn refuses_what_it_cannot_repair() {
        assert!(action_from_call("open_app", &json!({}), "x").is_err());
        assert!(action_from_call("open_url", &json!({"url": "javascript:alert(1)"}), "").is_err());
        assert!(action_from_call("open_app", &json!({"name": "/bin/sh"}), "").is_err());
        let e = action_from_call("delete_everything", &json!({}), "").unwrap_err();
        assert!(e.contains("open_app"), "the error lists the real tools: {e}");
    }

    #[test]
    fn decisions_become_calls_or_chat() {
        let schema = decision_schema();
        let actions = schema["properties"]["action"]["enum"].as_array().unwrap();
        assert_eq!(actions.len(), TOOLS.len() + 1);
        assert!(actions.contains(&json!("chat")));
        assert_eq!(decision_to_call(&json!({"action": "chat", "reply": "Hello"})), None);
        let (name, args) = decision_to_call(&json!({"action": "open_app", "app": "Mail", "reply": "x"})).unwrap();
        assert_eq!(name, "open_app");
        assert_eq!(args, json!({"app": "Mail"}));
        // The flat fields map onto the tools through their aliases.
        assert_eq!(action_from_call(&name, &args, "").unwrap(), Action::OpenApp { name: "Mail".into() });
        let (n, a) = decision_to_call(&json!({"action": "create_note", "text": "the plumber comes at 3"})).unwrap();
        assert!(matches!(action_from_call(&n, &a, "").unwrap(), Action::CreateNote { body, .. } if body == "The plumber comes at 3"));
    }

    fn catalog() -> Catalog {
        Catalog { shortcuts: vec!["Focus Timer".into(), "démarre la musique".into()], hands: true, browser: true, claude: true }
    }

    fn browse(args: Value, said: &str) -> Result<Action, String> {
        action_from_call_in("use_browser", &args, said, &catalog())
    }

    #[test]
    fn the_browser_tool_is_offered_only_when_agent_browser_is_there() {
        let names = |c: &Catalog| -> Vec<String> {
            tools_json_for(c).as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap().to_string()).collect()
        };
        assert!(names(&catalog()).contains(&"use_browser".to_string()));
        assert!(!names(&Catalog { browser: false, ..catalog() }).contains(&"use_browser".to_string()));
        let tool = tools_json_for(&catalog()).as_array().unwrap().iter().find(|t| t["function"]["name"] == "use_browser").cloned().unwrap();
        assert_eq!(tool["function"]["parameters"]["required"], json!(["text"]));
        assert!(decision_schema_for(&catalog())["properties"]["action"]["enum"].as_array().unwrap().contains(&json!("use_browser")));
        assert!(!decision_schema()["properties"]["action"]["enum"].as_array().unwrap().contains(&json!("use_browser")));
        let guide = decision_guide_for(&catalog());
        assert!(guide.contains("use_browser:") && guide.contains("comparing prices") && guide.contains("web_search"), "{guide}");
        assert!(!decision_guide_for(&Catalog::default()).contains("use_browser"));
        assert!(action_from_call("use_browser", &json!({"text": "x"}), "x").is_err(), "no agent-browser, no tool");
    }

    #[test]
    fn use_browser_takes_the_task_and_only_a_url_the_user_said() {
        let said = "on amazon.com find the price of the Kindle Paperwhite";
        assert_eq!(
            browse(json!({"text": "find the price of the Kindle Paperwhite", "url": "amazon.com"}), said).unwrap(),
            Action::UseBrowser { task: "find the price of the Kindle Paperwhite".into(), url: Some("https://amazon.com".into()) }
        );
        // A deeper page on the host the user named is still that site.
        assert_eq!(
            browse(json!({"text": "x", "url": "https://www.amazon.com/s?k=kindle"}), said).unwrap(),
            Action::UseBrowser { task: "x".into(), url: Some("https://www.amazon.com/s?k=kindle".into()) }
        );
        // A site named by name keeps an address on that site, subdomains included.
        assert_eq!(
            browse(json!({"text": "who created Simula", "url": "https://en.wikipedia.org"}), "on Wikipedia, find who created Simula")
                .unwrap(),
            Action::UseBrowser { task: "who created Simula".into(), url: Some("https://en.wikipedia.org".into()) }
        );
        // An address the user never gave is dropped: the run starts from the page already open.
        assert_eq!(
            browse(json!({"text": "compare kindle prices", "url": "https://deals.example.net"}), "compare Kindle prices").unwrap(),
            Action::UseBrowser { task: "compare kindle prices".into(), url: None }
        );
        assert_eq!(
            browse(json!({"text": "x", "url": "https://x.com"}), "on fix.com find x").unwrap(),
            Action::UseBrowser { task: "x".into(), url: Some("https://fix.com".into()) },
            "a host inside another one was not said: the site the user did say is where it starts"
        );
        assert_eq!(
            browse(json!({"text": "x", "url": "https://wikipedia.org.evil.net"}), "on Wikipedia, find x").unwrap(),
            Action::UseBrowser { task: "x".into(), url: None },
            "a look-alike host is not the site named"
        );
        // Only http(s).
        assert_eq!(
            browse(json!({"text": "x", "url": "javascript:alert(1)"}), "javascript:alert(1)").unwrap(),
            Action::UseBrowser { task: "x".into(), url: None }
        );
        // The task falls back to the request; aliases work.
        assert_eq!(
            action_from_call_in("browser", &json!({"url": "github.com"}), "on github.com find the newest Rune release", &catalog())
                .unwrap(),
            Action::UseBrowser { task: "on github.com find the newest Rune release".into(), url: Some("https://github.com".into()) }
        );
        let (n, a) = decision_to_call(&json!({"action": "use_browser", "text": "read the top story", "url": "", "reply": "x"})).unwrap();
        assert_eq!(
            action_from_call_in(&n, &a, "read me the top story on hacker news", &catalog()).unwrap(),
            Action::UseBrowser { task: "read the top story".into(), url: None }
        );
    }

    #[test]
    fn a_site_the_user_said_is_where_the_browser_starts_even_when_the_model_forgets_it() {
        // Seen in a recording: the model put "on en.wikipedia.org" in the task and left url empty.
        let said = "on en.wikipedia.org, find out who created the Simula language";
        assert_eq!(
            action_from_call_in(
                "use_browser",
                &json!({"text": "find out who created the Simula language on en.wikipedia.org"}),
                said,
                &catalog()
            )
            .unwrap(),
            Action::UseBrowser {
                task: "find out who created the Simula language on en.wikipedia.org".into(),
                url: Some("https://en.wikipedia.org".into())
            }
        );
        // No address said: none is made up, from the task or anywhere else.
        assert_eq!(
            action_from_call_in(
                "use_browser",
                &json!({"text": "read the top story on news.ycombinator.com"}),
                "read me the top story",
                &catalog()
            )
            .unwrap(),
            Action::UseBrowser { task: "read the top story on news.ycombinator.com".into(), url: None }
        );
    }

    #[test]
    fn a_message_keeps_only_a_recipient_the_user_named() {
        let hands = crate::catalog::Catalog { hands: true, ..Default::default() };
        let call = |args: Value, said: &str| action_from_call_in("send_message", &args, said, &hands).unwrap();
        assert_eq!(
            call(json!({"app": "whatsapp", "to": "Ada", "text": "See you at 7"}), "tell Ada on WhatsApp see you at 7"),
            Action::SendMessage { app: "WhatsApp".into(), to: Some("Ada".into()), text: "See you at 7".into() }
        );
        assert_eq!(
            call(json!({"app": "WhatsApp", "to": "Grace", "text": "On my way"}), "reply to my latest WhatsApp message saying on my way"),
            Action::SendMessage { app: "WhatsApp".into(), to: None, text: "On my way".into() },
            "a guessed recipient would open someone else's chat"
        );
        let offered = |c: &crate::catalog::Catalog| tools_json_for(c).to_string().contains("\"send_message\"");
        assert!(offered(&hands) && !offered(&crate::catalog::Catalog::default()), "only with on-screen control");
    }

    #[test]
    fn a_task_on_the_users_own_page_goes_to_their_tab() {
        // "this page", "here": the page the user is looking at, never with an address.
        assert_eq!(
            action_from_call_in(
                "use_browser",
                &json!({"text": "click Next", "tab": "yours", "url": "x.com"}),
                "click next here",
                &catalog()
            )
            .unwrap(),
            Action::UseTheirTab { task: "click Next".into() }
        );
        let (n, a) = decision_to_call(&json!({"action": "use_browser", "text": "what pokemon is shown", "tab": "user"})).unwrap();
        assert_eq!(
            action_from_call_in(&n, &a, "what is the pokemon you see?", &catalog()).unwrap(),
            Action::UseTheirTab { task: "what pokemon is shown".into() }
        );
        // Alfred's own tab stays the default.
        assert_eq!(
            action_from_call_in("use_browser", &json!({"text": "what pokemon is shown", "tab": "alfred"}), "what is it", &catalog())
                .unwrap(),
            Action::UseBrowser { task: "what pokemon is shown".into(), url: None }
        );
        assert_eq!(decision_schema_for(&catalog())["properties"]["tab"]["enum"], json!(["alfred", "yours"]));
    }

    #[test]
    fn optional_tools_appear_only_when_available() {
        assert_eq!(tools_json().as_array().unwrap().len(), TOOLS.len());
        let tools = tools_json_for(&catalog());
        let names: Vec<&str> = tools.as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"run_shortcut") && names.contains(&"use_app"));
        let schema = decision_schema_for(&catalog());
        assert_eq!(schema["properties"]["shortcut"]["enum"], json!(["Focus Timer", "démarre la musique"]));
        assert!(decision_schema()["properties"]["shortcut"].is_null());
        assert!(decision_guide_for(&catalog()).contains("The shortcuts: Focus Timer; démarre la musique."));
        assert!(!decision_guide_for(&Catalog::default()).contains("run_shortcut"));
    }

    #[test]
    fn shortcut_calls_resolve_to_real_names() {
        let c = catalog();
        assert_eq!(
            action_from_call_in("run_shortcut", &json!({"shortcut": "demarre la musique"}), "", &c).unwrap(),
            Action::RunShortcut { name: "démarre la musique".into(), input: None }
        );
        assert_eq!(
            action_from_call_in("run_shortcut", &json!({"shortcut": "Focus Timer", "text": "25 minutes"}), "", &c).unwrap(),
            Action::RunShortcut { name: "Focus Timer".into(), input: Some("25 minutes".into()) }
        );
        let e = action_from_call_in("run_shortcut", &json!({"shortcut": "Lights off"}), "", &c).unwrap_err();
        assert!(e.contains("Focus Timer"), "{e}");
        assert!(action_from_call("run_shortcut", &json!({"shortcut": "Focus Timer"}), "").is_err(), "no shortcuts, no tool");
    }

    #[test]
    fn use_app_needs_hands_and_takes_the_task() {
        let c = catalog();
        assert_eq!(
            action_from_call_in("use_app", &json!({"app": "mail", "text": "reply to the last email as a draft"}), "", &c).unwrap(),
            Action::UseApp { app: "Mail".into(), task: "reply to the last email as a draft".into() }
        );
        assert_eq!(
            action_from_call_in("use_app", &json!({"app": "Slack"}), "say hi in general", &c).unwrap(),
            Action::UseApp { app: "Slack".into(), task: "say hi in general".into() }
        );
        assert!(action_from_call("use_app", &json!({"app": "Mail", "text": "x"}), "").is_err(), "no hands, no tool");
        assert!(action_from_call_in("press_button", &json!({"app": "Mail", "name": "Send"}), "", &c).is_err(), "the model cannot press");
    }

    #[test]
    fn mail_calls_take_text_and_an_optional_sender() {
        assert_eq!(action_from_call("read_mail", &json!({}), "read my last email").unwrap(), Action::ReadMail { from: None });
        assert_eq!(action_from_call("read_email", &json!({"sender": "Ada"}), "").unwrap(), Action::ReadMail { from: Some("Ada".into()) });
        assert_eq!(
            action_from_call("reply_mail", &json!({"text": "i'll be there at 3", "from": "Ada"}), "").unwrap(),
            Action::DraftReply { from: Some("Ada".into()), text: "I'll be there at 3".into() }
        );
        assert_eq!(
            action_from_call("reply_mail", &json!({}), "answer the newest mail").unwrap(),
            Action::DraftReply { from: None, text: String::new() },
            "no text: the reply opens empty instead of quoting the request"
        );
        let (n, a) = decision_to_call(&json!({"action": "reply_mail", "text": "Thanks!", "reply": "Done"})).unwrap();
        assert_eq!(action_from_call(&n, &a, "").unwrap(), Action::DraftReply { from: None, text: "Thanks!".into() });
        assert!(decision_schema()["properties"]["action"]["enum"].as_array().unwrap().contains(&json!("read_mail")));
    }

    #[test]
    fn out_of_range_minutes_are_dropped() {
        let a = action_from_call("create_reminder", &json!({"title": "x", "minutes": -5}), "").unwrap();
        assert_eq!(a, Action::CreateReminder { title: "X".into(), minutes: None });
    }
}
