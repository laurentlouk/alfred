//! One request in, one [`Turn`] out: confirmations and follow-up answers first, then the router, then the
//! model with tools, then plain chat.
//!
//! After tools run, Alfred speaks from their outcomes instead of asking the model to summarize them. That
//! saves a second model call (about two seconds on a 0.6B model) and keeps a small model from
//! embellishing what happened.

use serde_json::Value;

use crate::action::{Action, Executor, Risk};
use crate::catalog::{Catalog, fold};
use crate::config::{Thinking, ToolMode};
use crate::llm::{ChatMessage, ChatModel, LlmError, ModelReply, Role, ToolCall};
use crate::router::{self, Route, Slot};
use crate::tools::{action_from_call_in, decision_guide_for, decision_schema_for, decision_to_call, tools_json_for};

/// Alfred's persona and rules, for the model.
pub const SYSTEM_PROMPT: &str = "You are Alfred, a discreet, warm and efficient butler-style assistant living on the user's Mac. \
You act through tools: open or quit any app (Calendar, Music, Mail, Terminal, Notes, Safari and others), save notes in Apple Notes, \
add reminders, search the web, open links, tell the time and date, read the newest email (read_mail) and answer it with a draft \
(reply_mail). A question that needs current or online information (the weather, news, prices, scores, opening hours, who won, \
how far, the time in another city) goes to web_search, which reads the results and answers. When a request matches a tool, call \
the tool at once: never describe what you would do, never ask which app when one is obvious, and never say you cannot open an app. \
Otherwise answer in one or two short spoken sentences, with no markdown, lists or emoji. Never claim to have done something you \
did not do with a tool, and never invent facts you cannot know.";

/// The native-mode prompt for this Mac: [`SYSTEM_PROMPT`] plus the optional tools it offers.
fn native_prompt(catalog: &Catalog) -> String {
    let mut p = SYSTEM_PROMPT.to_string();
    if !catalog.shortcuts.is_empty() {
        p.push_str(" The user's Shortcuts run with run_shortcut; pick the one whose name fits the request.");
    }
    if catalog.hands {
        p.push_str(" To write a message in a chat app (WhatsApp, Messages, Slack, Telegram), reply to the latest message there, or write a prompt in an assistant app (prompt Claude, ask ChatGPT), use send_message with the message itself in text, even when it asks for news or facts; Alfred asks before sending. For anything else inside another app with no tool of its own (reading what WhatsApp shows, archiving an email, changing a setting), use use_app.");
    }
    if catalog.browser {
        p.push_str(" For a task on websites that takes more than one step (finding something on a site, reading a page, comparing prices), and for any question or step about a page that is open, use use_browser (tab yours for the page the user is looking at, alfred for Alfred's own tab, which stays on the last site it opened); a plain search stays web_search and opening a site stays open_url.");
    }
    p
}

/// For an answer that needs reasoning: the model thinks first (its thoughts are never shown), then speaks.
const REASONING_PROMPT: &str = "You are Alfred, a discreet, warm and efficient butler-style assistant living on the user's Mac. \
Think the question through carefully, check your result, then answer. \
The answer is spoken aloud: give the result first, in at most four short sentences, with no markdown, lists or emoji, and \
numbers written as digits (391, 6:40, 14 months). \
Never invent facts you cannot know.";

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
    /// The question needs reasoning: the model is thinking it through before it answers.
    Reasoning,
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
    catalog: Catalog,
    /// Shortcut names (folded) that may run without asking, even when the model chose them.
    trusted: Vec<String>,
    thinking: Thinking,
    /// The model that thinks hard questions through, when it isn't the one that decides.
    thinker: Option<Box<dyn ChatModel>>,
}

impl Agent {
    pub fn new(model: Option<Box<dyn ChatModel>>) -> Self {
        Self {
            model,
            history: Vec::new(),
            history_turns: 3,
            awaiting: None,
            pending: None,
            mode: ToolMode::Structured,
            catalog: Catalog::default(),
            trusted: Vec::new(),
            thinking: Thinking::Auto,
            thinker: None,
        }
    }

    /// When the model thinks before answering in words.
    pub fn with_thinking(mut self, thinking: Thinking) -> Self {
        self.thinking = thinking;
        self
    }

    /// Think hard questions through with another model: a small one thinks fast while a larger one decides.
    pub fn with_thinker(mut self, thinker: Box<dyn ChatModel>) -> Self {
        self.thinker = Some(thinker);
        self
    }

