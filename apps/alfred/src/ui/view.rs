//! What the panel shows. Plain functions of the state; every handler only mutates `Ui`.

use rune::tok::type_;
use rune::{Color, Cx, El, KeyEvent, TextStyle, Tx, canvas, col, el, row, spacer, text, text_input};

use super::orb::{self, brass};
use super::{ChipState, Mode, Ui};

pub fn bg() -> Color {
    rune::oklch(0.16, 0.022, 265.0)
}
pub fn card() -> Color {
    rune::oklch(0.205, 0.024, 265.0)
}
pub fn hairline() -> Color {
    rune::oklch(0.30, 0.03, 265.0)
}
pub fn ink() -> Color {
    rune::oklch(0.96, 0.01, 85.0)
}
pub fn muted() -> Color {
    rune::oklch(0.70, 0.025, 265.0)
}
fn good() -> Color {
    rune::oklch(0.80, 0.14, 150.0)
}
fn bad() -> Color {
    rune::oklch(0.70, 0.17, 25.0)
}

const NAME: TextStyle = TextStyle::new(24.0, 650.0);
const REPLY: TextStyle = TextStyle::new(15.0, 450.0);

pub fn view(s: &Ui, _cx: &mut Cx<Ui>) -> El {
    let mut root = col()
        .key("root")
        .w_full()
        .h_full()
        .padding(22.0)
        .gap(16.0)
        .fill(bg())
        .radius(22.0)
        .clip_rounded(22.0)
        .border(1.0, brass().alpha(0.16))
        .child(header(s))
        .child(ask_field(s))
        .child(conversation(s));
    if let Some(action) = &s.pending {
        root = root.child(confirm_row(&action.short_label()));
    }
    root.child(spacer()).child(footer(s))
}

fn header(s: &Ui) -> El {
    let orb = canvas(orb::shape(s.clock as f32, s.mode, s.level))
        .key("orb")
        .role("button")
        .label(if s.mode == Mode::Listening { "Stop listening" } else { "Talk to Alfred" })
        .size(168.0, 168.0)
        .on_press_tx(|s: &mut Ui, tx: &mut Tx| s.toggle_listening(tx));
    let caption_color = if s.notice.is_some() {
        orb::ember()
    } else if s.mode == Mode::Idle {
        muted()
    } else {
        brass()
    };
    col()
        .key("header")
        .w_full()
        .gap(4.0)
        .center()
        .child(orb)
        .child(text("Alfred").key("name").font(NAME).color(ink()))
        .child(text(s.caption()).key("caption").font(type_::BODY).color(caption_color).wrap().max_lines(2))
}

fn ask_field(s: &Ui) -> El {
    let listening = s.mode == Mode::Listening;
    let field = text_input(&s.input, type_::BODY.lh(22.0))
        .key("ask")
        .label("Ask Alfred")
        .single_line()
        .grow(1.0)
        .color(ink())
        .placeholder("Ask Alfred: “open Notes and save buy milk”", muted())
        .on_edit(|s: &mut Ui| &mut s.input)
        .on_submit(|s: &mut Ui, _: &mut Tx| s.submit())
        .on_input_key(|s: &mut Ui, ev: &KeyEvent, tx: &mut Tx| super::on_key(s, ev, tx));
    let mic = row()
        .key("mic")
        .role("button")
        .label(if listening { "Stop listening" } else { "Talk" })
        .size(32.0, 32.0)
        .radius(16.0)
        .center()
        .fill(if listening { brass() } else { hairline() })
        .hover(brass().alpha(0.55))
        .on_press_tx(|s: &mut Ui, tx: &mut Tx| s.toggle_listening(tx))
        .child(canvas(orb::mic(if listening { bg() } else { ink() })).size(16.0, 16.0));
    row()
        .key("ask-row")
        .w_full()
        .h(50.0)
        .pl(16.0)
        .pr(9.0)
        .gap(10.0)
        .center_y()
        .radius(14.0)
        .fill(card())
        .border(1.0, if listening { brass().alpha(0.6) } else { hairline() })
        .child(field)
        .child(mic)
}

