//! Voice activity detection with Silero VAD: is the speaker talking right now, and for how long have
//! they been quiet.
//!
//! Silero tells speech from non-speech, so steady background noise reads as silence. Two level gates
//! then keep only the speaker's own voice, each measured on the loudest window of the last quarter
//! second so the gaps between syllables don't break the speech up:
//! - the room: speech must stand clear of the noise floor heard between words. That floor also raises
//!   Silero's threshold in a noisy room.
//! - the speaker: once their level is known (from the wake word, or the first words of the command), a
//!   voice much quieter than theirs, someone further away, isn't their speech.
//!
//! Loudness never ends speech by itself: it only keeps sounds that aren't the speaker from counting.

use std::collections::VecDeque;
use std::path::Path;

use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};

use crate::models;

/// The rate every part of the listening pipeline works at.
pub const SAMPLE_RATE: u32 = 16_000;
/// Silero's window at 16 kHz: 32 ms.
const WINDOW: usize = 512;
/// How much Silero's threshold rises from a quiet room to a loud one.
const NOISY_THRESHOLD_RISE: f32 = 0.15;
/// Noise floors, in dBFS, between which the threshold rises.
const QUIET_ROOM_DB: f32 = -60.0;
const LOUD_ROOM_DB: f32 = -35.0;
/// The floor assumed before any sound is heard.
const START_FLOOR_DB: f32 = -70.0;
/// The threshold changes at most this often, since each change rebuilds the detector.
const RETUNE_EVERY_SECS: f32 = 1.0;
/// Silero's own minimum speech: a short one, since it asks for that long without a single window under
/// the threshold, and the dips between syllables would hide whole words at 0.25 s and a 0.6 threshold.
/// [`VadConfig::min_speech_secs`] is counted over the speech as a whole instead.
const SILERO_MIN_SPEECH_SECS: f32 = 0.1;
/// Speech counts as over after this much silence. Short, so the end of a sentence is seen quickly; the
/// listener decides how much more silence ends a command.
const MIN_SILENCE_SECS: f32 = 0.25;
/// The gates look at the loudest window over this many: a quarter second.
const HOLD_WINDOWS: usize = 8;
/// The speaker's level sinks this much per second when they get quieter, so one loud word doesn't
/// set it for the whole command.
const VOICE_DECAY_DB_PER_SEC: f32 = 1.0;

/// How strict the VAD is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VadConfig {
    /// Silero's threshold in a quiet room; it rises by up to 0.15 as the noise floor climbs.
    pub threshold: f32,
    /// The first speech must last this long, in one stretch, before it counts, so a click, a cough or
    /// a word from across the room doesn't start a command.
    pub min_speech_secs: f32,
    /// Speech must be this many dB above the room's noise floor.
    pub above_noise_db: f32,
    /// Once the speaker's level is known, a voice this many dB quieter isn't theirs.
    pub below_voice_db: f32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self { threshold: 0.6, min_speech_secs: 0.25, above_noise_db: 12.0, below_voice_db: 15.0 }
    }
}

/// Silero VAD at 16 kHz, fed with chunks of any length.
pub struct Vad {
    cfg: VadConfig,
    detector: VoiceActivityDetector,
    model: String,
    threshold: f32,
    /// The level of the room between words, in dBFS: a running average over the windows Silero hears
    /// as non-speech, which falls quickly and rises slowly.
    floor_db: f32,
    /// The speaker's level, in dBFS, once known.
    voice_db: Option<f32>,
    /// The levels of the last [`HOLD_WINDOWS`] windows.
    recent_db: VecDeque<f32>,
    /// Where the threshold last changed.
    tuned_at: u64,
    /// Windows heard as silence on purpose (see [`Vad::skip`]) leave the room's floor alone.
    muted: bool,
    /// Samples waiting for a full window.
    pending: Vec<f32>,
    /// Samples the model has seen since the last reset.
    fed: u64,
    speaking: bool,
    /// Where the current stretch of speech began, before it counts, and how much of it was loud.
    run_start: Option<u64>,
    run_loud: u64,
    /// Where speech was first and last heard since the last reset, in samples.
    first: Option<u64>,
    last: Option<u64>,
}

