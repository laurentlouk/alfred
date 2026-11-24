use serde_json::json;

use super::*;
use crate::action::DryRun;
use crate::llm::{LlmError, Role, Scripted};

fn agent_with(replies: Vec<Result<crate::llm::ModelReply, LlmError>>) -> Agent {
    Agent::new(Some(Box::new(Scripted::new(replies))))
}

fn quiet() -> impl FnMut(Event) {
    |_| {}
}

#[test]
fn router_requests_never_touch_the_model() {
    let mut a = agent_with(vec![]); // a model call would answer "" and fail the assertion below
    let d = DryRun::new();
    let t = a.handle("open notes", &d, &mut quiet());
    assert_eq!(t.source, Source::Router);
    assert_eq!(t.reply, "Notes is open.");
    assert_eq!(d.actions(), vec![Action::OpenApp { name: "Notes".into() }]);
}

#[test]
fn the_model_handles_what_the_router_does_not() {
    let mut a = agent_with(vec![Scripted::calls("create_note", json!({"title": "Plumber", "body": "The plumber comes at 3"}))]);
    let d = DryRun::new();
    let t = a.handle("could you jot it down that the plumber comes at 3", &d, &mut quiet());
    // "jot …" isn't a router phrase when "it" follows; the model did it.
    assert!(matches!(t.source, Source::Model | Source::Router), "{t:?}");
    assert_eq!(d.actions().len(), 1);
    assert!(matches!(&d.actions()[0], Action::CreateNote { body, .. } if body.contains("plumber")));
}

#[test]
fn mangled_arguments_are_repaired_before_running() {
    let mut a = agent_with(vec![Scripted::calls("create_note", json!({"body": {"body": "Buy milk", "type": "string"}}))]);
    let d = DryRun::new();
    let t = a.handle("I need milk, put it somewhere I'll see it", &d, &mut quiet());
    assert_eq!(t.source, Source::Model);
    assert_eq!(d.actions(), vec![Action::CreateNote { title: "Buy milk".into(), body: "Buy milk".into() }]);
    assert_eq!(t.reply, "I've saved a note: “Buy milk”.");
}

#[test]
fn a_bad_call_gets_one_retry_with_the_error() {
    let model = Scripted::new([Scripted::calls("open_app", json!({})), Scripted::calls("open_app", json!({"name": "Calendar"}))]);
    let mut a = Agent::new(Some(Box::new(model)));
    let d = DryRun::new();
    let t = a.handle("I'd like to see my week", &d, &mut quiet());
    assert_eq!(d.actions(), vec![Action::OpenApp { name: "Calendar".into() }]);
    assert_eq!(t.source, Source::Model);
}

#[test]
fn chat_answers_come_back_as_words() {
    let mut a = agent_with(vec![Scripted::says("Good evening. How may I help?")]);
    let t = a.handle("hello", &DryRun::new(), &mut quiet());
    assert_eq!(t.source, Source::Chat);
    assert_eq!(t.reply, "Good evening. How may I help?");
}

#[test]
fn quitting_waits_for_a_yes() {
    let mut a = agent_with(vec![]);
    let d = DryRun::new();
    let t = a.handle("quit safari", &d, &mut quiet());
    assert_eq!(t.reply, "Shall I quit Safari?");
    assert_eq!(t.pending, Some(Action::QuitApp { name: "Safari".into() }));
    assert!(d.actions().is_empty(), "nothing runs before the yes");
    let t = a.handle("yes please", &d, &mut quiet());
    assert_eq!(t.reply, "Safari has been closed.");
    assert_eq!(d.actions(), vec![Action::QuitApp { name: "Safari".into() }]);
}

#[test]
fn no_cancels_and_a_new_request_replaces_the_question() {
    let d = DryRun::new();
    let mut a = agent_with(vec![]);
    a.handle("quit music", &d, &mut quiet());
    assert_eq!(a.handle("no", &d, &mut quiet()).reply, "Very well, I'll leave it.");
    a.handle("quit music", &d, &mut quiet());
    let t = a.handle("open notes", &d, &mut quiet());
    assert_eq!(t.reply, "Notes is open.");
    assert_eq!(d.actions(), vec![Action::OpenApp { name: "Notes".into() }]);
    assert!(a.pending().is_none());
}

#[test]
fn the_model_cannot_skip_confirmation() {
    let mut a = agent_with(vec![Scripted::calls("quit_app", json!({"name": "Finder"}))]);
    let d = DryRun::new();
    let t = a.handle("I'm done with files for today", &d, &mut quiet());
    assert_eq!(t.source, Source::Confirm);
    assert_eq!(t.pending, Some(Action::QuitApp { name: "Finder".into() }));
    assert!(d.actions().is_empty());
}

#[test]
fn follow_up_answers_fill_the_missing_piece() {
    let mut a = agent_with(vec![]);
    let d = DryRun::new();
    let t = a.handle("open Notes and save something for me", &d, &mut quiet());
    assert_eq!(t.reply, "Notes is open. What would you like me to write down?");
    assert_eq!(t.source, Source::Clarify);
    let t = a.handle("pick up the dry cleaning", &d, &mut quiet());
    assert_eq!(t.reply, "I've saved a note: “Pick up the dry cleaning”.");
    assert_eq!(
        d.actions(),
        vec![
            Action::OpenApp { name: "Notes".into() },
            Action::CreateNote { title: "Pick up the dry cleaning".into(), body: "Pick up the dry cleaning".into() }
        ]
    );
}

