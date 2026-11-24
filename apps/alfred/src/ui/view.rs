//! What Alfred shows: a compact column with no background at all, floating above other windows like a
//! widget. The green fluid orb on top (drawn on the GPU by `gfx`), then what Alfred is doing (drag it to
//! move Alfred), a field to type or talk, and the latest exchange as bare text. Plain functions of the
//! state; handlers only mutate `Ui`.

use rune::tok::type_;
use rune::{Color, Cx, El, KeyEvent, TextStyle, Tx, canvas, col, el, row, text, text_input};

use super::gfx;
use super::orb;
use super::{ChipState, Mode, Ui};

/// The thin outline that marks the field and the chips; there is no fill anywhere.
pub fn rim() -> Color {
    rune::oklch(0.86, 0.11, 165.0).alpha(0.45)
}
pub fn ink() -> Color {
    rune::oklch(0.97, 0.03, 165.0)
}
pub fn muted() -> Color {
    rune::oklch(0.86, 0.06, 165.0)
}
/// The accent: the orb's bright mint.
pub fn mint() -> Color {
    rune::oklch(0.86, 0.15, 162.0)
}
fn deep() -> Color {
    rune::oklch(0.24, 0.06, 165.0)
}
fn good() -> Color {
    rune::oklch(0.86, 0.15, 150.0)
}
fn bad() -> Color {
    rune::oklch(0.74, 0.17, 25.0)
}

/// A soft, wide, dark shadow behind light text: invisible on a dark desktop, enough on a white window for
/// the words to stay readable, as macOS does for desktop icon labels. Not a background: nothing is filled.
fn legible(e: El) -> El {
    e.shadow(0.0, 18.0, rune::oklch(0.12, 0.02, 165.0).alpha(0.55))
}

/// Text sits on whatever is behind the window, so it is a little heavier than usual.
const REPLY: TextStyle = TextStyle::new(14.0, 560.0);
const SMALL: TextStyle = TextStyle::new(12.0, 600.0);
/// The orb's diameter and the room around it for the glow.
const ORB: f32 = 148.0;
const HALO: f32 = 34.0;

pub fn view(s: &Ui, _cx: &mut Cx<Ui>) -> El {
    col()
        .key("root")
        .w_full()
        .h_full()
        .padding(8.0)
        .gap(9.0)
        .items_center()
        .child(orb_block(s))
        .child(caption(s))
        .child(ask_field(s))
        .child(exchange(s))
        .child(footer(s))
}

/// The orb: water in a glass bubble, as GPU layers in one spot, and a round button over them to talk.
/// Listening adds rings rippling out; thinking whirls the water and breathes; replying swells its waves
/// and pulses with Alfred's voice.
fn orb_block(s: &Ui) -> El {
    let t = s.clock as f32;
    let e = orb::energy(s.mode, s.level);
    let data = [e, s.level, orb::mode_code(s.mode), 0.0];
    let side = ORB + 2.0 * HALO;
    let liquid = ORB * 0.97;
    let inset = HALO + (ORB - liquid) / 2.0;
    let slot = |kind: u32| rune::effect(kind).effect_data(data);
    let mut block = el().key("orb-block").size(side, side).scale(orb::swell(s.mode, s.level, t));
    if s.mode == Mode::Listening {
        block = block.child(slot(gfx::RINGS).effect_bleed(90.0).absolute(HALO, HALO).size(ORB, ORB).radius(ORB / 2.0));
    }
    block
        .child(slot(gfx::GLOW).effect_bleed(HALO).absolute(HALO, HALO).size(ORB, ORB).radius(ORB / 2.0))
        .child(slot(gfx::BODY).absolute(inset, inset).size(liquid, liquid).radius(liquid / 2.0))
        .child(slot(gfx::VEINS).rotate(orb::swirl(s.mode, t)).absolute(inset, inset).size(liquid, liquid).radius(liquid / 2.0))
        .child(slot(gfx::RIPPLE).effect_backdrop(true).absolute(inset, inset).size(liquid, liquid).radius(liquid / 2.0))
        .child(slot(gfx::LENS).effect_backdrop(true).effect_bleed(16.0).absolute(HALO, HALO).size(ORB, ORB).radius(ORB / 2.0))
        .child(
            el().key("orb")
                .role("button")
                .label(if s.mode == Mode::Listening { "Stop listening" } else { "Talk to Alfred" })
                .absolute(HALO, HALO)
                .size(ORB, ORB)
                .radius(ORB / 2.0)
                .on_press_tx(|s: &mut Ui, tx: &mut Tx| s.toggle_listening(tx)),
        )
}

/// What Alfred is doing, or what it heard so far. Dragging it moves Alfred around the screen.
fn caption(s: &Ui) -> El {
    let color = if s.notice.is_some() {
        bad()
    } else if s.mode == Mode::Idle {
        muted()
    } else {
        mint()
    };
    row()
        .key("caption-pill")
        .window_drag()
        .h(22.0)
        .px(8.0)
        .center()
        .child(legible(text(s.caption()).key("caption").font(SMALL).color(color).truncate()))
}