impl Vad {
    pub fn load(models: &Path, cfg: VadConfig) -> Result<Self, String> {
        let dir = models::SILERO_VAD.dir(models);
        if !models::SILERO_VAD.is_ready(models) {
            return Err(format!("the voice activity model isn't downloaded yet (run `alfred setup`); looked in {}", dir.display()));
        }
        let model = dir.join(models::SILERO_VAD.required[0]).to_string_lossy().into_owned();
        let threshold = threshold_for(cfg.threshold, START_FLOOR_DB);
        Ok(Self {
            cfg,
            detector: detector(&model, threshold)?,
            model,
            threshold,
            floor_db: START_FLOOR_DB,
            voice_db: None,
            recent_db: VecDeque::with_capacity(HOLD_WINDOWS),
            tuned_at: 0,
            muted: false,
            pending: Vec::with_capacity(WINDOW * 4),
            fed: 0,
            speaking: false,
            run_start: None,
            run_loud: 0,
            first: None,
            last: None,
        })
    }

    /// Feed 16 kHz mono samples, any number at a time.
    pub fn accept(&mut self, samples: &[f32]) {
        self.pending.extend_from_slice(samples);
        let mut at = 0;
        while self.pending.len() - at >= WINDOW {
            let window = &self.pending[at..at + WINDOW];
            self.detector.accept_waveform(window);
            let db = 10.0 * (window.iter().map(|s| s * s).sum::<f32>() / WINDOW as f32).max(1e-12).log10();
            at += WINDOW;
            self.fed += WINDOW as u64;
            if self.recent_db.len() == HOLD_WINDOWS {
                self.recent_db.pop_front();
            }
            self.recent_db.push_back(db);
            let held = self.recent_db.iter().copied().fold(f32::MIN, f32::max);
            let silero = self.detector.detected();
            let voiced = silero && self.is_voice(held);
            if voiced {
                // Silero confirms speech SILERO_MIN_SPEECH_SECS after it starts: that much is credited.
                if self.run_start.is_none() {
                    self.run_start = Some(self.fed.saturating_sub(secs_to_samples(SILERO_MIN_SPEECH_SECS) + 2 * WINDOW as u64));
                    self.run_loud = secs_to_samples(SILERO_MIN_SPEECH_SECS);
                }
                // Only windows loud enough in themselves count, not Silero's or the gates' hold after a sound.
                if self.is_voice(db) {
                    self.run_loud += WINDOW as u64;
                }
            } else {
                self.run_start = None;
            }
            // The first stretch of a command's speech must last `min_speech_secs`; later ones count at once.
            let now = voiced && (self.first.is_some() || self.run_loud >= secs_to_samples(self.cfg.min_speech_secs));
            if now {
                self.first = self.first.or(self.run_start);
                self.last = Some(self.fed);
                let decayed = self.voice_db.map(|v| v - VOICE_DECAY_DB_PER_SEC * WINDOW as f32 / SAMPLE_RATE as f32);
                self.voice_db = Some(decayed.map_or(held, |v| v.max(held)));
            } else if self.speaking {
                // Speech ends MIN_SILENCE_SECS (Silero) or a hold (the gates) after its last window.
                self.last = Some(self.fed.saturating_sub(secs_to_samples(MIN_SILENCE_SECS)));
            }
            if !silero && !self.muted {
                // Only what Silero hears as non-speech is the room; a voice further away isn't.
                self.floor_db = follow_floor(self.floor_db, db);
            }
            self.speaking = now;
            // The detector queues each finished segment's samples; nothing here needs them.
            while !self.detector.is_empty() {
                self.detector.pop();
            }
            if !silero && !self.muted {
                self.retune();
            }
        }
        self.pending.drain(..at);
    }

