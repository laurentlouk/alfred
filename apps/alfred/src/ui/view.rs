//! What Alfred shows: a compact column with no background at all, floating above other windows like a
//! widget. Nothing is painted behind the text, not even a shadow (Rune draws a shadow as a box). The green
//! fluid orb on top (drawn on the GPU by `gfx`; drag it to move Alfred), what Alfred is doing, and a field
//! to type in that grows like a chat's. Answers are spoken, not written, unless Alfred
//! has no voice; a question waiting for a yes shows with its Yes and No. Plain functions of the state;
//! handlers only mutate `Ui`.

use rune::tok::type_;
use rune::{Color, Cx, El, KeyEvent, ScrollSpec, TextStyle, Tx, canvas, col, el, row, text, text_input};

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

/// Text sits on whatever is behind the window, so it is a little heavier than usual.
const REPLY: TextStyle = TextStyle::new(14.0, 560.0);
const SMALL: TextStyle = TextStyle::new(12.0, 600.0);
/// The orb's diameter and the room around it for the glow and the listening rings. Everything the orb
/// draws, at its largest, stays inside this square (see the tests).
pub const ORB: f32 = 148.0;
pub const HALO: f32 = 46.0;
/// The longest the field grows, in lines, before it scrolls; and its line height.
const FIELD_LINES: usize = 4;
const FIELD_LINE: f32 = 20.0;

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

/// The orb: water in a glass bubble, as GPU layers in one spot. Listening adds rings rippling out;
/// thinking whirls the water and breathes; replying swells its waves and pulses with Alfred's voice. Its
/// square never changes size: only what is inside it swells, and the glow and rings are bounded so they
/// stay inside it, so nothing is cut at the window's edge. Dragging the orb moves Alfred.
fn orb_block(s: &Ui) -> El {
    let t = s.clock as f32;
    let e = orb::energy(s.mode, s.level);
    let data = [e, s.level, orb::mode_code(s.mode), 0.0];
    let side = ORB + 2.0 * HALO;
    let liquid = ORB * 0.97;
    let inset = HALO + (ORB - liquid) / 2.0;
    let slot = |kind: u32| rune::effect(kind).effect_data(data);
    let mut orb = el().key("orb-swell").absolute(0.0, 0.0).size(side, side).scale(orb::swell(s.mode, s.level, t));
    if s.mode == Mode::Listening {
        orb = orb.child(slot(gfx::RINGS).effect_bleed(HALO).absolute(HALO, HALO).size(ORB, ORB).radius(ORB / 2.0));
    }
    let orb = orb
        .child(slot(gfx::GLOW).effect_bleed(HALO).absolute(HALO, HALO).size(ORB, ORB).radius(ORB / 2.0))
        .child(slot(gfx::BODY).absolute(inset, inset).size(liquid, liquid).radius(liquid / 2.0))
        .child(slot(gfx::VEINS).rotate(orb::swirl(s.mode, t)).absolute(inset, inset).size(liquid, liquid).radius(liquid / 2.0))
        .child(slot(gfx::RIPPLE).effect_backdrop(true).absolute(inset, inset).size(liquid, liquid).radius(liquid / 2.0))
        .child(slot(gfx::LENS).effect_backdrop(true).effect_bleed(16.0).absolute(HALO, HALO).size(ORB, ORB).radius(ORB / 2.0));
    el().key("orb-block")
        .size(side, side)
        .child(orb)
        .child(el().key("orb").label("Alfred: drag to move").window_drag().absolute(HALO, HALO).size(ORB, ORB).radius(ORB / 2.0))
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
        .child(text(s.caption()).key("caption").font(SMALL).color(color).truncate())
}

