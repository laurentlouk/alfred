//! How lively the orb is, and the mic icon. The orb itself is drawn on the GPU (`gfx`): its liquid moves
//! faster, its glow brightens and its outline wobbles more as `energy` rises.

use rune::{Color, Shape};

use super::Mode;

/// How lively the orb is in a mode, 0 to 1, with the voice level (the microphone or Alfred's speech) added.
pub fn energy(mode: Mode, level: f32) -> f32 {
    let base = match mode {
        Mode::Idle => 0.18,
        Mode::Listening => 0.45,
        Mode::Transcribing | Mode::Thinking => 0.62,
        Mode::Speaking => 0.40,
    };
    (base + level * 0.6).min(1.0)
}

/// The orb's size at time `t`, 1 at rest: it breathes slowly when idle, quickly while thinking, swells with
/// your voice while listening and pulses with Alfred's while it replies.
pub fn swell(mode: Mode, level: f32, t: f32) -> f32 {
    match mode {
        Mode::Idle => 1.0 + 0.015 * (t * 1.1).sin(),
        Mode::Listening => 1.0 + 0.07 * level,
        Mode::Transcribing | Mode::Thinking => 1.0 + 0.035 * (t * 4.5).sin(),
        Mode::Speaking => 1.0 + (MAX_SWELL - 1.0) * level.clamp(0.0, 1.0),
    }
}

/// How far the liquid has turned at time `t`, radians: a slow drift, a visible swirl while thinking.
pub fn swirl(mode: Mode, t: f32) -> f32 {
    match mode {
        Mode::Transcribing | Mode::Thinking => t * 2.2,
        Mode::Speaking => t * 0.5,
        Mode::Idle | Mode::Listening => t * 0.12,
    }
}

/// The mode as the compositor reads it from `effect_data[2]`.
pub fn mode_code(mode: Mode) -> f32 {
    match mode {
        Mode::Idle => 0.0,
        Mode::Listening => super::gfx::LISTENING,
        Mode::Transcribing => 2.0,
        Mode::Thinking => super::gfx::THINKING,
        Mode::Speaking => super::gfx::SPEAKING,
    }
}

/// The most the orb grows (with Alfred's voice at its loudest).
pub const MAX_SWELL: f32 = 1.11;

/// A microphone with a slash through it: listening is off.
pub fn mic_off(color: Color) -> Shape {
    mic(color).line([2.5, 2.5], [13.5, 13.5], 1.6, rune::LineCap::Round, color)
}

/// A microphone glyph in a 16 × 16 view box.
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
        assert_eq!(mode_code(Mode::Thinking), super::super::gfx::THINKING);
    }

    #[test]
    fn the_orb_moves_more_when_busy() {
        let range = |m: Mode| {
            let sizes: Vec<f32> = (0..60).map(|i| swell(m, 0.0, i as f32 * 0.05)).collect();
            sizes.iter().cloned().fold(f32::MIN, f32::max) - sizes.iter().cloned().fold(f32::MAX, f32::min)
        };
        assert!(range(Mode::Thinking) > 2.0 * range(Mode::Idle), "thinking breathes visibly");
        assert!(swell(Mode::Speaking, 0.8, 0.0) > swell(Mode::Speaking, 0.1, 0.0), "replying pulses with the voice");
        assert!(swirl(Mode::Thinking, 1.0) > 10.0 * swirl(Mode::Idle, 1.0), "thinking swirls the liquid");
    }
}
