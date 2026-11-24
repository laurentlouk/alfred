//! The tools the LLM may call, their JSON schema, and turning a (possibly mangled) tool call back into an
//! [`Action`].
//!
//! Small models pick the right tool but bend its arguments: a string comes back as `{"body": "…"}`, the
//! JSON arrives as a string inside a string, a parameter is renamed (`content` for `body`, `app` for
//! `name`). [`action_from_call`] repairs what it safely can and refuses the rest with a message the model
//! can act on.

use serde_json::{Value, json};

use crate::action::Action;
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
        description: "Search the web with Google in the browser and show the results.",
        params: &[p("query", "string", "What to search for.", true)],
    },
    ToolSpec {
        name: "open_url",
        description: "Open a web page in the browser.",
        params: &[p("url", "string", "The address, such as https://example.com.", true)],
    },
    ToolSpec { name: "get_time", description: "Tell the current local time and date.", params: &[] },
];

/// The `tools` array in the shape Ollama and OpenAI-compatible servers both accept.
pub fn tools_json() -> Value {
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
    let mut actions: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
    actions.push("chat");
    json!({
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": actions },
            "app": { "type": "string" },
            "text": { "type": "string" },
            "query": { "type": "string" },
            "url": { "type": "string" },
            "minutes": { "type": "integer" },
            "reply": { "type": "string" },
        },
        "required": ["action"],
    })
}

/// How to fill the decision, appended to the system prompt in structured mode.
pub const DECISION_GUIDE: &str = "Decide the single best action for the user's latest request and fill only its fields.
open_app: open an application (field app, such as Music, Calendar, Mail or Terminal).
quit_app: quit an application (field app).
create_note: save a note in Apple Notes (field text: the note's content).
create_reminder: add a reminder (field text: what to do; field minutes only if the user gave a delay).
web_search: search the web (field query).
open_url: open a website (field url).
get_time: the user asks the time, the date or the day.
chat: anything else; answer in one or two short sentences in field reply.";

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
        other => Err(format!(
            "There is no tool called \"{other}\". The tools are: {}.",
            TOOLS.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn out_of_range_minutes_are_dropped() {
        let a = action_from_call("create_reminder", &json!({"title": "x", "minutes": -5}), "").unwrap();
        assert_eq!(a, Action::CreateReminder { title: "X".into(), minutes: None });
    }
}
