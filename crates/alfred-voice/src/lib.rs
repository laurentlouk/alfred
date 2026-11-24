//! Alfred's voice and ears, fully on-device through sherpa-onnx:
//!
//! - [`Speaker`]: Kokoro-82M v1.0 (float32), with phonemes from `misaki-rs` (Kokoro's own G2P, MIT). No espeak-ng,
//!   so no GPL code ends up in the binary.
//! - [`Listener`]: Moonshine tiny for dictation.
//! - [`ears::Ears`]: the listening pipeline on one microphone stream ([`capture::Mic`]): the wake word
//!   "Alfred" ([`wake::WakeWord`]), Silero voice activity ([`vad::Vad`]) for a quick end of speech, and
//!   live text while the user speaks.
//!
//! All of them load from the folder [`models`] downloads to.

pub mod capture;
pub mod denoise;
pub mod ears;
pub mod models;
pub mod text;
pub mod vad;
pub mod wake;

pub use ears::{Ears, EarsConfig, Heard};

#[cfg(test)]
mod testkit;

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use sherpa_onnx::{
    GenerationConfig, OfflineRecognizer, OfflineRecognizerConfig, OfflineTts, OfflineTtsConfig, OfflineTtsKokoroModelConfig,
    OfflineTtsModelConfig, Wave,
};

/// Kokoro v1.0 speaker ids for its English voices (the model's own `speaker_names` metadata).
pub const VOICES: &[(&str, i32)] = &[
    ("af_alloy", 0),
    ("af_aoede", 1),
    ("af_bella", 2),
    ("af_heart", 3),
    ("af_jessica", 4),
    ("af_kore", 5),
    ("af_nicole", 6),
    ("af_nova", 7),
    ("af_river", 8),
    ("af_sarah", 9),
    ("af_sky", 10),
    ("am_adam", 11),
    ("am_echo", 12),
    ("am_eric", 13),
    ("am_fenrir", 14),
    ("am_liam", 15),
    ("am_michael", 16),
    ("am_onyx", 17),
    ("am_puck", 18),
    ("am_santa", 19),
    ("bf_alice", 20),
    ("bf_emma", 21),
    ("bf_isabella", 22),
    ("bf_lily", 23),
    ("bm_daniel", 24),
    ("bm_fable", 25),
    ("bm_george", 26),
    ("bm_lewis", 27),
];

/// The speaker id for a voice name, if it is one of the English voices.
pub fn voice_id(name: &str) -> Option<i32> {
    VOICES.iter().find(|(n, _)| *n == name.trim().to_lowercase()).map(|(_, id)| *id)
}

/// Mono float samples.
#[derive(Debug, Clone, PartialEq)]
pub struct Audio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl Audio {
    pub fn seconds(&self) -> f32 {
        if self.sample_rate == 0 { 0.0 } else { self.samples.len() as f32 / self.sample_rate as f32 }
    }

    /// Loudness, 0 to 1, at `hz` values per second: what the orb ripples with while Alfred speaks.
    pub fn envelope(&self, hz: f32) -> Vec<f32> {
        let step = ((self.sample_rate as f32 / hz.max(1.0)) as usize).max(1);
        let rms: Vec<f32> = self.samples.chunks(step).map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt()).collect();
        let peak = rms.iter().copied().fold(0.0f32, f32::max).max(1e-6);
        rms.into_iter().map(|r| (r / peak).clamp(0.0, 1.0)).collect()
    }

    pub fn write_wav(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let ok = sherpa_onnx::write(&path.to_string_lossy(), &self.samples, self.sample_rate as i32);
        if ok { Ok(()) } else { Err(format!("couldn't write {}", path.display())) }
    }
}

/// Threads for Kokoro: the Mac's performance cores (`hw.perflevel0.physicalcpu`), read once. Elsewhere,
/// or when that can't be read, the available parallelism, 2 to 8.
fn performance_cores() -> i32 {
    static CORES: OnceLock<i32> = OnceLock::new();
    *CORES.get_or_init(|| {
        std::process::Command::new("sysctl")
            .args(["-n", "hw.perflevel0.physicalcpu"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<i32>().ok())
            .filter(|n| *n > 0)
            .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get().clamp(2, 8) as i32).unwrap_or(4))
    })
}