fn ask_field(s: &Ui) -> El {
    let listening = s.mode == Mode::Listening;
    let field = text_input(&s.input, type_::BODY.lh(20.0))
        .key("ask")
        .label("Ask Alfred")
        .single_line()
        .grow(1.0)
        .color(ink())
        .placeholder("Ask Alfred…", muted())
        .on_edit(|s: &mut Ui| &mut s.input)
        .on_submit(|s: &mut Ui, _: &mut Tx| s.submit())
        .on_input_key(|s: &mut Ui, ev: &KeyEvent, tx: &mut Tx| super::on_key(s, ev, tx));
    let mic = row()
        .key("mic")
        .role("button")
        .label(if listening { "Stop listening" } else { "Talk" })
        .size(28.0, 28.0)
        .radius(14.0)
        .center()
        .border(1.0, if listening { mint() } else { rim() })
        .fill(if listening { mint() } else { Color::TRANSPARENT })
        .hover(mint().alpha(0.4))
        .on_press_tx(|s: &mut Ui, tx: &mut Tx| s.toggle_listening(tx))
        .child(canvas(orb::mic(if listening { deep() } else { ink() })).size(14.0, 14.0));
    legible(row())
        .key("ask-row")
        .w_full()
        .h(40.0)
        .pl(14.0)
        .pr(6.0)
        .gap(8.0)
        .center_y()
        .radius(20.0)
        .border(1.0, if listening { mint().alpha(0.85) } else { rim() })
        .child(field)
        .child(mic)
}

/// The latest exchange only: what you asked, Alfred's answer, what it did, and a Yes/No when it waits for
/// one. Before any, one example of what to say.
fn exchange(s: &Ui) -> El {
    let Some((i, t)) = s.turns.iter().enumerate().next_back() else {
        return block("tips").child(legible(text("Try “open Notes and save buy milk”").key("tip-0").font(SMALL).color(muted()).wrap()));
    };
    let mut c = block(&format!("turn-{i}"));
    if !t.you.is_empty() {
        c = c.child(legible(text(format!("“{}”", t.you)).key(format!("you-{i}")).font(SMALL).color(muted()).truncate()));
    }
    let reply = if t.alfred.is_empty() { "…".to_string() } else { t.alfred.clone() };
    c = c.child(legible(text(reply).key(format!("alfred-{i}")).font(REPLY.lh(19.0)).color(ink()).wrap().max_lines(5)));
    if !t.chips.is_empty() {
        let mut chips = row().key(format!("chips-{i}")).gap(5.0);
        for (j, ch) in t.chips.iter().take(2).enumerate() {
            chips = chips.child(chip(&format!("chip-{i}-{j}"), &ch.label, ch.state));
        }
        c = c.child(chips);
    }
    if s.pending.is_some() {
        c = c.child(confirm_row());
    }
    c
}

/// A group of lines, with no box around it.
fn block(key: &str) -> El {
    col().key(key.to_string()).w_full().gap(5.0).px(6.0).items_center()
}

fn chip(key: &str, label: &str, state: ChipState) -> El {
    let (dot, mark) = match state {
        ChipState::Running => (mint(), "•"),
        ChipState::Done => (good(), "✓"),
        ChipState::Failed => (bad(), "!"),
    };
    legible(row())
        .key(key.to_string())
        .h(22.0)
        .px(8.0)
        .gap(5.0)
        .center_y()
        .radius(11.0)
        .border(1.0, dot.alpha(0.6))
        .child(text(mark).font(SMALL).color(dot))
        .child(text(label.to_string()).font(SMALL).color(ink()).truncate())
}

fn confirm_row() -> El {
    let button = |key: &'static str, label: &'static str, primary: bool, yes: bool| {
        row()
            .key(key)
            .role("button")
            .label(label)
            .h(28.0)
            .grow(1.0)
            .center()
            .radius(14.0)
            .border(1.0, if primary { mint() } else { rim() })
            .fill(if primary { mint() } else { Color::TRANSPARENT })
            .hover(if primary { mint().alpha(0.8) } else { mint().alpha(0.25) })
            .on_press(move |s: &mut Ui| s.confirm(yes))
            .child(text(label).font(type_::TITLE).color(if primary { deep() } else { ink() }))
    };
    row().key("confirm").w_full().gap(8.0).child(button("yes", "Yes", true, true)).child(button("no", "No", false, false))
}

/// The keys to remember, small. Dragging it moves Alfred too.
fn footer(s: &Ui) -> El {
    let hint = text(format!("{} · ⌘L talk · esc hide", pretty_shortcut(&s.shortcut))).key("status").font(SMALL).color(muted().alpha(0.85));
    row().key("footer").window_drag().h(18.0).px(8.0).center().child(legible(hint.truncate()))
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