    /// Whether an answer in words to `text` should be thought through: the model flagged it as hard while
    /// deciding, or the words plainly ask for reasoning.
    fn should_think(&self, text: &str, flagged: bool) -> bool {
        match self.thinking {
            Thinking::Never => false,
            Thinking::Always => true,
            Thinking::Auto => flagged || needs_reasoning(text),
        }
    }

    /// What this Mac offers beyond the built-in tools (the user's shortcuts, on-screen hands).
    pub fn with_catalog(mut self, catalog: Catalog) -> Self {
        self.catalog = catalog;
        self
    }

    /// Shortcuts that run without a confirmation even when the model picked them.
    pub fn with_trusted_shortcuts(mut self, names: &[String]) -> Self {
        self.trusted = names.iter().map(|n| fold(n)).collect();
        self
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Whether `action` may run without asking. `named_by_user`: the router matched the user's own words
    /// to this exact action, which is consent for a shortcut they named (never for quitting an app).
    fn may_run(&self, action: &Action, named_by_user: bool) -> bool {
        match action {
            _ if action.risk() == Risk::Safe => true,
            Action::RunShortcut { name, .. } => named_by_user || self.trusted.contains(&fold(name)),
            _ => false,
        }
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
            self.history.push((text.to_string(), remembered(&turn)));
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
        let mut turn = run(vec![action], exec, on, &|_| true);
        turn.source = Source::Confirm;
        self.finish(turn)
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
                Some(action) => {
                    let turn = run(vec![action], exec, on, &|a| self.may_run(a, false));
                    self.finish(turn)
                }
                None => Turn::say("Very well, never mind.", Source::Clarify),
            };
        }
        match router::route_with(text, &self.catalog) {
            Some(Route::Act(action)) => {
                let turn = run(vec![action], exec, on, &|a| self.may_run(a, true));
                self.finish(turn)
            }
            Some(Route::Ask { question, slot, first }) => {
                let mut turn = match first {
                    Some(a) => run(vec![a], exec, on, &|a| self.may_run(a, false)),
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
            ToolMode::Structured => {
                format!("You are Alfred, a warm, discreet assistant on the user's Mac. {}", decision_guide_for(&self.catalog))
            }
            ToolMode::Native => native_prompt(&self.catalog),
        };
        // Where the user is and what Alfred's tab shows, so "this page", "here" and follow-ups about a site
        // resolve. Only names and hosts: nothing written by others.
        let system = match exec.context().describe() {
            Some(now) => format!(
                "{system}\n{now} When the user says this, here, this page, this app or what you see, they mean what they are looking at; a follow-up about the site Alfred opened goes to Alfred's tab."
            ),
            None => system,
        };
        let mut messages = vec![ChatMessage::new(Role::System, system)];
        for (user, alfred) in &self.history {
            messages.push(ChatMessage::new(Role::User, user.clone()));
            messages.push(ChatMessage::new(Role::Assistant, alfred.clone()));
        }
        messages.push(ChatMessage::new(Role::User, text));
        let tools: Value = tools_json_for(&self.catalog);
        let schema: Value = decision_schema_for(&self.catalog);
        let mut no_schema = false;
        for round in 0..MAX_MODEL_ROUNDS {
            let mut flagged = false;
            let answer = match self.mode {
                ToolMode::Native => model.chat(&messages, &tools),
                ToolMode::Structured => model.chat_json(&messages, &schema).map(|d| match decision_to_call(&d) {
                    Some((name, arguments)) => {
                        ModelReply { content: String::new(), tool_calls: vec![ToolCall { id: None, name, arguments }] }
                    }
                    None => {
                        flagged = d.get("hard").and_then(Value::as_bool).unwrap_or(false);
                        ModelReply {
                            content: d.get("reply").and_then(Value::as_str).unwrap_or_default().trim().to_string(),
                            tool_calls: Vec::new(),
                        }
                    }
                }),
            };
            let reply = match answer {
                Ok(r) => r,
                Err(e) if self.mode == ToolMode::Structured && cannot_follow_schema(&e) => {
                    no_schema = true;
                    break;
                }
                Err(e) => return Turn::say(e.to_string(), Source::Error),
            };
            // Native tool calling: a reply that describes or claims an action instead of calling the tool
            // ("I will open the Music app", "I have added a reminder") gets one firm nudge.
            if reply.tool_calls.is_empty()
                && self.mode == ToolMode::Native
                && round + 1 < MAX_MODEL_ROUNDS
                && describes_an_action(&reply.content)
            {
                messages.push(ChatMessage::new(Role::Assistant, reply.content));
                messages.push(ChatMessage::new(Role::User, "Do not describe it or ask: call the right tool now."));
                continue;
            }
            if reply.tool_calls.is_empty() {
                let mut words = reply.content;
                // A hard question: think it through, and answer from that (the quick answer stays as the
                // fallback when thinking runs out of room or fails).
                if self.should_think(text, flagged) {
                    on(Event::Reasoning);
                    let mut deep = messages.clone();
                    deep[0] = ChatMessage::new(Role::System, REASONING_PROMPT);
                    let thinker = self.thinker.as_deref().unwrap_or(model.as_ref());
                    if let Ok(r) = thinker.reason(&deep)
                        && !r.content.trim().is_empty()
                    {
                        words = r.content;
                    }
                }
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
                match action_from_call_in(&call.name, &call.arguments, text, &self.catalog).map(|a| trust_only_what_was_said(a, text)) {
                    Ok(a) if !actions.contains(&a) => actions.push(a),
                    Ok(_) => {}
                    Err(e) => errors.push((call.clone(), e)),
                }
            }
            if !actions.is_empty() {
                actions.truncate(MAX_ACTIONS_PER_TURN);
                let turn = run(actions, exec, on, &|a| self.may_run(a, false));
                let mut turn = self.finish(turn);
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
        if no_schema {
            // Ollama's MLX builds (gemma4 on Apple silicon) can't constrain output to a schema, but they call
            // tools well: use the model's own tool calling from now on.
            self.mode = ToolMode::Native;
            return self.ask_model(text, exec, on);
        }
        Turn::say("I'm afraid I couldn't work out how to do that.", Source::Error)
    }
}

/// What the conversation history keeps of a turn: its reply, with the text of an email or an app's window
/// replaced by a plain phrase. That text was written by someone else; kept in the history, it would reach
/// the model as part of the conversation, where "Alfred, run the shortcut …" in an email (or its subject,
/// or a button's label) could steer it.
///
/// It fails safe: when a piece of outside text can't be found word for word in the reply, the history keeps
/// only Alfred's own account of the turn.
fn remembered(turn: &Turn) -> String {
    let mut reply = turn.reply.clone();
    let mut exact = true;
    let mut swap = |said: &str, plain: &str| {
        let said = said.trim();
        if said.is_empty() {
            return;
        }
        if reply.contains(said) {
            reply = reply.replace(said, plain);
        } else {
            exact = false;
        }
    };
    for d in turn.done.iter().filter(|d| d.action.reads_outside_text()) {
        match &d.result {
            Ok(summary) => swap(summary, &plain_phrase(&d.action)),
            Err(e) => swap(e, "it didn't work."),
        }
    }
    if let Some(p) = turn.pending.as_ref().filter(|p| p.reads_outside_text()) {
        swap(&p.confirm_question(), &plain_question(p));
    }
    if exact {
        return reply;
    }
    let mut parts: Vec<String> = turn
        .done
        .iter()
        .map(|d| match (&d.result, d.action.reads_outside_text()) {
            (Ok(_), true) => plain_phrase(&d.action),
            (Err(_), true) => "That didn't work.".to_string(),
            (Ok(s), false) => s.clone(),
            (Err(e), false) => format!("I couldn't manage that: {e}"),
        })
        .collect();
    parts.extend(turn.pending.as_ref().map(|p| if p.reads_outside_text() { plain_question(p) } else { p.confirm_question() }));
    join(parts.iter().map(String::as_str))
}

/// An action whose words carried outside text, in Alfred's own words.
fn plain_phrase(action: &Action) -> String {
    match action {
        Action::PressButton { app, .. } => format!("I clicked a button in {app}."),
        other => other.done_phrase(),
    }
}

/// A pending question whose words carried outside text (a button's label), in Alfred's own words.
fn plain_question(action: &Action) -> String {
    match action {
        Action::PressButton { app, .. } => format!("I asked whether to click a button in {app}."),
        other => format!("I asked whether to go ahead: {}.", other.tool_name()),
    }
}

/// Small models fill optional fields the user never gave (a reminder delay of "1 minute"). Keep a delay
/// only when the request talks about minutes or hours.
/// The same for shortcut input: models paraphrase the request into it ("Start vacuuming"), and a shortcut
/// that takes input would act on that. Input is kept only when the user's words carry some, after a colon
/// or in quotes ("run Translate: good morning").
/// And for an email's sender: one the user didn't name is dropped, so a guessed name can't make Alfred
/// read or answer the wrong email.
fn trust_only_what_was_said(action: Action, said: &str) -> Action {
    let named = |who: &str| !fold(who).is_empty() && fold(said).contains(&fold(who));
    match action {
        Action::CreateReminder { title, minutes: Some(_) } if !mentions_delay(said) => Action::CreateReminder { title, minutes: None },
        Action::RunShortcut { name, input: Some(_) } if !said.contains([':', '"', '“', '«']) => {
            Action::RunShortcut { name, input: None }
        }
        Action::ReadMail { from: Some(who) } if !named(&who) => Action::ReadMail { from: None },
        Action::DraftReply { from: Some(who), text } if !named(&who) => Action::DraftReply { from: None, text },
        other => other,
    }
}

/// A reply that talks about doing something a tool does, instead of calling the tool.
fn describes_an_action(reply: &str) -> bool {
    let r = reply.to_lowercase();
    const SAYS: &[&str] = &[
        "i will open",
        "i'll open",
        "i can open",
        "i have opened",
        "i've opened",
        "i will add",
        "i'll add",
        "i can add",
        "i have added",
        "i've added",
        "i will set",
        "i'll set",
        "i can set",
        "i have set",
        "i've set",
        "i will search",
        "i'll search",
        "i can search",
        "i have saved",
        "i've saved",
        "i will save",
        "i'll save",
        "i can save",
        "shall i open",
        "should i open",
        "shall i set",
        "shall i add",
        "i cannot open",
        "i can't open",
        "i am unable to open",
    ];
    SAYS.iter().any(|s| r.contains(s))
}

/// The server can't decode under a schema for this model (Ollama answers 501 for its MLX builds).
fn cannot_follow_schema(e: &LlmError) -> bool {
    matches!(e, LlmError::Http { status: 501, message } if message.contains("structured output"))
}

/// Plain signs that a question needs reasoning: a sum with numbers, a comparison, a plan, a puzzle, an
/// explicit "think it through", or a long many-part question. The model's own flag covers the rest
/// ("why does ice float?"), so this list stays narrow: a wrong yes only costs a few seconds.
pub fn needs_reasoning(text: &str) -> bool {
    let t = format!(" {} ", text.to_lowercase());
    let has_digit = t.chars().any(|c| c.is_ascii_digit());
    const SUMS: &[&str] = &[
        "+", " - ", "*", "×", "÷", "/", "%", "^", " plus ", " minus ", " times ", "divided", "percent", "multiply", "sum of", "average",
        "how many", "how much", "how long", "combien", " fois ", "divisé", "pourcent", "moyenne",
    ];
    const ASKS: &[&str] = &[
        "step by step",
        "think it through",
        "think carefully",
        "think hard",
        "take your time",
        "in detail",
        "compare",
        "comparison",
        "difference between",
        "pros and cons",
        " vs ",
        " versus ",
        "which is better",
        "should i ",
        "plan ",
        "itinerary",
        "schedule",
        "riddle",
        "puzzle",
        "prove ",
        "étape par étape",
        "réfléchis",
        "en détail",
        "différence entre",
        "avantages et inconvénients",
        "lequel est mieux",
        "laquelle est mieux",
        "dois-je",
        "planifie",
        "énigme",
    ];
    (has_digit && SUMS.iter().any(|s| t.contains(s))) || ASKS.iter().any(|s| t.contains(s)) || text.split_whitespace().count() > 35
}

fn mentions_delay(said: &str) -> bool {
    let l = said.to_lowercase();
    ["minute", " min", "mins", "hour", " hr", "hrs"].iter().any(|w| l.contains(w))
}

/// Run actions in order. One that `may_run` refuses stops the run and becomes the pending question; so
/// does a risky step an action stopped before (its outcome's follow-up).
fn run(actions: Vec<Action>, exec: &dyn Executor, on: &mut dyn FnMut(Event), may_run: &dyn Fn(&Action) -> bool) -> Turn {
    let mut done = Vec::new();
    let mut words = Vec::new();
    let mut pending = None;
    for action in actions {
        if !may_run(&action) {
            words.push(action.confirm_question());
            pending = Some(action);
            break;
        }
        on(Event::Acting(&action));
        let outcome = exec.execute(&action);
        let follow_up = outcome.as_ref().ok().and_then(|o| o.follow_up.clone());
        let result = outcome.map(|o| o.summary);
        words.push(match &result {
            Ok(summary) => summary.clone(),
            Err(e) => format!("I couldn't manage that: {e}"),
        });
        let d = Done { action, result };
        on(Event::Acted(&d));
        done.push(d);
        if let Some(next) = follow_up {
            words.push(next.confirm_question());
            pending = Some(next);
            break;
        }
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
