use alfred_core::{Action, Source, Turn as AgentTurn};
use rune::{Env, Runtime, TextEdit};

use super::view::{pretty_shortcut, view};
use super::{ChipState, Mode, SIZE, Ui, gfx, orb, scenes, view};

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
fn nothing_is_written_while_answers_are_spoken() {
    let mut ui = Ui::demo();
    ui.ask("open notes".into());
    ui.on_turn(1, agent_turn("Notes is open.", None));
    let rt = runtime(ui);
    assert!(rt.find("turn-0").is_none() && rt.find("alfred-0").is_none() && rt.find("tips").is_none());
}

#[test]
fn without_a_voice_the_answer_is_written() {
    let mut ui = Ui::demo();
    ui.voice_ready = false;
    ui.ask("open notes".into());
    ui.on_turn(1, agent_turn("Notes is open.", None));
    assert!(runtime(ui).find("alfred-0").is_some());
}

#[test]
fn with_the_wake_word_the_mic_turns_listening_off_and_on() {
    let mut ui = Ui::demo();
    ui.wake_word = true;
    let mut rt = runtime(ui);
    let press = |rt: &mut Runtime<Ui>| {
        let (x, y) = rt.find("mic").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    };
    press(&mut rt);
    assert!(rt.state.muted);
    assert_eq!(rt.state.caption(), "Not listening.");
    // Nothing is heard while it's off, even on demand.
    rt.state.toggle_listening();
    assert_eq!(rt.state.mode, Mode::Idle);
    assert!(rt.state.caption().contains("Listening is off"));
    press(&mut rt);
    assert!(!rt.state.muted);
}

#[test]
fn muting_while_listening_drops_the_command() {
    let mut ui = Ui::demo();
    ui.wake_word = true;
    ui.input = TextEdit::new("typed ");
    ui.toggle_listening();
    assert_eq!(ui.mode, Mode::Listening);
    ui.set_muted(true);
    assert_eq!(ui.mode, Mode::Idle);
    assert_eq!(ui.input.text, "typed");
}

#[test]
fn the_orb_moves_alfred_and_everything_it_draws_stays_in_its_square() {
    let rt = runtime(Ui::demo());
    assert!(rt.find("orb").is_some(), "the orb is there to drag");
    let half = view::ORB / 2.0 + view::HALO;
    let reach = gfx::ring_reach(1.0).max(gfx::glow_radius(1.0, 1.0));
    assert!((view::ORB / 2.0 + reach) * orb::MAX_SWELL <= half, "rings or glow would be cut at the window's edge");
    for t in 0..200 {
        assert!(orb::swell(Mode::Speaking, 1.0, t as f32 * 0.05) <= orb::MAX_SWELL);
    }
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
    ui.on_partial(4, "save pick up the".into());
    assert_eq!(ui.input.text, "in Notes, save pick up the", "typed words stay in front");
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

#[test]
fn saying_alfred_starts_a_request_and_the_ears_carry_it_through() {
    use alfred_voice::Heard;
    let mut ui = Ui::demo();
    ui.input.replace_text("in Notes,");
    ui.on_ears(Heard::Wake);
    assert_eq!(ui.mode, Mode::Listening);
    ui.on_ears(Heard::Level(0.6));
    assert!((ui.mic_level - 0.6).abs() < 1e-6, "the orb follows the voice");
    ui.on_ears(Heard::Partial("save buy".into()));
    assert_eq!(ui.input.text, "in Notes, save buy");
    ui.on_ears(Heard::Final("save buy milk".into()));
    assert_eq!(ui.turns.last().unwrap().you, "in Notes, save buy milk");
    assert_eq!(ui.mode, Mode::Thinking);
}

#[test]
fn a_command_with_no_words_or_a_microphone_error_is_explained() {
    use alfred_voice::Heard;
    let mut ui = Ui::demo();
    ui.input.replace_text("remind me");
    ui.on_ears(Heard::Wake);
    ui.on_ears(Heard::Nothing);
    assert_eq!((ui.mode, ui.input.text.as_str()), (Mode::Idle, "remind me"));
    assert!(ui.caption().contains("didn't hear"));
    ui.on_ears(Heard::Final("stray words after the end".into()));
    assert!(ui.turns.is_empty(), "a late result changes nothing once listening ended");
    ui.on_ears(Heard::Error("the microphone went away".into()));
    assert!(ui.caption().contains("microphone went away"));
}

#[test]
fn the_footer_says_when_alfred_is_recording() {
    let mut ui = Ui::demo();
    assert!(!view::footer_text(&ui).contains("rec"));
    ui.recording = true;
    assert!(view::footer_text(&ui).starts_with("● rec · "), "{}", view::footer_text(&ui));
    assert!(runtime(ui).find("status").is_some());
}
