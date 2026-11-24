//! One request in, one [`Turn`] out: confirmations and follow-up answers first, then the router, then the
//! model with tools, then plain chat.
//!
//! After tools run, Alfred speaks from their outcomes instead of asking the model to summarize them. That
//! saves a second model call (about two seconds on a 0.6B model) and keeps a small model from
//! embellishing what happened.

use serde_json::Value;

use crate::action::{Action, Executor, Risk};
use crate::config::ToolMode;
use crate::llm::{ChatMessage, ChatModel, ModelReply, Role, ToolCall};
use crate::router::{self, Route, Slot};
use crate::tools::{DECISION_GUIDE, action_from_call, decision_schema, decision_to_call, tools_json};

/// Alfred's persona and rules, for the model.
pub const SYSTEM_PROMPT: &str = "You are Alfred, a discreet, warm and efficient butler-style assistant living on the user's Mac. \
You can act through tools: open or quit apps, save notes in Apple Notes, add reminders, search the web, open links and tell the time. \
When the user asks for something a tool can do, call that tool with complete plain-text arguments. \
Otherwise answer in one or two short spoken sentences, with no markdown, lists or emoji. \
Never claim to have done something you did not do with a tool, and never invent facts you cannot know.";

/// Where a turn's answer came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The deterministic router.
    Router,
    /// The model chose tools.
    Model,
    /// The model answered in words.
    Chat,
    /// Alfred asked a follow-up question.
    Clarify,
    /// A confirmation was answered or is awaited.
    Confirm,
    /// Something failed (no model, unreachable server).
    Error,
}

/// One executed action and how it went.
#[derive(Debug, Clone, PartialEq)]
pub struct Done {
    pub action: Action,
    pub result: Result<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    /// What Alfred says.
    pub reply: String,
    pub done: Vec<Done>,
    /// A risky action waiting for yes or no.
    pub pending: Option<Action>,
    pub source: Source,
}

impl Turn {
    fn say(reply: impl Into<String>, source: Source) -> Self {
        Self { reply: reply.into(), done: Vec::new(), pending: None, source }
    }
}

