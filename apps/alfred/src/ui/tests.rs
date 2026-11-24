use alfred_core::{Action, Source, Turn as AgentTurn};
use rune::{Env, Runtime, TextEdit};

use super::view::{pretty_shortcut, view};
use super::{ChipState, Mode, SIZE, Ui, scenes};

fn runtime(ui: Ui) -> Runtime<Ui> {
    let mut rt = Runtime::new(ui, view, SIZE, Env { system_dark: true, scale: 2.0 });
    rt.frame(0.0);
    rt
}

fn agent_turn(reply: &str, pending: Option<Action>) -> AgentTurn {
    AgentTurn { reply: reply.into(), done: Vec::new(), pending, source: Source::Router }
}

#[test]
fn every_scene_renders_the_orb_field_and_footer() {
    for (name, setup) in scenes::SCENES {
        let mut ui = Ui::demo();
        setup(&mut ui);
        let rt = runtime(ui);
        for key in ["orb", "ask", "footer", "caption"] {
            assert!(rt.find(key).is_some(), "scene {name} lacks {key}");
        }
    }
}

#[test]
fn an_empty_panel_suggests_what_to_say() {
    let rt = runtime(Ui::demo());
    assert!(rt.find("tips").is_some());
    assert!(rt.find("turn-0").is_none());
}

#[test]
fn submitting_starts_a_turn_and_clears_the_field() {
    let mut ui = Ui::demo();
    ui.input = TextEdit::new("open notes");
    ui.submit();
    assert_eq!(ui.turns.len(), 1);
    assert_eq!(ui.turns[0].you, "open notes");
    assert_eq!(ui.mode, Mode::Thinking);
    assert!(ui.input.text.is_empty());
    let rt = runtime(ui);
    assert!(rt.find("you-0").is_some() && rt.find("alfred-0").is_some(), "the pending answer shows as …");
}

#[test]
fn blank_input_does_nothing() {
    let mut ui = Ui::demo();
    ui.input = TextEdit::new("   ");
    ui.submit();
    assert!(ui.turns.is_empty());
    assert_eq!(ui.mode, Mode::Idle);
}

#[test]
fn answers_to_superseded_requests_are_ignored() {
    let mut ui = Ui::demo();
    ui.ask("first".into());
    ui.ask("second".into());
    ui.on_acting(1, "stale".into());
    ui.on_turn(1, agent_turn("stale answer", None));
    assert!(ui.turns.last().unwrap().chips.is_empty());
    assert!(ui.turns.last().unwrap().alfred.is_empty());
    ui.on_acting(2, "Opening Notes".into());
    ui.on_acted(2, true);
    ui.on_turn(2, agent_turn("Notes is open.", None));
    let last = ui.turns.last().unwrap();
    assert_eq!(last.alfred, "Notes is open.");
    assert_eq!(last.chips[0].state, ChipState::Done);
    assert_eq!(ui.mode, Mode::Idle, "no voice link in tests, so nothing to speak");
}

#[test]
fn the_conversation_keeps_the_last_three_turns() {
    let mut ui = Ui::demo();
    for i in 0..5 {
        ui.ask(format!("request {i}"));
    }
    assert_eq!(ui.turns.len(), 3);
    assert_eq!(ui.turns[0].you, "request 2");
}

#[test]
fn a_pending_action_shows_yes_and_no_and_yes_resolves_it() {
    let mut ui = Ui::demo();
    ui.ask("quit safari".into());
    ui.on_turn(1, agent_turn("Shall I quit Safari?", Some(Action::QuitApp { name: "Safari".into() })));
    let mut rt = runtime(ui);
    let (x, y) = rt.find("yes").expect("a Yes chip");
    assert!(rt.find("no").is_some());
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.pending.is_none());
    assert_eq!(rt.state.mode, Mode::Thinking, "the confirmation went to the brain");
}

#[test]
fn heard_silence_is_explained_not_sent() {
    let mut ui = Ui::demo();
    ui.ask("x".into());
    ui.on_heard(1, "  ".into());
    assert_eq!(ui.mode, Mode::Idle);
    assert!(ui.caption().contains("didn't catch"));
    ui.on_heard(1, "open notes".into());
    assert_eq!(ui.turns.last().unwrap().you, "open notes");
}

#[test]
fn dictation_without_the_model_says_so() {
    let mut ui = Ui::demo();
    ui.ears_ready = false;
    let mut rt = runtime(ui);
    let (x, y) = rt.find("mic").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.mode, Mode::Idle);
    assert!(rt.state.caption().contains("alfred setup"));
}

#[test]
fn shortcuts_print_as_mac_symbols() {
    assert_eq!(pretty_shortcut("ctrl+alt+space"), "⌃⌥Space");
    assert_eq!(pretty_shortcut("ctrl+shift+space"), "⌃⇧Space");
    assert_eq!(pretty_shortcut("cmd+shift+k"), "⌘⇧K");
}

#[test]
fn words_appear_in_the_field_while_listening() {
    let mut ui = Ui::demo();
    ui.attempt = 4;
    ui.mode = Mode::Listening;
    ui.typed = "in Notes,".into();
    ui.peeking = true;
    ui.on_partial(4, "save pick up the".into());
    assert_eq!(ui.input.text, "in Notes, save pick up the", "typed words stay in front");
    assert!(!ui.peeking, "the next live update may be asked for");
    ui.on_partial(3, "an older request".into());
    assert_eq!(ui.input.text, "in Notes, save pick up the", "stale text is ignored");
    ui.on_partial(4, "  ".into());
    assert_eq!(ui.input.text, "in Notes, save pick up the", "silence keeps what was shown");

    ui.on_heard(4, "save pick up the dry cleaning".into());
    assert_eq!(ui.turns.last().unwrap().you, "in Notes, save pick up the dry cleaning");
    assert_eq!(ui.input.text, "", "the request was sent; the field is clear");
}

#[test]
fn nothing_heard_puts_back_what_was_typed() {
    let mut ui = Ui::demo();
    ui.attempt = 2;
    ui.mode = Mode::Transcribing;
    ui.typed = "remind me".into();
    ui.input.replace_text("remind me to call");
    ui.on_heard(2, "".into());
    assert_eq!(ui.input.text, "remind me");
    assert_eq!(ui.mode, Mode::Idle);
}

#[test]
fn the_listening_scene_shows_live_words() {
    let (_, setup) = scenes::SCENES.iter().find(|(name, _)| *name == "listening").unwrap();
    let mut ui = Ui::demo();
    setup(&mut ui);
    assert_eq!(ui.input.text, "open Notes and save pick up the");
    assert!(runtime(ui).find("ask").is_some());
}
