//! Named states for headless snapshots (`alfred --snap out.png --scene conversation`) and the README.

use alfred_core::Action;

use super::{Chip, ChipState, Mode, Turn, Ui};

/// A named scene and how it changes the starting state.
type Scene = (&'static str, fn(&mut Ui));

pub const SCENES: &[Scene] = &[
    ("idle", |_| {}),
    ("listening", listening),
    ("thinking", thinking),
    ("speaking", speaking),
    ("conversation", conversation),
    ("confirm", confirm),
];

fn turn(you: &str, alfred: &str, chips: &[(&str, ChipState)]) -> Turn {
    Turn { you: you.into(), alfred: alfred.into(), chips: chips.iter().map(|(l, s)| Chip { label: (*l).into(), state: *s }).collect() }
}

fn listening(s: &mut Ui) {
    s.mode = Mode::Listening;
    s.level = 0.7;
    s.input.replace_text("open Notes and save pick up the");
}

fn thinking(s: &mut Ui) {
    s.mode = Mode::Thinking;
    s.level = 0.35;
    s.turns = vec![turn("could you jot down that the plumber comes at 3 on Thursday", "", &[])];
}

fn speaking(s: &mut Ui) {
    s.mode = Mode::Speaking;
    s.level = 0.8;
    s.turns = vec![turn(
        "open Notes and save pick up the dry cleaning",
        "I've saved a note: “Pick up the dry cleaning”.",
        &[("Saving note “Pick up the dry cleaning”", ChipState::Done)],
    )];
}

fn conversation(s: &mut Ui) {
    s.turns = vec![
        turn(
            "open notes and save buy milk and bread",
            "I've saved a note: “Buy milk and bread”.",
            &[("Saving note “Buy milk and bread”", ChipState::Done)],
        ),
        turn(
            "search google for the rune ui framework",
            "Here are the results for “the rune ui framework”.",
            &[("Searching “the rune ui framework”", ChipState::Done)],
        ),
        turn(
            "remind me in 20 minutes to stretch",
            "I've added a reminder: “Stretch”, due in 20 minutes.",
            &[("Adding reminder “Stretch”", ChipState::Done)],
        ),
    ];
}

fn confirm(s: &mut Ui) {
    s.turns = vec![turn("quit safari", "Shall I quit Safari?", &[])];
    s.pending = Some(Action::QuitApp { name: "Safari".into() });
}