fn conversation(s: &Ui) -> El {
    let mut list = col().key("conversation").w_full().gap(14.0);
    if s.turns.is_empty() {
        let examples = [
            "“open Notes and save pick up the dry cleaning”",
            "“search Google for the Rune UI framework”",
            "“remind me in 20 minutes to stretch”",
        ];
        let mut tips = col().key("tips").w_full().gap(6.0).child(text("Try").font(type_::SECTION).color(muted()));
        for (i, e) in examples.iter().enumerate() {
            tips = tips.child(text(*e).key(format!("tip-{i}")).font(type_::BODY).color(ink().alpha(0.82)));
        }
        return list.child(tips);
    }
    for (i, t) in s.turns.iter().enumerate() {
        let latest = i + 1 == s.turns.len();
        let mut turn = col().key(format!("turn-{i}")).w_full().gap(5.0).opacity(if latest { 1.0 } else { 0.62 });
        if !t.you.is_empty() {
            turn = turn.child(text(format!("“{}”", t.you)).key(format!("you-{i}")).font(type_::BODY).color(muted()).wrap().max_lines(2));
        }
        let reply = if t.alfred.is_empty() && latest { "…".to_string() } else { t.alfred.clone() };
        if !reply.is_empty() {
            turn = turn.child(text(reply).key(format!("alfred-{i}")).font(REPLY.lh(21.0)).color(ink()).wrap().max_lines(4));
        }
        if !t.chips.is_empty() {
            let mut chips = row().key(format!("chips-{i}")).gap(6.0);
            for (j, c) in t.chips.iter().take(3).enumerate() {
                chips = chips.child(chip(&format!("chip-{i}-{j}"), &c.label, c.state));
            }
            turn = turn.child(chips);
        }
        list = list.child(turn);
    }
    list
}

fn chip(key: &str, label: &str, state: ChipState) -> El {
    let (dot, mark) = match state {
        ChipState::Running => (brass(), "•"),
        ChipState::Done => (good(), "✓"),
        ChipState::Failed => (bad(), "!"),
    };
    row()
        .key(key.to_string())
        .h(24.0)
        .px(9.0)
        .gap(6.0)
        .center_y()
        .radius(12.0)
        .fill(card())
        .border(1.0, dot.alpha(0.45))
        .child(text(mark).font(type_::LABEL).color(dot))
        .child(text(label.to_string()).font(type_::LABEL).color(ink()).truncate())
}

fn confirm_row(what: &str) -> El {
    let button = |key: &'static str, label: &'static str, primary: bool, yes: bool| {
        row()
            .key(key)
            .role("button")
            .label(label)
            .h(30.0)
            .px(16.0)
            .center()
            .radius(9.0)
            .fill(if primary { brass() } else { hairline() })
            .hover(if primary { brass().alpha(0.8) } else { muted().alpha(0.4) })
            .on_press(move |s: &mut Ui| s.confirm(yes))
            .child(text(label).font(type_::TITLE).color(if primary { bg() } else { ink() }))
    };
    row()
        .key("confirm")
        .w_full()
        .gap(10.0)
        .center_y()
        .padding(12.0)
        .radius(14.0)
        .fill(card())
        .border(1.0, brass().alpha(0.45))
        .child(
            col()
                .grow(1.0)
                .gap(2.0)
                .child(text("Needs your OK").font(type_::SECTION).color(brass()))
                .child(text(what.to_string()).key("confirm-what").font(type_::TITLE).color(ink())),
        )
        .child(button("yes", "Yes", true, true))
        .child(button("no", "No", false, false))
}

fn footer(s: &Ui) -> El {
    let key = |k: String, what: &'static str| {
        row()
            .gap(5.0)
            .center_y()
            .child(
                row()
                    .h(19.0)
                    .px(6.0)
                    .radius(5.0)
                    .center()
                    .fill(card())
                    .border(1.0, hairline())
                    .child(text(k).font(type_::LABEL).color(ink())),
            )
            .child(text(what).font(type_::LABEL).color(muted()))
    };
    let voice = if s.voice_ready { "Kokoro voice" } else { "system voice" };
    let status = if s.model_label.is_empty() { voice.to_string() } else { format!("{} · {voice}", short_model(&s.model_label)) };
    row()
        .key("footer")
        .w_full()
        .gap(12.0)
        .center_y()
        .child(key(pretty_shortcut(&s.shortcut), "summon"))
        .child(key("⏎".into(), "send"))
        .child(key("⌘L".into(), "talk"))
        .child(key("esc".into(), "hide"))
        .child(spacer())
        .child(el().child(text(status).key("status").font(type_::LABEL).color(muted()).truncate()))
}

fn short_model(label: &str) -> &str {
    label.split(" via ").next().unwrap_or(label)
}

/// "ctrl+alt+space" → "⌃⌥Space".
pub fn pretty_shortcut(s: &str) -> String {
    s.split('+')
        .map(|p| match p.trim().to_lowercase().as_str() {
            "ctrl" | "control" => "⌃".to_string(),
            "alt" | "option" | "opt" => "⌥".to_string(),
            "shift" => "⇧".to_string(),
            "cmd" | "command" | "super" | "mod" => "⌘".to_string(),
            other => alfred_core::normalize::capitalize(other),
        })
        .collect()
}