    /// Whether a window whose quarter-second peak is `held_db` is loud enough to be the speaker.
    fn is_voice(&self, held_db: f32) -> bool {
        held_db >= self.floor_db + self.cfg.above_noise_db && self.voice_db.is_none_or(|v| held_db >= v - self.cfg.below_voice_db)
    }

    /// Hear the next `n` samples as silence, whatever they hold (the ring of the wake word), without
    /// taking them for the room's noise.
    pub fn skip(&mut self, n: usize) {
        self.muted = true;
        self.accept(&vec![0.0; n]);
        self.muted = false;
    }

    /// The speaker's level, when it is known before they speak: the wake word they just said.
    pub fn expect_voice(&mut self, db: f32) {
        self.voice_db = Some(db);
    }

    /// Between words, move the threshold to the room's noise floor. Rebuilding the detector drops its
    /// memory of the last half second, which only matters inside speech.
    fn retune(&mut self) {
        let want = threshold_for(self.cfg.threshold, self.floor_db);
        if (want - self.threshold).abs() < 0.01 || self.fed.saturating_sub(self.tuned_at) < secs_to_samples(RETUNE_EVERY_SECS) {
            return;
        }
        if let Ok(d) = detector(&self.model, want) {
            self.detector = d;
            self.threshold = want;
            self.tuned_at = self.fed;
        }
    }

    /// The room's noise floor between words, in dBFS.
    pub fn noise_floor_db(&self) -> f32 {
        self.floor_db
    }

    /// Silero's threshold now.
    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    /// The speaker's level, in dBFS, once known.
    pub fn voice_db(&self) -> Option<f32> {
        self.voice_db
    }

    /// Whether someone is speaking now.
    pub fn is_speech(&self) -> bool {
        self.speaking
    }

    /// Whether any speech was heard since the last reset.
    pub fn heard_speech(&self) -> bool {
        self.first.is_some()
    }

    /// Seconds since speech was last heard (since the reset if none was): 0 while someone speaks.
    pub fn silence_secs(&self) -> f32 {
        if self.speaking {
            return 0.0;
        }
        self.fed.saturating_sub(self.last.unwrap_or(0)) as f32 / SAMPLE_RATE as f32
    }

    /// Where speech started and ended since the last reset, in samples from the reset; the end is
    /// where the model is now while speech goes on.
    pub fn speech_span(&self) -> Option<(u64, u64)> {
        Some((self.first?, self.last?))
    }

    /// Samples seen since the last reset.
    pub fn position(&self) -> u64 {
        self.fed + self.pending.len() as u64
    }

    /// Forget the speech heard so far (it was a cough, or the end of the wake word) while keeping
    /// positions and listening on.
    pub fn forget_speech(&mut self) {
        self.run_start = None;
        self.first = None;
        self.last = None;
        self.voice_db = None;
    }

    /// Forget everything heard, the speaker included; positions start again from 0. The room's noise
    /// floor is kept.
    pub fn reset(&mut self) {
        self.detector.reset();
        self.pending.clear();
        self.recent_db.clear();
        self.voice_db = None;
        self.fed = 0;
        self.tuned_at = 0;
        self.speaking = false;
        self.run_start = None;
        self.first = None;
        self.last = None;
    }
}

/// The loudest 32 ms window of `samples`, in dBFS: the level a speaker reaches, measured as the VAD's
/// gates measure it.
pub fn peak_db(samples: &[f32]) -> f32 {
    samples.chunks(WINDOW).map(|w| 10.0 * (w.iter().map(|s| s * s).sum::<f32>() / w.len() as f32).max(1e-12).log10()).fold(-120.0, f32::max)
}

fn detector(model: &str, threshold: f32) -> Result<VoiceActivityDetector, String> {
    let config = VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: Some(model.to_string()),
            threshold,
            min_silence_duration: MIN_SILENCE_SECS,
            min_speech_duration: SILERO_MIN_SPEECH_SECS,
            window_size: WINDOW as i32,
            // Longer speech is split into segments, which this wrapper drops anyway; detection goes on.
            max_speech_duration: 30.0,
        },
        sample_rate: SAMPLE_RATE as i32,
        num_threads: 1,
        debug: false,
        ..Default::default()
    };
    VoiceActivityDetector::create(&config, 30.0).ok_or_else(|| "the voice activity model failed to load".to_string())
}