/// Kokoro text-to-speech.
pub struct Speaker {
    tts: OfflineTts,
    g2p: misaki_rs::G2P,
    vocab: HashSet<char>,
    sid: i32,
    speed: f32,
}

impl Speaker {
    /// Load Kokoro from the models folder. `voice` is a name from [`VOICES`]; British voices (`b…`) get
    /// British phonemes.
    pub fn load(models: &Path, voice: &str, speed: f32) -> Result<Self, String> {
        let dir = models::KOKORO.dir(models);
        if !models::KOKORO.is_ready(models) {
            return Err(format!("the Kokoro voice isn't downloaded yet (run `alfred setup`); looked in {}", dir.display()));
        }
        let sid = voice_id(voice).ok_or_else(|| format!("unknown voice {voice:?}; try bm_george, bf_emma, af_heart or am_michael"))?;
        let p = |f: &str| Some(dir.join(f).to_string_lossy().into_owned());
        let config = OfflineTtsConfig {
            model: OfflineTtsModelConfig {
                kokoro: OfflineTtsKokoroModelConfig {
                    model: p(models::KOKORO.required[0]),
                    voices: p("voices.bin"),
                    tokens: p("tokens.txt"),
                    length_scale: 1.0,
                    ..Default::default()
                },
                num_threads: performance_cores(),
                debug: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let tts = OfflineTts::create(&config).ok_or("Kokoro failed to load")?;
        let tokens = std::fs::read_to_string(dir.join("tokens.txt")).map_err(|e| e.to_string())?;
        let vocab = tokens.lines().filter_map(|l| l.chars().next()).collect();
        let british = voice.trim().to_lowercase().starts_with('b');
        let g2p = misaki_rs::G2P::new(if british { misaki_rs::Language::EnglishGB } else { misaki_rs::Language::EnglishUS });
        Ok(Self { tts, g2p, vocab, sid, speed: speed.clamp(0.5, 2.0) })
    }

    /// Kokoro phonemes for `text`, keeping only symbols the model knows (unknown words are skipped).
    pub fn phonemes(&self, text: &str) -> String {
        let (ph, _) = self.g2p.g2p(text).unwrap_or_default();
        ph.chars().filter(|c| self.vocab.contains(c)).collect::<String>().trim().to_string()
    }

    /// Synthesize one sentence or short paragraph.
    pub fn synthesize(&self, text: &str) -> Result<Audio, String> {
        let ph = self.phonemes(&text::speakable(text));
        if ph.is_empty() {
            return Ok(Audio { samples: Vec::new(), sample_rate: self.tts.sample_rate() as u32 });
        }
        let config = GenerationConfig {
            sid: self.sid,
            speed: self.speed,
            phoneme_codepoints: Some(vec![ph.chars().map(|c| c as i32).collect()]),
            ..Default::default()
        };
        let audio = self.tts.generate_with_config("", &config, None::<fn(&[f32], f32) -> bool>).ok_or("Kokoro produced no audio")?;
        Ok(Audio { samples: audio.samples().to_vec(), sample_rate: audio.sample_rate() as u32 })
    }

    /// Synthesize sentence by sentence, handing each one over as soon as it's ready. Return false from
    /// `each` to stop early (the user interrupted).
    pub fn speak_each(&self, text: &str, mut each: impl FnMut(usize, Audio) -> bool) -> Result<(), String> {
        for (i, s) in text::sentences(&text::speakable(text)).iter().enumerate() {
            let audio = self.synthesize(s)?;
            if !audio.samples.is_empty() && !each(i, audio) {
                break;
            }
        }
        Ok(())
    }
}

/// Moonshine speech-to-text.
pub struct Listener {
    recognizer: OfflineRecognizer,
}

impl Listener {
    pub fn load(models: &Path) -> Result<Self, String> {
        let dir = models::MOONSHINE.dir(models);
        if !models::MOONSHINE.is_ready(models) {
            return Err(format!("the dictation model isn't downloaded yet (run `alfred setup`); looked in {}", dir.display()));
        }
        let p = |f: &str| Some(dir.join(f).to_string_lossy().into_owned());
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.moonshine.encoder = p("encoder_model.ort");
        config.model_config.moonshine.merged_decoder = p("decoder_model_merged.ort");
        config.model_config.tokens = p("tokens.txt");
        config.model_config.num_threads = 2;
        config.model_config.debug = false;
        let recognizer = OfflineRecognizer::create(&config).ok_or("Moonshine failed to load")?;
        Ok(Self { recognizer })
    }

    pub fn transcribe(&self, samples: &[f32], sample_rate: u32) -> String {
        if samples.is_empty() {
            return String::new();
        }
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(sample_rate as i32, samples);
        self.recognizer.decode(&stream);
        stream.get_result().map(|r| r.text.trim().to_string()).unwrap_or_default()
    }

    pub fn transcribe_file(&self, path: &Path) -> Result<String, String> {
        let wave = Wave::read(&path.to_string_lossy()).ok_or_else(|| format!("couldn't read {}", path.display()))?;
        Ok(self.transcribe(wave.samples(), wave.sample_rate() as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kokoro_runs_on_the_performance_cores() {
        let n = performance_cores();
        assert!((1..=64).contains(&n), "{n}");
        assert_eq!(performance_cores(), n, "read once");
    }

    #[test]
    fn voices_resolve() {
        assert_eq!(voice_id("bm_george"), Some(26));
        assert_eq!(voice_id(" AF_HEART "), Some(3));
        assert_eq!(voice_id("nobody"), None);
    }

    #[test]
    fn envelope_is_normalized() {
        let a = Audio { samples: (0..2400).map(|i| if i < 1200 { 0.1 } else { 0.5 }).collect(), sample_rate: 2400 };
        let e = a.envelope(2.0);
        assert_eq!(e.len(), 2);
        assert!((e[1] - 1.0).abs() < 1e-6 && (e[0] - 0.2).abs() < 1e-3, "{e:?}");
    }

    /// The models folder for the round-trip test: `ALFRED_MODELS`, else Alfred's own.
    fn models() -> std::path::PathBuf {
        std::env::var_os("ALFRED_MODELS").map(Into::into).unwrap_or_else(alfred_core_models)
    }

    fn alfred_core_models() -> std::path::PathBuf {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
        home.join("Library/Application Support/Alfred/models")
    }

    /// Run with `cargo test --release -p alfred-voice -- --ignored --nocapture measure`.
    #[test]
    #[ignore]
    fn measure_kokoro() {
        let m = models();
        if !models::KOKORO.is_ready(&m) {
            return;
        }
        let speaker = Speaker::load(&m, "bm_george", 1.0).unwrap();
        speaker.synthesize("Warm up.").unwrap();
        let reply = "Of course, I have opened Notes, and your shopping list now has milk and eggs.";
        let time = |t: &str| {
            let start = std::time::Instant::now();
            let a = speaker.synthesize(t).unwrap();
            (start.elapsed().as_secs_f32(), a.seconds())
        };
        let first = text::sentences(reply).remove(0);
        eprintln!("kokoro on {} threads", performance_cores());
        for t in ["Notes is open.", reply, first.as_str()] {
            let (took, long) = time(t);
            eprintln!("kokoro: {took:.2} s for {long:.1} s of {t:?}");
        }
    }

    /// Kokoro says it, Moonshine hears it. Skips itself when the models aren't downloaded.
    #[test]
    fn round_trip_speech_is_understood() {
        let m = models();
        if !models::KOKORO.is_ready(&m) || !models::MOONSHINE.is_ready(&m) {
            eprintln!("skipping: voice models not in {}", m.display());
            return;
        }
        let speaker = Speaker::load(&m, "bm_george", 1.0).unwrap();
        let listener = Listener::load(&m).unwrap();
        let audio = speaker.synthesize("I have saved your note about buying milk.").unwrap();
        assert!(audio.seconds() > 1.0, "{} s", audio.seconds());
        let heard = listener.transcribe(&audio.samples, audio.sample_rate).to_lowercase();
        for word in ["saved", "note", "milk"] {
            assert!(heard.contains(word), "heard {heard:?}");
        }
    }
}
