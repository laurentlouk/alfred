//! The Alfred orb: concentric rings of brass dots around a warm core. Idle, it drifts; listening, it
//! swells with the microphone; thinking, a bright sweep circles it; speaking, it ripples with the voice.

use std::f32::consts::TAU;

use rune::{Color, Shape};

use super::Mode;

pub fn brass() -> Color {
    rune::oklch(0.80, 0.13, 78.0)
}

pub fn ember() -> Color {
    rune::oklch(0.70, 0.16, 55.0)
}

pub fn core() -> Color {
    rune::oklch(0.97, 0.05, 90.0)
}

const VIEW: f32 = 200.0;
const RINGS: usize = 6;

fn circle(c: [f32; 2], r: f32, n: usize) -> Vec<[f32; 2]> {
    (0..n)
        .map(|i| {
            let a = i as f32 * TAU / n as f32;
            [c[0] + a.cos() * r, c[1] + a.sin() * r]
        })
        .collect()
}

/// How lively the orb is in a mode, 0 to 1, before the level is added.
fn energy(mode: Mode, level: f32) -> f32 {
    let base = match mode {
        Mode::Idle => 0.34,
        Mode::Listening => 0.45,
        Mode::Transcribing | Mode::Thinking => 0.55,
        Mode::Speaking => 0.40,
    };
    (base + level * 0.6).min(1.0)
}

/// The orb at time `t` (seconds) in a 200 × 200 view box.
pub fn shape(t: f32, mode: Mode, level: f32) -> Shape {
    let c = [VIEW / 2.0, VIEW / 2.0];
    let e = energy(mode, level);
    let mut s = Shape::new();

    // Glow: soft discs behind everything.
    for (r, a) in [(96.0, 0.035), (78.0, 0.05), (58.0, 0.07), (40.0, 0.09)] {
        s = s.fill(circle(c, r * (0.92 + 0.12 * e), 64), brass().alpha(a * (0.6 + e)));
    }

    let speed = match mode {
        Mode::Thinking | Mode::Transcribing => 1.4,
        Mode::Listening => 0.35,
        Mode::Speaking => 0.25,
        Mode::Idle => 0.08,
    };
    // The thinking sweep: an angle where dots burn brighter.
    let sweep = t * 3.2;
    for ring in 0..RINGS {
        let rf = ring as f32;
        let base_r = 20.0 + rf * 13.0;
        let n = 10 + ring * 7;
        let dir = if ring % 2 == 0 { 1.0 } else { -1.0 };
        let rot = t * speed * dir * (1.0 - rf * 0.07) + rf * 0.37;
        // Listening and speaking push the rings outward, the inner ones most.
        let swell =
            1.0 + level * 0.16 * (1.0 - rf / RINGS as f32) * if matches!(mode, Mode::Listening | Mode::Speaking) { 1.0 } else { 0.3 };
        for k in 0..n {
            let a = rot + k as f32 * TAU / n as f32;
            let wave = ((a * 3.0 - t * 1.7 + rf * 0.9).sin() * 0.5 + 0.5) * e;
            let burn = if matches!(mode, Mode::Thinking | Mode::Transcribing) {
                let d = ((a - sweep).rem_euclid(TAU) - std::f32::consts::PI).abs() / std::f32::consts::PI;
                d.powf(6.0)
            } else {
                0.0
            };
            let wobble = 1.0 + 0.025 * ((t * 5.0 + k as f32 * 0.8 + rf).sin()) * e;
            let r = base_r * swell * wobble;
            let p = [c[0] + a.cos() * r, c[1] + a.sin() * r];
            let bright = (0.38 + 0.6 * wave + 0.6 * burn).min(1.0);
            let size = (1.3 + 1.5 * bright) * (1.0 - rf * 0.06);
            let color = if burn > 0.5 {
                core()
            } else if ring >= RINGS - 2 {
                ember()
            } else {
                brass()
            };
            s = s.fill(circle(p, size, 10), color.alpha(bright));
        }
    }

    // Core: a bright heart that breathes.
    let breath = 0.5 + 0.5 * (t * 1.6).sin();
    let core_r = 9.0 + 4.0 * e + 2.0 * breath * (0.4 + level);
    s = s.fill(circle(c, core_r * 1.8, 40), brass().alpha(0.25 + 0.2 * e));
    s = s.fill(circle(c, core_r, 40), core().alpha(0.85 + 0.15 * e));
    s.view_box(VIEW, VIEW)
}

/// A small microphone glyph in a 16 × 16 box.
pub fn mic(color: Color) -> Shape {
    let mut s = Shape::new();
    s = s.line([8.0, 3.5], [8.0, 8.5], 5.0, rune::LineCap::Round, color);
    let arc: Vec<[f32; 2]> = (0..=12)
        .map(|i| {
            let a = std::f32::consts::PI * i as f32 / 12.0;
            [8.0 - a.cos() * 5.0, 8.0 + a.sin() * 4.5]
        })
        .collect();
    s = s.stroke(arc, 1.4, color);
    s = s.line([8.0, 12.5], [8.0, 14.5], 1.4, rune::LineCap::Round, color);
    s.view_box(16.0, 16.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn energy_grows_with_mode_and_level() {
        assert!(energy(Mode::Idle, 0.0) < energy(Mode::Thinking, 0.0));
        assert!(energy(Mode::Listening, 0.9) > energy(Mode::Listening, 0.1));
        assert!(energy(Mode::Speaking, 5.0) <= 1.0);
    }

    #[test]
    fn circles_have_their_points_on_the_radius() {
        for p in circle([10.0, 10.0], 5.0, 12) {
            let d = ((p[0] - 10.0).powi(2) + (p[1] - 10.0).powi(2)).sqrt();
            assert!((d - 5.0).abs() < 1e-4);
        }
    }
}