/// The field grows with what is typed, like a chat's, up to a few lines, then scrolls with the cursor in
/// view. Return sends; Shift+Return starts a new line.
fn ask_field(s: &Ui) -> El {
    let listening = s.mode == Mode::Listening;
    let field = text_input(&s.input, type_::BODY.lh(FIELD_LINE))
        .key("ask")
        .label("Ask Alfred")
        .w_full()
        .color(ink())
        .placeholder("Ask Alfred…", muted())
        .on_edit(|s: &mut Ui| &mut s.input)
        .on_submit(|s: &mut Ui, _: &mut Tx| s.submit())
        .on_input_key(|s: &mut Ui, ev: &KeyEvent, tx: &mut Tx| super::on_key(s, ev, tx));
    // With "Alfred" listened for all the time, the button turns that off and on; otherwise it dictates.
    let (label, glyph, lit) = match (s.wake_word, s.muted) {
        (true, true) => ("Listen for “Alfred”", orb::mic_off(muted()), false),
        (true, false) => ("Stop listening", orb::mic(ink()), listening),
        (false, _) if listening => ("Stop listening", orb::mic(deep()), true),
        (false, _) => ("Talk", orb::mic(ink()), false),
    };
    let glyph = if lit { orb::mic(deep()) } else { glyph };
    let ring = if lit || (s.wake_word && !s.muted) { mint() } else { rim() };
    let mic = row()
        .key("mic")
        .role("button")
        .label(label)
        .size(28.0, 28.0)
        .radius(14.0)
        .center()
        .border(1.0, ring)
        .fill(if lit { mint() } else { Color::TRANSPARENT })
        .hover(mint().alpha(0.4))
        .on_press(|s: &mut Ui| s.press_mic())
        .child(canvas(glyph).size(14.0, 14.0));
    row()
        .key("ask-row")
        .w_full()
        .min_h(40.0)
        .py(6.0)
        .pl(14.0)
        .pr(6.0)
        .gap(8.0)
        .items_end()
        .radius(20.0)
        .border(1.0, if listening { mint().alpha(0.85) } else { rim() })
        .child(growing(field).grow(1.0).mb(4.0))
        .child(mic)
}

/// The field inside a viewport that grows a line at a time up to [`FIELD_LINES`], then scrolls, always
/// following the end: the latest words stay in view as they arrive, typed or dictated, like a chat's
/// composer.
fn growing(field: El) -> El {
    col()
        .key("scroll")
        .w_full()
        .shrink()
        .min_h(FIELD_LINE)
        .max_h(FIELD_LINES as f32 * FIELD_LINE)
        .scroll(ScrollSpec { stick_bottom: true })
        .child(field.animate_edit_layout())
}

/// Answers are spoken, so nothing is written, except a question waiting for a yes (with its Yes and No)
/// and, when Alfred has no voice, its latest answer.
fn exchange(s: &Ui) -> El {
    let last = s.turns.iter().enumerate().next_back();
    if s.pending.is_some()
        && let Some((i, t)) = last
    {
        return block(&format!("turn-{i}"))
            .child(text(t.alfred.clone()).key(format!("alfred-{i}")).font(REPLY.lh(19.0)).color(ink()).wrap().max_lines(3))
            .child(confirm_row());
    }
    let Some((i, t)) = last.filter(|_| !s.speaks_aloud()) else { return el().key("exchange") };
    let mut c = block(&format!("turn-{i}"));
    if !t.you.is_empty() {
        c = c.child(text(format!("“{}”", t.you)).key(format!("you-{i}")).font(SMALL).color(muted()).truncate());
    }
    let reply = if t.alfred.is_empty() { "…".to_string() } else { t.alfred.clone() };
    c = c.child(text(reply).key(format!("alfred-{i}")).font(REPLY.lh(19.0)).color(ink()).wrap().max_lines(5));
    if !t.chips.is_empty() {
        let mut chips = row().key(format!("chips-{i}")).gap(5.0);
        for (j, ch) in t.chips.iter().take(2).enumerate() {
            chips = chips.child(chip(&format!("chip-{i}-{j}"), &ch.label, ch.state));
        }
        c = c.child(chips);
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
    row()
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
    row().key("footer").window_drag().h(18.0).px(8.0).center().child(hint.truncate())
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
