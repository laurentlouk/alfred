//! Telling the user's voice from the room while listening, from the microphone's level alone.
//!
//! The gate learns the room's noise floor in the first moments (before anyone speaks) and keeps following it
//! while no one does. Sound counts as voice only when it stays clearly above that floor for a moment, so a
//! fan, traffic or music neither starts a command nor keeps one open: when the user stops talking, the
//! level falls back to the floor and the silence clock runs even though the noise goes on.

/// Seconds spent learning the room when listening starts.
const CALIBRATE: f64 = 0.35;
/// Seconds the level must stay above the threshold to count as speech (a door, a cough don't).
const HOLD: f64 = 0.25;
/// The lowest threshold, for a silent room (0 to 1 on the meter).
const MIN_THRESHOLD: f32 = 0.24;
/// With no speech this long after listening starts, there is nothing to wait for.
pub const NO_SPEECH: f64 = 5.0;

#[derive(Debug, Clone, PartialEq)]
pub struct NoiseGate {
    started: f64,
    floor: f32,
    above_since: Option<f64>,
}

impl NoiseGate {
    pub fn new(t: f64) -> Self {
        Self { started: t, floor: 0.0, above_since: None }
    }

    /// The level voice must reach over this room's noise.
    pub fn threshold(&self) -> f32 {
        MIN_THRESHOLD.max(self.floor + 0.2).max(self.floor * 2.0)
    }

    /// Feed one meter reading at time `t`; true while the user is speaking.
    pub fn feed(&mut self, t: f64, level: f32) -> bool {
        if t - self.started < CALIBRATE {
            // Learning the room: the loudest steady level heard before anyone speaks.
            self.floor = self.floor.max(level * 0.9);
            return false;
        }
        if level > self.threshold() {
            let since = *self.above_since.get_or_insert(t);
            return t - since >= HOLD;
        }
        self.above_since = None;
        // Quiet: follow the room slowly, so noise that grows or fades moves the floor with it.
        self.floor += (level - self.floor) * 0.03;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed `level` at 60 Hz from `from` to `to`; how many readings counted as voice.
    fn run(g: &mut NoiseGate, from: f64, to: f64, level: impl Fn(f64) -> f32) -> usize {
        let mut voiced = 0;
        let mut t = from;
        while t < to {
            voiced += g.feed(t, level(t)) as usize;
            t += 1.0 / 60.0;
        }
        voiced
    }

    #[test]
    fn steady_background_noise_is_not_voice() {
        let mut g = NoiseGate::new(0.0);
        let fan = |t: f64| 0.30 + 0.03 * ((t * 13.0).sin() as f32);
        assert_eq!(run(&mut g, 0.0, 3.0, fan), 0, "a fan never counts as speech");
        assert!(g.threshold() > 0.4, "the threshold rose above the fan: {}", g.threshold());
    }

    #[test]
    fn a_voice_over_noise_counts_and_silence_returns_when_it_stops() {
        let mut g = NoiseGate::new(0.0);
        run(&mut g, 0.0, 0.5, |_| 0.30);
        assert!(run(&mut g, 0.5, 2.0, |_| 0.75) > 60, "speaking over the noise is voice");
        assert_eq!(run(&mut g, 2.0, 4.0, |_| 0.30), 0, "the noise alone after the sentence is silence");
    }

    #[test]
    fn a_short_bang_is_not_voice() {
        let mut g = NoiseGate::new(0.0);
        run(&mut g, 0.0, 0.5, |_| 0.05);
        let bang = |t: f64| if (0.6..0.7).contains(&t) { 0.9 } else { 0.05 };
        assert_eq!(run(&mut g, 0.5, 1.5, bang), 0);
    }

    #[test]
    fn a_quiet_room_keeps_the_lowest_threshold() {
        let mut g = NoiseGate::new(0.0);
        run(&mut g, 0.0, 1.0, |_| 0.02);
        assert!((g.threshold() - MIN_THRESHOLD).abs() < 1e-6);
    }
}
