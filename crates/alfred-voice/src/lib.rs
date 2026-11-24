//! Alfred's voice and ears, fully on-device through sherpa-onnx:
//!
//! - [`Speaker`]: Kokoro-82M v1.0, with phonemes from `misaki-rs` (Kokoro's own G2P, MIT). No espeak-ng,
//!   so no GPL code ends up in the binary.
//! - [`Listener`]: Moonshine tiny for dictation.
//!
//! Both load from the folder [`models`] downloads to.

pub mod models;
pub mod text;

use std::collections::HashSet;
use std::path::Path;

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
                    model: p("model.int8.onnx"),
                    voices: p("voices.bin"),
                    tokens: p("tokens.txt"),
                    length_scale: 1.0,
                    ..Default::default()
                },
                num_threads: std::thread::available_parallelism().map(|n| n.get().clamp(2, 6) as i32).unwrap_or(4),
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

    /// What has been said so far in a recording that is still being written, for live text while the
    /// user speaks. Empty when there is too little audio yet or the file can't be read.
    pub fn transcribe_partial(&self, path: &Path) -> String {
        let Ok(bytes) = std::fs::read(path) else { return String::new() };
        match read_growing_wav(&bytes) {
            Some((samples, rate)) if samples.len() as u32 >= rate * 3 / 10 => self.transcribe(&samples, rate),
            _ => String::new(),
        }
    }
}

/// The samples of a 16-bit PCM WAV, read up to the end of the file whatever its header says. A recorder
/// writes the header's sizes only when it stops, so a file still being recorded has its sound after the
/// `data` chunk and a size of 0 (or a placeholder) in front of it. Other chunks (a recorder's `FLLR`
/// padding) are skipped. Returns mono samples (channels averaged) and the sample rate.
pub fn read_growing_wav(bytes: &[u8]) -> Option<(Vec<f32>, u32)> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let u16_at = |i: usize| bytes.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |i: usize| bytes.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let mut format: Option<(u16, u32, u16)> = None; // channels, rate, bits
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32_at(at + 4)? as usize;
        let body = at + 8;
        if id == b"fmt " {
            let tag = u16_at(body)?;
            if tag != 1 && tag != 0xFFFE {
                return None; // not integer PCM
            }
            format = Some((u16_at(body + 2)?, u32_at(body + 4)?, u16_at(body + 14)?));
        } else if id == b"data" {
            let (channels, rate, bits) = format?;
            if bits != 16 || channels == 0 || rate == 0 {
                return None;
            }
            let frame = 2 * channels as usize;
            let data = &bytes[body..];
            let frames = data.len() / frame;
            let samples = (0..frames)
                .map(|f| {
                    let sum: f32 = (0..channels as usize)
                        .map(|c| {
                            let i = f * frame + 2 * c;
                            i16::from_le_bytes([data[i], data[i + 1]]) as f32 / 32768.0
                        })
                        .sum();
                    sum / channels as f32
                })
                .collect();
            return Some((samples, rate));
        }
        // Chunks are padded to an even length.
        at = body.checked_add(size + (size & 1))?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A WAV as a recorder leaves it mid-recording: sizes still 0, a padding chunk before the sound.
    fn growing_wav(samples: &[f32], rate: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF\0\0\0\0WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&1u16.to_le_bytes()); // mono
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"FLLR");
        b.extend_from_slice(&5u32.to_le_bytes());
        b.extend_from_slice(&[0; 6]); // 5 bytes, padded to 6
        b.extend_from_slice(b"data\0\0\0\0");
        for s in samples {
            b.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
        b
    }

    #[test]
    fn a_wav_still_being_recorded_is_read_to_its_end() {
        let wav = growing_wav(&[0.0, 0.5, -0.5, 0.25], 16_000);
        let (samples, rate) = read_growing_wav(&wav).unwrap();
        assert_eq!(rate, 16_000);
        assert_eq!(samples.len(), 4);
        assert!((samples[1] - 0.5).abs() < 1e-3 && (samples[2] + 0.5).abs() < 1e-3, "{samples:?}");
        // A half-written last sample is left out; anything that isn't a WAV is refused.
        assert_eq!(read_growing_wav(&wav[..wav.len() - 1]).unwrap().0.len(), 3);
        assert_eq!(read_growing_wav(b"RIFF\0\0\0\0WAVE"), None, "no data yet");
        assert_eq!(read_growing_wav(b"not a wav file at all"), None);
    }

    /// The models folder for the round-trip test: `ALFRED_MODELS`, else Alfred's own.
    fn models() -> std::path::PathBuf {
        std::env::var_os("ALFRED_MODELS").map(Into::into).unwrap_or_else(alfred_core_models)
    }

    fn alfred_core_models() -> std::path::PathBuf {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
        home.join("Library/Application Support/Alfred/models")
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

        // Live text: the first part of the sentence, from a file still being written.
        let path = std::env::temp_dir().join(format!("alfred-growing-{}.wav", std::process::id()));
        let half = &audio.samples[..audio.samples.len() * 3 / 5];
        std::fs::write(&path, growing_wav(half, audio.sample_rate)).unwrap();
        let so_far = listener.transcribe_partial(&path).to_lowercase();
        std::fs::remove_file(&path).ok();
        assert!(so_far.contains("saved") && !so_far.contains("milk"), "heard so far {so_far:?}");
    }
}
