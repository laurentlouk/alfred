//! The two voice models Alfred downloads once, from the official sherpa-onnx releases.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A model archive and the files that prove it was unpacked.
#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    /// The folder the archive unpacks to, inside the models folder.
    pub id: &'static str,
    pub label: &'static str,
    pub url: &'static str,
    pub size_mb: u32,
    pub required: &'static [&'static str],
}

/// Kokoro-82M v1.0, int8: 54 voices, 24 kHz. Apache-2.0.
pub const KOKORO: ModelSpec = ModelSpec {
    id: "kokoro-int8-multi-lang-v1_0",
    label: "Kokoro-82M v1.0 voice (int8)",
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-int8-multi-lang-v1_0.tar.bz2",
    size_mb: 126,
    required: &["model.int8.onnx", "voices.bin", "tokens.txt"],
};

/// Moonshine tiny, English, quantized: 28 MB, transcribes seconds of speech in tens of milliseconds.
pub const MOONSHINE: ModelSpec = ModelSpec {
    id: "sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27",
    label: "Moonshine tiny dictation (English)",
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27.tar.bz2",
    size_mb: 28,
    required: &["encoder_model.ort", "decoder_model_merged.ort", "tokens.txt"],
};

pub const ALL: &[ModelSpec] = &[KOKORO, MOONSHINE];

impl ModelSpec {
    pub fn dir(&self, models: &Path) -> PathBuf {
        models.join(self.id)
    }

    pub fn is_ready(&self, models: &Path) -> bool {
        let dir = self.dir(models);
        self.required.iter().all(|f| dir.join(f).is_file())
    }

    /// The folder, downloading and unpacking the archive first if needed. Safe to re-run: a partial
    /// download is kept as `*.part` and replaced, never mistaken for a finished one.
    pub fn ensure(&self, models: &Path, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<PathBuf, String> {
        if self.is_ready(models) {
            return Ok(self.dir(models));
        }
        std::fs::create_dir_all(models).map_err(|e| format!("{}: {e}", models.display()))?;
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
            assert!(m.url.ends_with(&format!("{}.tar.bz2", m.id)), "{} unpacks to {}", m.url, m.id);
        }
    }
}
