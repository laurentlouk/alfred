//! The wake word "Alfred", heard by a streaming keyword spotter.
//!
//! The spotter matches a sequence of the model's own BPE tokens. `tokens.txt` has no `▁AL` and no
//! `RED`, so a line such as `▁AL F RED` could never match. The model's `bpe.model` (a unigram
//! SentencePiece model) segments "ALFRED" as `▁A L F RE D`, its best-scoring path; the same Viterbi
//! segmentation reproduces every line of the model's own `keywords.txt` (`▁HE Y ▁S I RI`, `▁A LE X A`).
//! "Hey Alfred" ends with the same tokens, so it is heard without a line of its own.

use std::path::Path;

use sherpa_onnx::{KeywordSpotter, KeywordSpotterConfig, OnlineStream};

use crate::models;
use crate::vad::SAMPLE_RATE;

/// The keyword list handed to the spotter.
pub const KEYWORDS: &str = "▁A L F RE D @ALFRED";

/// Hears every test phrase across five Kokoro voices with a margin: the first miss comes at 0.35. From
/// 0.05 to 0.3 the threshold changes no false hit; those come from near-homophones ("Elf red").
pub const DEFAULT_THRESHOLD: f32 = 0.25;

/// The spotter fires after this many 40 ms frames without a new token after `D`. Two, against one,
/// cuts hits on "Alfredo" from 22 to 14 in 24 and still hears 200 of 200 isolated "Alfred"s (five
/// voices, eight alignments); three starts to miss "Alfred what time is it" said in one breath.
const TRAILING_BLANKS: i32 = 2;

/// One detection.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// The keyword's name in [`KEYWORDS`].
    pub keyword: String,
    /// Where the keyword ends and the command after it begins, in samples since the spotter was loaded.
    pub end: u64,
    /// Where the spotter was when it fired, in samples since it was loaded.
    pub at: u64,
}

/// The spotter stamps the last token about where the next word begins. Transcribing the command from
/// 0.04 to 0.07 s before that stamp gave the cleanest text over 33 short commands in three voices:
/// later cuts lose the command's first sound ("Alfred, call mom" heard as "Come on"), earlier ones
/// let the "d" through ("And call mom").
const COMMAND_STARTS_BEFORE_LAST_TOKEN_SECS: f32 = 0.05;

/// A streaming keyword spotter for "Alfred" on 16 kHz audio.
pub struct WakeWord {
    kws: KeywordSpotter,
    stream: OnlineStream,
    /// Samples accepted since load.
    fed: u64,
    /// Where the current stream started: its timestamps count from here.
    origin: u64,
}

impl WakeWord {
    /// Load the spotter. A lower `threshold` hears the word more easily; see [`DEFAULT_THRESHOLD`].
    pub fn load(models: &Path, threshold: f32) -> Result<Self, String> {
        let dir = models::KWS.dir(models);
        if !models::KWS.is_ready(models) {
            return Err(format!("the wake word model isn't downloaded yet (run `alfred setup`); looked in {}", dir.display()));
        }
        let p = |f: &str| Some(dir.join(f).to_string_lossy().into_owned());
        let mut config = KeywordSpotterConfig::default();
        config.model_config.transducer.encoder = p(models::KWS.required[0]);
        config.model_config.transducer.decoder = p(models::KWS.required[1]);
        config.model_config.transducer.joiner = p(models::KWS.required[2]);
        config.model_config.tokens = p("tokens.txt");
        config.model_config.num_threads = 1;
        config.model_config.debug = false;
        config.keywords_threshold = threshold.clamp(0.01, 0.99);
        config.num_trailing_blanks = TRAILING_BLANKS;
        config.keywords_buf = Some(KEYWORDS.to_string());
        let kws = KeywordSpotter::create(&config).ok_or("the wake word model failed to load")?;
        let stream = kws.create_stream();
        Ok(Self { kws, stream, fed: 0, origin: 0 })
    }

    /// Feed 16 kHz mono samples, any number at a time. Returns the first hit among them; the spotter
    /// then starts afresh, so one "Alfred" fires once.
    pub fn accept(&mut self, samples: &[f32]) -> Option<Hit> {
        self.stream.accept_waveform(SAMPLE_RATE as i32, samples);
        self.fed += samples.len() as u64;
        while self.kws.is_ready(&self.stream) {
            self.kws.decode(&self.stream);
            let Some(r) = self.kws.get_result(&self.stream) else { continue };
            if r.keyword.is_empty() {
                continue;
            }
            let last = r.timestamps.last().copied().unwrap_or(0.0) - COMMAND_STARTS_BEFORE_LAST_TOKEN_SECS;
            let end = (self.origin + (last.max(0.0) * SAMPLE_RATE as f32) as u64).min(self.fed);
            let hit = Hit { keyword: r.keyword, end, at: self.fed };
            self.reset();
            return Some(hit);
        }
        None
    }