/// The noise floor after a non-speech window at `db`: down fast (a door closed), up slowly (about 1.5 s
/// to follow a fan that starts), never below -90 dBFS (digital silence).
fn follow_floor(floor: f32, db: f32) -> f32 {
    let db = db.max(-90.0);
    let rate = if db < floor { 0.3 } else { 0.02 };
    floor + rate * (db - floor)
}

/// Silero's threshold for a room whose noise floor is `floor_db`, from `quiet` up by at most
/// [`NOISY_THRESHOLD_RISE`], in steps of 0.05.
fn threshold_for(quiet: f32, floor_db: f32) -> f32 {
    let x = ((floor_db - QUIET_ROOM_DB) / (LOUD_ROOM_DB - QUIET_ROOM_DB)).clamp(0.0, 1.0);
    ((quiet + x * NOISY_THRESHOLD_RISE) * 20.0).round() / 20.0
}

fn secs_to_samples(secs: f32) -> u64 {
    (secs * SAMPLE_RATE as f32) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;

    /// Feeds `samples` in uneven chunks, as a microphone would.
    fn feed(vad: &mut Vad, samples: &[f32]) -> bool {
        let mut heard = false;
        let mut at = 0;
        for size in [100, 333, 1024, 7, 2000, 512].iter().cycle() {
            if at >= samples.len() {
                break;
            }
            let end = (at + size).min(samples.len());
            vad.accept(&samples[at..end]);
            heard |= vad.is_speech();
            at = end;
        }
        heard
    }

    #[test]
    fn the_threshold_rises_with_the_noise_floor() {
        assert_eq!(threshold_for(0.6, -80.0), 0.6);
        assert_eq!(threshold_for(0.6, -60.0), 0.6);
        assert_eq!(threshold_for(0.6, -50.0), 0.65);
        assert_eq!(threshold_for(0.6, -35.0), 0.75);
        assert_eq!(threshold_for(0.6, -10.0), 0.75);
        assert_eq!(threshold_for(0.5, -35.0), 0.65);
        // The floor drops at once when the room goes quiet and climbs slowly when it gets loud.
        assert!((follow_floor(-40.0, -70.0) - -49.0).abs() < 1e-4);
        assert!((follow_floor(-70.0, -40.0) - -69.4).abs() < 1e-4);
        assert_eq!(follow_floor(-89.0, -200.0), -89.0 - 0.3);
    }

    #[test]
    fn the_noise_floor_follows_the_room() {
        let Some(m) = ready(&[models::SILERO_VAD]) else { return };
        let mut vad = Vad::load(&m, VadConfig::default()).unwrap();
        assert_eq!(vad.threshold(), 0.6);
        // A fan at -35 dBFS: no speech, and Silero must be surer of it.
        let fan: Vec<f32> = pink(16_000 * 5, 4);
        let rms = (fan.iter().map(|s| s * s).sum::<f32>() / fan.len() as f32).sqrt();
        let fan: Vec<f32> = fan.iter().map(|s| s * 10f32.powf(-35.0 / 20.0) / rms).collect();
        assert!(!feed(&mut vad, &fan));
        assert!((vad.noise_floor_db() - -35.0).abs() < 3.0, "{} dB", vad.noise_floor_db());
        assert_eq!(vad.threshold(), 0.75);
        // A reset keeps the room; the fan stopping brings the threshold back down.
        vad.reset();
        assert_eq!(vad.threshold(), 0.75);
        assert!(!feed(&mut vad, &silence(2.0)));
        assert!(vad.noise_floor_db() < -80.0, "{} dB", vad.noise_floor_db());
        assert_eq!(vad.threshold(), 0.6);
    }

    #[test]
    fn a_syllable_does_not_start_speech() {
        let Some(m) = ready(&[models::SILERO_VAD]) else { return };
        let mut vad = Vad::load(&m, VadConfig::default()).unwrap();
        let speech = say("Open Notes.");
        let onset = speech.iter().position(|s| s.abs() > 0.02).unwrap();
        let mut blip = silence(0.5);
        blip.extend(&speech[onset..onset + 2_900]);
        blip.extend(silence(1.0));
        assert!(!feed(&mut vad, &blip), "0.18 s of sound");
        vad.reset();
        let mut clip = silence(0.5);
        clip.extend(&speech);
        assert!(feed(&mut vad, &clip), "a whole command");
    }

    #[test]
    fn only_the_speakers_voice_counts() {
        let Some(m) = ready(&[models::SILERO_VAD]) else { return };
        let mut vad = Vad::load(&m, VadConfig::default()).unwrap();
        let user = say("Open Notes and save buy milk.");
        let far = {
            let sp = crate::Speaker::load(&m, "af_heart", 1.0).unwrap();
            let a = sp.synthesize("The game starts at eight tonight, so we should leave early.").unwrap();
            crate::capture::resample(&a.samples, a.sample_rate, 16_000)
        };
        // Someone across the room, 25 dB under the speaker.
        let gain = speech_rms(&user) / speech_rms(&far) * 10f32.powf(-25.0 / 20.0);
        let far: Vec<f32> = far.iter().map(|s| s * gain).collect();

        // The speaker, then the far voice: the speech ends with the speaker's.
        let mut clip = silence(0.5);
        clip.extend(&user);
        let user_ends = clip.len();
        clip.extend(&far);
        feed(&mut vad, &clip);
        let (_, end) = vad.speech_span().unwrap();
        assert!(
            secs(end as usize) < secs(user_ends) + 0.1,
            "speech ran to {} s, the speaker stopped at {} s",
            secs(end as usize),
            secs(user_ends)
        );
        assert!(!vad.is_speech() && vad.silence_secs() > 1.5);

        // Knowing the speaker's level from the wake word, the far voice alone starts nothing.
        vad.reset();
        vad.expect_voice(peak_db(&user));
        let mut clip = silence(0.5);
        clip.extend(&far);
        assert!(!feed(&mut vad, &clip));

        // Nor does steady noise, however long.
        vad.reset();
        let fan: Vec<f32> = pink(16_000 * 4, 8).iter().map(|s| s * 0.02).collect();
        assert!(!feed(&mut vad, &fan));
    }

    #[test]
    fn speech_is_heard_and_silence_is_not() {
        let Some(m) = ready(&[models::SILERO_VAD]) else { return };
        let mut vad = Vad::load(&m, VadConfig::default()).unwrap();

        // Silence, then quiet hiss: nobody speaks.
        assert!(!feed(&mut vad, &silence(2.0)));
        let mut seed = 7u32;
        let hiss: Vec<f32> = (0..32_000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 8) as f32 / 16_777_216.0 - 0.5) * 0.004
            })
            .collect();
        assert!(!feed(&mut vad, &hiss));
        assert!(!vad.heard_speech());
        assert!((vad.silence_secs() - 4.0).abs() < 0.05, "{}", vad.silence_secs());

        // A sentence: speech, found where it is.
        vad.reset();
        let speech = say("Open Notes and save buy milk.");
        let mut clip = silence(1.0);
        clip.extend(&speech);
        assert!(feed(&mut vad, &clip));
        let (start, _) = vad.speech_span().unwrap();
        assert!((secs(start as usize) - 1.0).abs() < 0.25, "speech starts at {}", secs(start as usize));

        // Then quiet again: speech ends, and the silence is counted from the end of the words.
        feed(&mut vad, &silence(1.0));
        assert!(!vad.is_speech());
        let (_, end) = vad.speech_span().unwrap();
        let spoken_until = 1.0 + secs(speech.len());
        assert!((secs(end as usize) - spoken_until).abs() < 0.3, "speech ends at {} for {spoken_until}", secs(end as usize));
        assert!((vad.silence_secs() - (secs(clip.len()) + 1.0 - secs(end as usize))).abs() < 0.05);
    }
}
