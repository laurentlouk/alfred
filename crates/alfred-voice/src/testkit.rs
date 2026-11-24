//! Shared helpers for the tests that use the real models: real speech from Kokoro at 16 kHz.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use crate::models::{self, ModelSpec};
use crate::{Speaker, capture};

/// The models folder: `ALFRED_MODELS`, else Alfred's own.
pub fn models() -> PathBuf {
    std::env::var_os("ALFRED_MODELS").map(Into::into).unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        home.join("Library/Application Support/Alfred/models")
    })
}

/// The models folder when every model in `specs` (and Kokoro, which makes the test audio) is there.
/// Prints why a test skips itself otherwise.
pub fn ready(specs: &[ModelSpec]) -> Option<PathBuf> {
    let m = models();
    let missing: Vec<_> = specs.iter().chain([&models::KOKORO]).filter(|s| !s.is_ready(&m)).map(|s| s.label).collect();
    if missing.is_empty() {
        Some(m)
    } else {
        eprintln!("skipping: {} not in {}", missing.join(", "), m.display());
        None
    }
}

fn speaker() -> &'static Mutex<Speaker> {
    static SPEAKER: OnceLock<Mutex<Speaker>> = OnceLock::new();
    SPEAKER.get_or_init(|| Mutex::new(Speaker::load(&models(), "bm_george", 1.0).expect("Kokoro loads")))
}

/// `text` said by Kokoro's bm_george, resampled to 16 kHz.
pub fn say(text: &str) -> Vec<f32> {
    let audio = speaker().lock().unwrap().synthesize(text).expect("Kokoro speaks");
    capture::resample(&audio.samples, audio.sample_rate, 16_000)
}

pub fn silence(secs: f32) -> Vec<f32> {
    vec![0.0; (secs * 16_000.0) as usize]
}

/// Seconds of 16 kHz audio.
pub fn secs(samples: usize) -> f32 {
    samples as f32 / 16_000.0
}

/// Seeded white noise, uniform in [-1, 1).
pub fn white(n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f32 / 8_388_608.0 - 1.0
        })
        .collect()
}

/// Seeded pink noise (falling 3 dB per octave, like a fan or distant traffic), by Paul Kellet's filter.
pub fn pink(n: usize, seed: u32) -> Vec<f32> {
    let mut b = [0.0f32; 7];
    white(n, seed)
        .into_iter()
        .map(|w| {
            b[0] = 0.99886 * b[0] + w * 0.055_517_9;
            b[1] = 0.99332 * b[1] + w * 0.075_075_9;
            b[2] = 0.969 * b[2] + w * 0.153_852;
            b[3] = 0.8665 * b[3] + w * 0.310_485_6;
            b[4] = 0.55 * b[4] + w * 0.532_952_2;
            b[5] = -0.7616 * b[5] - w * 0.016_898;
            let p = b.iter().sum::<f32>() + w * 0.5362;
            b[6] = w * 0.115_926;
            p
        })
        .collect()
}

/// The RMS of the louder half of the 10 ms frames: the level of the speech itself, not its pauses.
pub fn speech_rms(speech: &[f32]) -> f32 {
    let mut e: Vec<f32> = speech.chunks(160).map(|c| c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).collect();
    e.sort_by(|a, b| b.total_cmp(a));
    let loud = &e[..e.len().div_ceil(2)];
    (loud.iter().sum::<f32>() / loud.len() as f32).sqrt()
}

/// `noise` scaled to `db_below` dB under the level of `speech`, then added to `clip` sample by sample.
pub fn mix_noise(clip: &[f32], speech: &[f32], noise: &[f32], db_below: f32) -> Vec<f32> {
    let n_rms = (noise.iter().map(|s| s * s).sum::<f32>() / noise.len().max(1) as f32).sqrt().max(1e-9);
    let gain = speech_rms(speech) * 10f32.powf(-db_below / 20.0) / n_rms;
    clip.iter().zip(noise.iter().cycle()).map(|(c, n)| c + n * gain).collect()
}
