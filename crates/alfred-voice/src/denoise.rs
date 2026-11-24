//! Noise suppression with GTCRN, streaming: steady background noise (a fan, traffic) comes out of the
//! microphone audio before the VAD and Moonshine hear it.
//!
//! Each call yields two streams, sample for sample with the input:
//! - `speech`, fully cleaned, for the VAD and the orb's level;
//! - `words`, the cleaned audio with some of the original mixed back in, for Moonshine. Fully cleaned
//!   audio carries artifacts that cost Moonshine words, while the mix gains them back. On ten Kokoro
//!   sentences (67 words) the mix scored 55 words with white noise 10 dB under the speech (44 raw, 45
//!   cleaned), 59 with pink noise at 10 dB (56 and 56), 53 with pink noise at 5 dB (38 and 34), and the
//!   same 65 as raw on clean speech.

use std::collections::VecDeque;
use std::path::Path;

use sherpa_onnx::{
    OfflineSpeechDenoiserGtcrnModelConfig, OfflineSpeechDenoiserModelConfig, OnlineSpeechDenoiser, OnlineSpeechDenoiserConfig,
};

use crate::models;
use crate::vad::SAMPLE_RATE;

/// How much of the cleaned audio goes into `words`.
const WORDS_CLEAN_SHARE: f32 = 0.7;
/// The model takes about a second to settle on a room: its first second from a cold start carries
/// artifacts that turned "Open Notes" into "Obernotes" in a test. Over that second `words` moves from
/// the original audio to the mix.
const SETTLE_SAMPLES: usize = SAMPLE_RATE as usize;

/// The two cleaned streams for one input chunk; see the module notes.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Denoised {
    pub speech: Vec<f32>,
    pub words: Vec<f32>,
}

/// GTCRN on 16 kHz mono audio, fed in chunks of any length. Its output matches the input sample for
/// sample but comes in 256-sample frames, up to 512 samples (32 ms) behind.
pub struct Denoiser {
    inner: OnlineSpeechDenoiser,
    /// Input not yet matched by output, to mix into `words`.
    raw: VecDeque<f32>,
    /// Output samples still to drop: they belong to warm-up audio.
    skip: usize,
    /// Samples the model has heard since its last reset, up to [`SETTLE_SAMPLES`].
    settled: usize,
}

impl Denoiser {
    pub fn load(models: &Path) -> Result<Self, String> {
        let dir = models::GTCRN.dir(models);
        if !models::GTCRN.is_ready(models) {
            return Err(format!("the noise suppression model isn't downloaded yet (run `alfred setup`); looked in {}", dir.display()));
        }
        let config = OnlineSpeechDenoiserConfig {
            model: OfflineSpeechDenoiserModelConfig {
                gtcrn: OfflineSpeechDenoiserGtcrnModelConfig {
                    model: Some(dir.join(models::GTCRN.required[0]).to_string_lossy().into_owned()),
                },
                num_threads: 1,
                ..Default::default()
            },
        };
        let inner = OnlineSpeechDenoiser::create(&config).ok_or("the noise suppression model failed to load")?;
        if inner.sample_rate() != SAMPLE_RATE as i32 {
            return Err(format!("the noise suppression model runs at {} Hz, not {SAMPLE_RATE}", inner.sample_rate()));
        }
        Ok(Self { inner, raw: VecDeque::new(), skip: 0, settled: 0 })
    }

    /// The cleaned streams for as much of the input as the model has finished.
    pub fn run(&mut self, samples: &[f32]) -> Denoised {
        self.raw.extend(samples);
        let clean = self.inner.run(samples, SAMPLE_RATE as i32).samples;
        let mut out = Denoised { speech: Vec::with_capacity(clean.len()), words: Vec::with_capacity(clean.len()) };
        for c in clean {
            let r = self.raw.pop_front().unwrap_or(0.0);
            self.settled = (self.settled + 1).min(SETTLE_SAMPLES);
            if self.skip > 0 {
                self.skip -= 1;
                continue;
            }
            let share = WORDS_CLEAN_SHARE * self.settled as f32 / SETTLE_SAMPLES as f32;
            out.speech.push(c);
            out.words.push(share * c + (1.0 - share) * r);
        }
        out
    }

    /// Let the model settle on `samples` (the room's sound just before the audio that matters) without
    /// returning them: the next output starts with the sample after them.
    pub fn warm_up(&mut self, samples: &[f32]) {
        self.run(samples);
        self.skip = self.raw.len();
    }

    /// Start afresh, for audio that doesn't follow what came before.
    pub fn reset(&mut self) {
        self.inner.reset();
        self.raw.clear();
        self.skip = 0;
        self.settled = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn noise_goes_and_speech_stays_in_step() {
        let Some(m) = ready(&[models::GTCRN]) else { return };
        let mut d = Denoiser::load(&m).unwrap();
        let speech = say("Open Notes and save buy milk.");
        let mut clip = silence(1.0);
        clip.extend(&speech);
        clip.extend(silence(2.0));
        let noisy = mix_noise(&clip, &speech, &white(clip.len(), 3), 10.0);
        let mut out = Denoised::default();
        for c in noisy.chunks(333) {
            let o = d.run(c);
            out.speech.extend(o.speech);
            out.words.extend(o.words);
        }
        // Less than two frames are still in the model; the rest comes out in step with the input.
        assert!(noisy.len() - out.speech.len() <= 512 && out.speech.len() == out.words.len());
        let lag = (-300i64..300)
            .max_by(|a, b| {
                let corr = |l: i64| -> f32 {
                    (0..out.speech.len() as i64).filter_map(|k| clip.get((k - l) as usize).map(|c| out.speech[k as usize] * c)).sum()
                };
                corr(*a).total_cmp(&corr(*b))
            })
            .unwrap();
        assert_eq!(lag, 0);
        // From a cold start `words` is the input itself, and a second later the mix.
        assert!((out.words[100] - noisy[100]).abs() < 1e-3);
        // Hiss after the speech drops by more than 20 dB (about 35 dB measured; pink noise, by about
        // 13 dB once the model has settled); `words` keeps some of it.
        let tail = 16_000 + speech.len() + 8_000..out.speech.len();
        let (before, cleaned, mixed) = (rms(&noisy[tail.clone()]), rms(&out.speech[tail.clone()]), rms(&out.words[tail]));
        assert!(20.0 * (before / cleaned).log10() > 20.0, "{before} -> {cleaned}");
        assert!(mixed > cleaned && mixed < before, "{mixed}");
    }

    #[test]
    fn warm_up_audio_is_not_returned() {
        let Some(m) = ready(&[models::GTCRN]) else { return };
        let mut d = Denoiser::load(&m).unwrap();
        let noise = pink(16_000, 9);
        d.warm_up(&noise[..8_000]);
        let mut n = 0;
        for c in noise[8_000..].chunks(160) {
            n += d.run(c).speech.len();
        }
        assert!(8_000 - n <= 512, "{n} samples back for 8000 after the warm-up");
    }
}