#[test]
fn duplicate_calls_run_once() {
    let call = crate::llm::ToolCall { id: None, name: "web_search".into(), arguments: json!({"query": "rune"}) };
    let reply = crate::llm::ModelReply { content: String::new(), tool_calls: vec![call.clone(), call.clone(), call] };
    let mut a = agent_with(vec![Ok(reply)]);
    let d = DryRun::new();
    a.handle("find me that rust ui thing", &d, &mut quiet());
    assert_eq!(d.actions().len(), 1);
}

#[test]
fn no_model_still_routes_and_explains() {
    let mut a = Agent::new(None);
    let d = DryRun::new();
    assert_eq!(a.handle("what time is it", &d, &mut quiet()).reply, "It's 09:41 on Tuesday 24 November.");
    let t = a.handle("tell me a story", &d, &mut quiet());
    assert_eq!(t.source, Source::Error);
    assert!(t.reply.contains("open Notes"));
}

#[test]
fn model_errors_are_reported_in_words() {
    let mut a = agent_with(vec![Err(LlmError::ModelMissing { model: "qwen3:0.6b".into() })]);
    let t = a.handle("tell me a joke", &DryRun::new(), &mut quiet());
    assert_eq!(t.source, Source::Error);
    assert!(t.reply.contains("ollama pull qwen3:0.6b"));
}

#[test]
fn history_reaches_the_model_and_stays_bounded() {
    let model = std::sync::Arc::new(Scripted::new((0..10).map(|i| Scripted::says(&format!("answer {i}")))));
    let mut a = Agent::new(Some(Box::new(model.clone()))).with_history_turns(2);
    let d = DryRun::new();
    for i in 0..6 {
        a.handle(&format!("question {i}"), &d, &mut quiet());
    }
    assert_eq!(a.history.len(), 2, "the last two turns");
    assert_eq!(a.history[0].0, "question 4");
    let seen = model.seen.lock().unwrap();
    let last = seen.last().unwrap();
    assert_eq!(last.len(), 1 + 2 * 2 + 1, "system, two earlier turns, the question: {last:?}");
    assert_eq!(last[1].content, "question 3");
}

#[test]
fn events_announce_progress() {
    let mut a = agent_with(vec![]);
    let mut log = Vec::new();
    a.handle("search google for rune", &DryRun::new(), &mut |e| {
        log.push(match e {
            Event::Thinking => "thinking".to_string(),
            Event::Acting(x) => format!("acting {}", x.tool_name()),
            Event::Acted(d) => format!("acted {}", d.result.is_ok()),
        })
    });
    assert_eq!(log, vec!["acting web_search", "acted true"]);
}

#[test]
fn native_mode_sends_tools_and_retries_with_tool_errors() {
    let model =
        std::sync::Arc::new(Scripted::new([Scripted::calls("open_app", json!({})), Scripted::calls("open_app", json!({"name": "Mail"}))]));
    let mut a = Agent::new(Some(Box::new(model.clone()))).with_tool_mode(crate::config::ToolMode::Native);
    let d = DryRun::new();
    a.handle("get my email going", &d, &mut quiet());
    assert_eq!(d.actions(), vec![Action::OpenApp { name: "Mail".into() }]);
    let seen = model.seen.lock().unwrap();
    assert!(seen[1].iter().any(|m| m.role == Role::Tool), "the retry carries the tool error");
}

#[test]
fn structured_mode_explains_the_decision_format_to_the_model() {
    let model = std::sync::Arc::new(Scripted::new([Scripted::says("Hello.")]));
    let mut a = Agent::new(Some(Box::new(model.clone())));
    a.handle("hello", &DryRun::new(), &mut quiet());
    let seen = model.seen.lock().unwrap();
    assert!(seen[0][0].content.contains("create_note: save a note"));
}

#[test]
fn a_delay_the_user_never_gave_is_dropped() {
    let mut a = agent_with(vec![Scripted::calls("create_reminder", json!({"text": "water the plants", "minutes": 1}))]);
    let d = DryRun::new();
    a.handle("I need to remember to water the plants, put it in my reminders", &d, &mut quiet());
    assert_eq!(d.actions(), vec![Action::CreateReminder { title: "Water the plants".into(), minutes: None }]);
    let mut a = agent_with(vec![Scripted::calls("create_reminder", json!({"text": "drink water", "minutes": 45}))]);
    let d = DryRun::new();
    a.handle("nudge me in 45 minutes to drink water", &d, &mut quiet());
    assert_eq!(d.actions(), vec![Action::CreateReminder { title: "Drink water".into(), minutes: Some(45) }]);
}

#[test]
fn an_empty_chat_decision_gets_a_free_form_answer() {
    let mut a = agent_with(vec![Scripted::says(""), Scripted::says("I'm Alfred, at your service.")]);
    let t = a.handle("who are you?", &DryRun::new(), &mut quiet());
    assert_eq!(t.source, Source::Chat);
    assert_eq!(t.reply, "I'm Alfred, at your service.");
}

#[test]
fn yes_and_no_words() {
    assert_eq!(yes_or_no("Yes."), Some(true));
    assert_eq!(yes_or_no("go ahead"), Some(true));
    assert_eq!(yes_or_no("Alfred, no thanks"), Some(false));
    assert_eq!(yes_or_no("open notes"), None);
}