/// Progress while a turn runs, for the UI.
#[derive(Debug)]
pub enum Event<'a> {
    Thinking,
    Acting(&'a Action),
    Acted(&'a Done),
}

const MAX_ACTIONS_PER_TURN: usize = 3;
const MAX_MODEL_ROUNDS: usize = 2;

pub struct Agent {
    model: Option<Box<dyn ChatModel>>,
    history: Vec<(String, String)>,
    history_turns: usize,
    awaiting: Option<Slot>,
    pending: Option<Action>,
    mode: ToolMode,
}

impl Agent {
    pub fn new(model: Option<Box<dyn ChatModel>>) -> Self {
        Self { model, history: Vec::new(), history_turns: 3, awaiting: None, pending: None, mode: ToolMode::Structured }
    }

    pub fn with_tool_mode(mut self, mode: ToolMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_history_turns(mut self, n: usize) -> Self {
        self.history_turns = n;
        self
    }

    pub fn model_name(&self) -> Option<String> {
        self.model.as_ref().map(|m| m.describe())
    }

    /// The risky action waiting for an answer, if any.
    pub fn pending(&self) -> Option<&Action> {
        self.pending.as_ref()
    }

    pub fn reset(&mut self) {
        self.history.clear();
        self.awaiting = None;
        self.pending = None;
    }

    /// Handle what the user said or typed.
    pub fn handle(&mut self, text: &str, exec: &dyn Executor, on: &mut dyn FnMut(Event)) -> Turn {
        let text = text.trim();
        let turn = self.decide(text, exec, on);
        if !text.is_empty() {
            self.history.push((text.to_string(), turn.reply.clone()));
            let keep = self.history_turns.max(1);
            if self.history.len() > keep {
                self.history.drain(..self.history.len() - keep);
            }
        }
        turn
    }

    /// Answer the pending confirmation directly (the UI's Yes and No chips).
    pub fn answer_confirmation(&mut self, yes: bool, exec: &dyn Executor, on: &mut dyn FnMut(Event)) -> Turn {
        let Some(action) = self.pending.take() else {
            return Turn::say("There's nothing waiting for an answer.", Source::Confirm);
        };
        if !yes {
            return Turn::say("Very well, I'll leave it.", Source::Confirm);
        }
        let mut turn = run(vec![action], exec, on, true);
        turn.source = Source::Confirm;
        turn
    }

    fn decide(&mut self, text: &str, exec: &dyn Executor, on: &mut dyn FnMut(Event)) -> Turn {
        if text.is_empty() {
            return Turn::say("At your service.", Source::Chat);
        }
        if self.pending.is_some() {
            match yes_or_no(text) {
                Some(yes) => return self.answer_confirmation(yes, exec, on),
                None => self.pending = None, // a new request replaces the question
            }
        }
        if let Some(slot) = self.awaiting.take() {
            return match slot.fill(text) {
                Some(action) => self.finish(run(vec![action], exec, on, false)),
                None => Turn::say("Very well, never mind.", Source::Clarify),
            };
        }
        match router::route(text) {
            Some(Route::Act(action)) => self.finish(run(vec![action], exec, on, false)),
            Some(Route::Ask { question, slot, first }) => {
                let mut turn = match first {
                    Some(a) => run(vec![a], exec, on, false),
                    None => Turn::say("", Source::Clarify),
                };
                turn.reply = join([turn.reply.as_str(), question.as_str()]);
                turn.source = Source::Clarify;
                self.awaiting = Some(slot);
                turn
            }
            None => self.ask_model(text, exec, on),
        }
    }

    fn finish(&mut self, turn: Turn) -> Turn {
        self.pending = turn.pending.clone();
        turn
    }

    fn ask_model(&mut self, text: &str, exec: &dyn Executor, on: &mut dyn FnMut(Event)) -> Turn {
        let Some(model) = self.model.as_ref() else {
            return Turn::say(
                "I can only follow simple commands until a language model is set up: try \"open Notes\", \"take a note: …\" or \"search Google for …\".",
                Source::Error,
            );
        };
        on(Event::Thinking);
        // Small models follow a short, focused prompt best: in structured mode the decision guide leads.
        let system = match self.mode {
            ToolMode::Structured => format!("You are Alfred, a warm, discreet assistant on the user's Mac. {DECISION_GUIDE}"),
            ToolMode::Native => SYSTEM_PROMPT.to_string(),
        };
        let mut messages = vec![ChatMessage::new(Role::System, system)];
        for (user, alfred) in &self.history {
            messages.push(ChatMessage::new(Role::User, user.clone()));
            messages.push(ChatMessage::new(Role::Assistant, alfred.clone()));
        }
        messages.push(ChatMessage::new(Role::User, text));
        let tools: Value = tools_json();
        let schema: Value = decision_schema();
        for round in 0..MAX_MODEL_ROUNDS {
            let answer = match self.mode {
                ToolMode::Native => model.chat(&messages, &tools),
                ToolMode::Structured => model.chat_json(&messages, &schema).map(|d| match decision_to_call(&d) {
                    Some((name, arguments)) => {
                        ModelReply { content: String::new(), tool_calls: vec![ToolCall { id: None, name, arguments }] }
                    }
                    None => ModelReply {
                        content: d.get("reply").and_then(Value::as_str).unwrap_or_default().trim().to_string(),
                        tool_calls: Vec::new(),
                    },
                }),
            };
            let reply = match answer {
                Ok(r) => r,
                Err(e) => return Turn::say(e.to_string(), Source::Error),
            };
            if reply.tool_calls.is_empty() {
                let mut words = reply.content;
                if words.trim().is_empty() && self.mode == ToolMode::Structured {
                    // The model chose to talk but left the reply empty: ask it once more, free-form.
                    let mut plain = messages.clone();
                    plain[0] = ChatMessage::new(Role::System, SYSTEM_PROMPT);
                    words = model.chat(&plain, &Value::Array(Vec::new())).map(|r| r.content).unwrap_or_default();
                }
                if words.trim().is_empty() {
                    words = "I'm sorry, I didn't catch that. Could you say it another way?".into();
                }
                return Turn::say(words, Source::Chat);
            }
            let calls = dedupe(reply.tool_calls);
            let mut actions = Vec::new();
            let mut errors = Vec::new();
            for call in &calls {
                match action_from_call(&call.name, &call.arguments, text).map(|a| trust_only_what_was_said(a, text)) {
                    Ok(a) if !actions.contains(&a) => actions.push(a),
                    Ok(_) => {}
                    Err(e) => errors.push((call.clone(), e)),
                }
            }
            if !actions.is_empty() {
                actions.truncate(MAX_ACTIONS_PER_TURN);
                let mut turn = self.finish(run(actions, exec, on, false));
                turn.source = if turn.pending.is_some() { Source::Confirm } else { Source::Model };
                return turn;
            }
            if round + 1 == MAX_MODEL_ROUNDS {
                break;
            }
            // Tell the model what was wrong and let it try once more.
            match self.mode {
                ToolMode::Native => {
                    messages.push(ChatMessage { tool_calls: calls.clone(), ..ChatMessage::new(Role::Assistant, reply.content) });
                    for (call, e) in &errors {
                        messages.push(ChatMessage::tool_result(call, format!("Error: {e} Call the tool again with corrected arguments.")));
                    }
                }
                ToolMode::Structured => {
                    let tried = calls.iter().map(|c| format!("{} {}", c.name, c.arguments)).collect::<Vec<_>>().join("; ");
                    let why = errors.iter().map(|(_, e)| e.as_str()).collect::<Vec<_>>().join(" ");
                    messages.push(ChatMessage::new(Role::Assistant, tried));
                    messages.push(ChatMessage::new(Role::User, format!("That didn't work: {why} Decide again with the fields filled in.")));
                }
            }
        }
        Turn::say("I'm afraid I couldn't work out how to do that.", Source::Error)
    }
}

/// Small models fill optional fields the user never gave (a reminder delay of "1 minute"). Keep a delay
/// only when the request talks about minutes or hours.
fn trust_only_what_was_said(action: Action, said: &str) -> Action {
    match action {
        Action::CreateReminder { title, minutes: Some(_) } if !mentions_delay(said) => Action::CreateReminder { title, minutes: None },
        other => other,
    }
}

fn mentions_delay(said: &str) -> bool {
    let l = said.to_lowercase();
    ["minute", " min", "mins", "hour", " hr", "hrs"].iter().any(|w| l.contains(w))
}

/// Run actions in order; a risky one (unless `confirmed`) stops the run and becomes the pending question.
fn run(actions: Vec<Action>, exec: &dyn Executor, on: &mut dyn FnMut(Event), confirmed: bool) -> Turn {
    let mut done = Vec::new();
    let mut words = Vec::new();
    let mut pending = None;
    for action in actions {
        if action.risk() == Risk::Confirm && !confirmed {
            words.push(action.confirm_question());
            pending = Some(action);
            break;
        }
        on(Event::Acting(&action));
        let result = exec.execute(&action).map(|o| o.summary);
        words.push(match &result {
            Ok(summary) => summary.clone(),
            Err(e) => format!("I couldn't manage that: {e}"),
        });
        let d = Done { action, result };
        on(Event::Acted(&d));
        done.push(d);
    }
    let source = if pending.is_some() { Source::Confirm } else { Source::Router };
    Turn { reply: join(words.iter().map(String::as_str)), done, pending, source }
}

fn join<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts.into_iter().map(str::trim).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ")
}

/// Small models repeat the same call in one turn; keep the first of each.
fn dedupe(calls: Vec<ToolCall>) -> Vec<ToolCall> {
    let mut seen = Vec::new();
    calls
        .into_iter()
        .filter(|c| {
            let key = (c.name.clone(), c.arguments.to_string());
            if seen.contains(&key) {
                false
            } else {
                seen.push(key);
                true
            }
        })
        .collect()
}

/// `Some(true)` for yes, `Some(false)` for no, `None` for anything else.
pub fn yes_or_no(text: &str) -> Option<bool> {
    let t = router::clean(text).to_lowercase();
    let t = t.trim_end_matches(['.', '!']);
    const YES: &[&str] = &[
        "yes",
        "yeah",
        "yep",
        "yup",
        "sure",
        "ok",
        "okay",
        "go ahead",
        "do it",
        "please do",
        "confirm",
        "y",
        "of course",
        "yes please",
        "absolutely",
    ];
    const NO: &[&str] = &[
        "no",
        "nope",
        "nah",
        "cancel",
        "don't",
        "do not",
        "stop",
        "never mind",
        "nevermind",
        "n",
        "leave it",
        "no thanks",
        "no thank you",
    ];
    if YES.contains(&t) {
        Some(true)
    } else if NO.contains(&t) {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
