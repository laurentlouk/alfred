//! Writing a message in a chat app the quick way: open the conversation, type into the message box, and
//! hand the send button back for the user to confirm, with no model call.

use std::sync::{Arc, Mutex};

use alfred_core::llm::{LlmError, ModelReply, Scripted};
use serde_json::{Value, json};

use super::*;

/// A chat app shaped like WhatsApp: a search field, conversation rows named "Name, last message, time",
/// and, once a conversation is open, a message box whose send button appears only when it holds text.
#[derive(Default)]
struct FakeChat {
    state: Mutex<ChatState>,
}

#[derive(Default)]
struct ChatState {
    search: String,
    open: Option<String>,
    message: String,
    log: Vec<Vec<String>>,
    /// An app with one message box and no conversation list (an assistant app).
    single_box: bool,
}

const ROWS: &[(&str, &str)] = &[
    ("@s1:e10", "Ada Lovelace, See you at 6, 14:02"),
    ("@s1:e11", "Charles Babbage, The engine works, 13:40"),
    ("@s1:e12", "Grace Hopper, Found the bug, Yesterday"),
];

impl ChatState {
    fn window(&self) -> Value {
        if self.single_box {
            let mut main = vec![json!({"ref_id": "@s1:e45", "role": "textarea", "name": "Reply to the assistant", "value": self.message})];
            if !self.message.is_empty() {
                main.push(json!({"ref_id": "@s1:e46", "role": "button", "name": "Send message"}));
            }
            return json!({"window": {"title": "Assistant"}, "tree": {"role": "window", "children": main}});
        }
        // Grace is further down the list: only a search shows her.
        let rows: Vec<Value> = ROWS
            .iter()
            .filter(|(_, name)| {
                if self.search.is_empty() { !name.starts_with("Grace") } else { name.to_lowercase().contains(&self.search.to_lowercase()) }
            })
            .map(|(r, name)| json!({"ref_id": r, "role": "button", "name": name}))
            .collect();
        let mut sidebar = vec![json!({"ref_id": "@s1:e3", "role": "textfield", "name": "\u{200e}Rechercher", "value": self.search})];
        sidebar.extend(rows);
        let mut main = Vec::new();
        if let Some(who) = &self.open {
            main.push(json!({"role": "heading", "name": who}));
            main.push(json!({"ref_id": "@s1:e45", "role": "textfield", "name": "\u{200e}Saisissez un message", "value": self.message}));
            main.push(if self.message.is_empty() {
                json!({"ref_id": "@s1:e47", "role": "button", "name": "Message vocal"})
            } else {
                json!({"ref_id": "@s1:e46", "role": "button", "name": "Envoyer"})
            });
        }
        json!({"window": {"title": "\u{200e}WhatsApp"}, "tree": {"role": "window", "children": [
            {"role": "group", "children": sidebar},
            {"role": "group", "children": main}
        ]}})
    }
}

impl Desktop for FakeChat {
    fn call(&self, args: &[String], stdin: Option<&str>) -> Result<Value, DesktopError> {
        let mut s = self.state.lock().unwrap();
        let mut logged = args.to_vec();
        if let Some(t) = stdin {
            logged.push(format!("<stdin:{t}>"));
        }
        s.log.push(logged);
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        match a.as_slice() {
            ["status"] => Ok(json!({"permissions": {"accessibility": {"state": "granted"}}})),
            ["snapshot", _, _, "--surface", "menubar", ..] => Ok(json!({"tree": {"role": "menu", "children": []}})),
            ["snapshot", ..] => Ok(s.window()),
            ["click", r] => {
                if let Some((_, name)) = ROWS.iter().find(|(id, _)| id == r) {
                    s.open = Some(name.split(',').next().unwrap().to_string());
                    s.message.clear();
                }
                Ok(json!({}))
            }
            ["type", "@s1:e3", "--stdin"] | ["set-value", "@s1:e3", "--stdin"] => {
                s.search = stdin.unwrap_or_default().to_string();
                Ok(json!({}))
            }
            ["type", "@s1:e45", "--stdin"] | ["set-value", "@s1:e45", "--stdin"] => {
                s.message = stdin.unwrap_or_default().to_string();
                Ok(json!({}))
            }
            other => Err(DesktopError { code: "INVALID_ARGS".into(), message: format!("unexpected {other:?}"), retry_safe: true }),
        }
    }
}

#[derive(Clone, Default)]
struct Chat(Arc<FakeChat>);

impl Desktop for Chat {
    fn call(&self, args: &[String], stdin: Option<&str>) -> Result<Value, DesktopError> {
        self.0.call(args, stdin)
    }
}

impl Chat {
    fn log(&self) -> Vec<Vec<String>> {
        self.0.state.lock().unwrap().log.clone()
    }

    fn clicked(&self, r: &str) -> bool {
        self.log().iter().any(|c| c.len() == 2 && c[0] == "click" && c[1] == r)
    }

    fn message(&self) -> String {
        self.0.state.lock().unwrap().message.clone()
    }
}

