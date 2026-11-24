//! The voice models Alfred downloads once, from the official sherpa-onnx releases.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// How a model is published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Packaging {
    /// A `.tar.bz2` archive that unpacks to the folder `id`.
    Archive,
    /// One file, saved inside the folder `id` under the name of the first required file.
    File,
}

/// A model download and the files that prove it is in place.
#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    /// The folder the model lives in, inside the models folder.
    pub id: &'static str,
    pub label: &'static str,
    pub url: &'static str,
    pub size_mb: u32,
    pub required: &'static [&'static str],
    pub packaging: Packaging,
}

/// Kokoro-82M v1.0, float32: 54 voices, 24 kHz. Apache-2.0. Float rather than int8 because it renders
/// more than twice as fast on Apple silicon (0.53 s against 1.24 s for "Notes is open." on an M1 Pro),
/// for a 350 MB download against 126 MB.
pub const KOKORO: ModelSpec = ModelSpec {
    id: "kokoro-multi-lang-v1_0",
    label: "Kokoro-82M v1.0 voice",
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-multi-lang-v1_0.tar.bz2",
    size_mb: 350,
    required: &["model.onnx", "voices.bin", "tokens.txt"],
    packaging: Packaging::Archive,
};

/// Moonshine tiny, English, quantized: 28 MB, transcribes seconds of speech in tens of milliseconds.
pub const MOONSHINE: ModelSpec = ModelSpec {
    id: "sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27",
    label: "Moonshine tiny dictation (English)",
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27.tar.bz2",
    size_mb: 28,
    required: &["encoder_model.ort", "decoder_model_merged.ort", "tokens.txt"],
    packaging: Packaging::Archive,
};

/// The English keyword spotter that hears "Alfred": a 3.3M-parameter streaming zipformer trained on
/// GigaSpeech, int8. Apache-2.0 (the model's own README). The archive is 17 MB and holds float and int8
/// copies; Alfred loads the int8 ones (5 MB).
pub const KWS: ModelSpec = ModelSpec {
    id: "sherpa-onnx-kws-zipformer-gigaspeech-3.3M-2024-01-01",
    label: "Wake word \"Alfred\" (keyword spotter)",
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/sherpa-onnx-kws-zipformer-gigaspeech-3.3M-2024-01-01.tar.bz2",
    size_mb: 17,
    required: &[
        "encoder-epoch-12-avg-2-chunk-16-left-64.int8.onnx",
        "decoder-epoch-12-avg-2-chunk-16-left-64.int8.onnx",
        "joiner-epoch-12-avg-2-chunk-16-left-64.int8.onnx",
        "tokens.txt",
    ],
    packaging: Packaging::Archive,
};

/// Silero VAD, which tells speech from silence in 32 ms windows. MIT (snakers4/silero-vad), as
/// republished by sherpa-onnx.
pub const SILERO_VAD: ModelSpec = ModelSpec {
    id: "silero-vad",
    label: "Silero voice activity detector",
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx",
    size_mb: 1,
    required: &["silero_vad.onnx"],
    packaging: Packaging::File,
};

/// GTCRN, a 48k-parameter streaming speech enhancer that takes steady background noise out of the
/// microphone audio before the VAD and Moonshine hear it. MIT (Xiaobin-Rong/gtcrn), as republished by
/// sherpa-onnx.
pub const GTCRN: ModelSpec = ModelSpec {
    id: "gtcrn",
    label: "GTCRN noise suppression",
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/speech-enhancement-models/gtcrn_simple.onnx",
    size_mb: 1,
    required: &["gtcrn_simple.onnx"],
    packaging: Packaging::File,
};

pub const ALL: &[ModelSpec] = &[KOKORO, MOONSHINE, KWS, SILERO_VAD, GTCRN];

impl ModelSpec {
    pub fn dir(&self, models: &Path) -> PathBuf {
        models.join(self.id)
    }

    pub fn is_ready(&self, models: &Path) -> bool {
        let dir = self.dir(models);
        self.required.iter().all(|f| dir.join(f).is_file())
    }

    /// The folder, downloading (and unpacking) the model first if needed. Safe to re-run: a partial
    /// download is kept as `*.part` and replaced, never mistaken for a finished one.
    pub fn ensure(&self, models: &Path, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<PathBuf, String> {
        if self.is_ready(models) {
            return Ok(self.dir(models));
        }
        std::fs::create_dir_all(models).map_err(|e| format!("{}: {e}", models.display()))?;
        if self.packaging == Packaging::File {
            let dir = self.dir(models);
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let name = self.required.first().ok_or_else(|| format!("{} names no file", self.label))?;
            let part = dir.join(format!("{name}.part"));
            download(self.url, &part, progress)?;
            std::fs::rename(&part, dir.join(name)).map_err(|e| e.to_string())?;
            return Ok(dir);
        }
        let archive = models.join(format!("{}.tar.bz2", self.id));
        let part = models.join(format!("{}.tar.bz2.part", self.id));
        download(self.url, &part, progress)?;
        std::fs::rename(&part, &archive).map_err(|e| e.to_string())?;
        let status =
            Command::new("tar").arg("-xjf").arg(&archive).arg("-C").arg(models).status().map_err(|e| format!("couldn't run tar: {e}"))?;
        std::fs::remove_file(&archive).ok();
        if !status.success() {
            return Err(format!("unpacking {} failed ({status})", self.label));
        }
        if !self.is_ready(models) {
            return Err(format!("{} unpacked but its files are missing in {}", self.label, self.dir(models).display()));
        }
        Ok(self.dir(models))
    }
}

fn download(url: &str, to: &Path, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_connect(Some(std::time::Duration::from_secs(20))).build().into();
    let resp = agent.get(url).call().map_err(|e| format!("download {url}: {e}"))?;
    let total = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
    let mut reader = resp.into_body().into_reader();
    let mut file = File::create(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("download {url}: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        progress(done, total);
    }
    file.sync_all().map_err(|e| e.to_string())?;
    if let Some(t) = total
        && t != done
    {
        return Err(format!("download {url} stopped at {done} of {t} bytes"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_needs_every_required_file() {
        let dir = std::env::temp_dir().join(format!("alfred-models-test-{}", std::process::id()));
        let m = MOONSHINE.dir(&dir);
        std::fs::create_dir_all(&m).unwrap();
        assert!(!MOONSHINE.is_ready(&dir));
        for f in MOONSHINE.required {
            std::fs::write(m.join(f), b"x").unwrap();
        }
        assert!(MOONSHINE.is_ready(&dir));
        assert!(!KOKORO.is_ready(&dir));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn urls_are_official_https_releases() {
        for m in ALL {
            assert!(m.url.starts_with("https://github.com/k2-fsa/sherpa-onnx/releases/download/"), "{}", m.url);
            match m.packaging {
                Packaging::Archive => assert!(m.url.ends_with(&format!("{}.tar.bz2", m.id)), "{} unpacks to {}", m.url, m.id),
                Packaging::File => assert!(m.url.ends_with(&format!("/{}", m.required[0])), "{} saves as {}", m.url, m.required[0]),
            }
        }
    }

    #[test]
    fn every_model_has_its_own_folder() {
        let mut ids: Vec<_> = ALL.iter().map(|m| m.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), ALL.len());
        for new in [KWS, SILERO_VAD, GTCRN] {
            assert!(ALL.iter().any(|m| m.id == new.id), "setup downloads {}", new.label);
        }
    }
}