    /// Start afresh: forget the audio heard so far, including any not decoded yet. A fresh stream
    /// rather than `KeywordSpotter::reset`, because after a reset the spotter's timestamps count from
    /// a decoder position it doesn't expose.
    pub fn reset(&mut self) {
        self.kws.reset(&self.stream);
        self.stream = self.kws.create_stream();
        self.origin = self.fed;
    }

    /// Samples accepted since load.
    pub fn position(&self) -> u64 {
        self.fed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;

    /// Every hit in `clip`, fed in 10 ms chunks as the microphone would.
    fn hits(w: &mut WakeWord, clip: &[f32]) -> Vec<Hit> {
        clip.chunks(160).filter_map(|c| w.accept(c)).collect()
    }

    #[test]
    fn alfred_is_heard_and_other_sentences_are_not() {
        let Some(m) = ready(&[models::KWS]) else { return };
        let mut w = WakeWord::load(&m, DEFAULT_THRESHOLD).unwrap();
        let mut clip = silence(1.0);
        let speech = say("Alfred, open Notes and save buy milk.");
        clip.extend(&speech);
        clip.extend(silence(1.0));
        let h = hits(&mut w, &clip);
        assert_eq!(h.len(), 1, "{h:?}");
        assert_eq!(h[0].keyword, "ALFRED");
        // The keyword ends early in the sentence, and the spotter fires soon after.
        let (end, at) = (secs(h[0].end as usize), secs(h[0].at as usize));
        assert!(end > 1.3 && end < 2.1, "keyword ends at {end} s");
        assert!(at > end && at - end < 0.8, "fired at {at} s for a keyword ending at {end} s");

        for other in [
            "Open Notes and save buy milk.",
            "The weather is nice today.",
            "Remind me to buy bread and eggs.",
            "Please call Albert tomorrow.",
            "What time is it in Paris?",
        ] {
            let mut clip = silence(0.5);
            clip.extend(say(other));
            clip.extend(silence(1.0));
            assert_eq!(hits(&mut w, &clip), vec![], "{other:?}");
        }
    }

    #[test]
    fn each_alfred_fires_once() {
        let Some(m) = ready(&[models::KWS]) else { return };
        let mut w = WakeWord::load(&m, DEFAULT_THRESHOLD).unwrap();
        let alfred = say("Alfred, open Notes.");
        let mut clip = silence(0.5);
        for _ in 0..2 {
            clip.extend(&alfred);
            clip.extend(silence(1.5));
        }
        let h = hits(&mut w, &clip);
        assert_eq!(h.len(), 2, "{h:?}");
        assert!(h[1].end > h[0].at, "the second hit is the second word: {h:?}");
        assert_eq!(w.position(), clip.len() as u64);
    }

    /// Run with `cargo test --release -p alfred-voice -- --ignored --nocapture measure`.
    #[test]
    #[ignore]
    fn measure_wake_word() {
        let Some(m) = ready(&[models::KWS]) else { return };
        let mut w = WakeWord::load(&m, DEFAULT_THRESHOLD).unwrap();
        let mut seed = 1u32;
        let hiss: Vec<f32> = (0..16_000 * 60)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 8) as f32 / 16_777_216.0 - 0.5) * 0.01
            })
            .collect();
        let t = std::time::Instant::now();
        hits(&mut w, &hiss);
        eprintln!("wake word: {:.1} ms of CPU per second of audio", t.elapsed().as_secs_f32() * 1000.0 / 60.0);

        // Latency from the end of the word's sound to the hit, over offsets that shift the decoder's
        // 320 ms chunks against the word.
        let alfred = say("Alfred.");
        let e: Vec<f32> = alfred.chunks(160).map(|c| c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).collect();
        let peak = e.iter().copied().fold(0.0, f32::max);
        let sound_ends = (e.iter().rposition(|&v| v > peak * 0.01).unwrap_or(0) + 1) * 160;
        let mut lat = Vec::new();
        for k in 0..16 {
            let lead = silence(0.5 + k as f32 * 0.02);
            let mut clip = lead.clone();
            clip.extend(&alfred);
            clip.extend(silence(1.5));
            w.reset();
            let start = w.position();
            if let Some(h) = hits(&mut w, &clip).first() {
                lat.push(secs((h.at - start) as usize - lead.len() - sound_ends));
            }
        }
        lat.sort_by(f32::total_cmp);
        eprintln!(
            "wake word: {} of 16 heard; latency after the word {:.2}..{:.2} s, median {:.2} s",
            lat.len(),
            lat[0],
            lat[lat.len() - 1],
            lat[lat.len() / 2]
        );
    }
}