fn chat_hands(chat: &Chat, replies: Vec<Result<ModelReply, LlmError>>) -> (DesktopHands, Arc<Scripted>) {
    let model = Arc::new(Scripted::new(replies));
    (DesktopHands::new(Box::new(chat.clone()), Box::new(model.clone()), 8), model)
}

fn send_button(o: &Outcome) -> Action {
    o.follow_up.clone().expect("the send button comes back for a yes")
}

#[test]
fn a_named_chat_is_opened_and_written_to_with_no_model_call() {
    let chat = Chat::default();
    let (h, model) = chat_hands(&chat, vec![]);
    let o = h.send_message("WhatsApp", Some("Ada"), "See you at 7\nthanks").unwrap();
    assert_eq!(o.summary, "I've written “See you at 7 thanks” to Ada Lovelace in WhatsApp.");
    assert_eq!(send_button(&o), Action::PressButton { app: "WhatsApp".into(), role: "button".into(), name: "Envoyer".into() });
    assert!(chat.clicked("@s1:e10"), "Ada's conversation was opened");
    assert_eq!(chat.message(), "See you at 7 thanks", "typed without the line break, which would send");
    assert!(!chat.clicked("@s1:e46"), "never sent without a yes");
    assert!(model.seen.lock().unwrap().is_empty(), "no model call");
}

#[test]
fn a_chat_not_in_the_list_is_found_with_search() {
    let chat = Chat::default();
    let (h, _) = chat_hands(&chat, vec![]);
    let o = h.send_message("WhatsApp", Some("grace"), "Got it").unwrap();
    assert_eq!(o.summary, "I've written “Got it” to Grace Hopper in WhatsApp.");
    assert!(chat.log().iter().any(|c| c[0] == "type" && c[1] == "@s1:e3"), "searched for her");
    assert!(chat.clicked("@s1:e12"));
}

#[test]
fn replying_to_the_latest_message_opens_the_newest_conversation() {
    let chat = Chat::default();
    let (h, _) = chat_hands(&chat, vec![]);
    let o = h.send_message("WhatsApp", None, "On my way").unwrap();
    assert_eq!(o.summary, "I've written “On my way” to Ada Lovelace in WhatsApp.");
    assert!(chat.clicked("@s1:e10"), "the first row is the newest conversation");
}

#[test]
fn an_app_with_one_message_box_is_written_in_directly() {
    let chat = Chat::default();
    chat.0.state.lock().unwrap().single_box = true;
    let (h, _) = chat_hands(&chat, vec![]);
    let o = h.send_message("Claude", None, "Summarize today's AI news").unwrap();
    assert_eq!(o.summary, "I've written “Summarize today's AI news” in Claude.");
    assert_eq!(send_button(&o), Action::PressButton { app: "Claude".into(), role: "button".into(), name: "Send message".into() });
}

#[test]
fn when_no_conversation_matches_the_usual_steps_take_over() {
    let chat = Chat::default();
    let plan = Scripted::calls("plan", json!({"steps": ["Find the conversation"]}));
    let (h, model) = chat_hands(&chat, vec![plan, Scripted::calls("fail", json!({"note": "There is no conversation with Zelda."}))]);
    let e = h.send_message("WhatsApp", Some("Zelda"), "Hello").unwrap_err();
    assert_eq!(e, "In WhatsApp: There is no conversation with Zelda.");
    assert!(!model.seen.lock().unwrap().is_empty(), "the model took over");
    assert!(chat.message().is_empty(), "nothing was typed into a wrong conversation");
}

/// What the quick message path would find in real apps, without typing anything: the key places' names
/// (placeholders such as "Type a message") and how many conversations the window lists, never their names.
/// Run with `ALFRED_LIVE_APPS="WhatsApp,Messages,Claude" cargo test -p alfred-hands --lib live_chat_places -- --ignored --nocapture`.
#[test]
#[ignore = "looks at real apps on screen"]
fn live_chat_places() {
    let apps = std::env::var("ALFRED_LIVE_APPS").unwrap_or_else(|_| "WhatsApp".into());
    let Some(program) =
        ["/opt/homebrew/bin/agent-desktop", "/usr/local/bin/agent-desktop"].iter().map(std::path::PathBuf::from).find(|p| p.exists())
    else {
        return;
    };
    let hands = DesktopHands::new(Box::new(crate::desktop::AgentDesktop::new(program)), Box::new(Scripted::new([])), 8);
    for app in apps.split(',').map(str::trim) {
        let view = match hands.snapshot_window(app) {
            Ok(snap) => compact(&snap, WINDOW_LIMIT),
            Err(e) => {
                eprintln!("{app}: {e}");
                continue;
            }
        };
        let p = places(&view.refs);
        let show =
            |k: &Option<(String, Element)>| k.as_ref().map(|(_, e)| format!("{} {:?}", e.role, e.name)).unwrap_or_else(|| "none".into());
        eprintln!(
            "{app}: message box {} | search {} | send {} | conversations listed {} | newest found {}",
            show(&p.message_box),
            show(&p.search),
            show(&p.send),
            conversations(&view).iter().filter(|(_, e)| e.name.split(',').count() >= 2).count(),
            newest_conversation(&view).is_some()
        );
    }
}
