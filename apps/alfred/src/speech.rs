//! Saying things out loud: Kokoro sentence by sentence through `afplay`, the next sentence rendering
//! while the current one plays. macOS `say` stands in until the Kokoro model is downloaded.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alfred_voice::Speaker;

/// What is playing now, so a new request (or Escape) can cut it off.
#[derive(Default)]
pub struct Playback {
    child: Mutex<Option<Child>>,
    generation: AtomicU64,
}

impl Playback {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Stop the current speech and anything queued after it.
    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut c) = self.child.lock()
            && let Some(mut child) = c.take()
        {
            child.kill().ok();
            child.wait().ok();
        }
    }

    fn current(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Run a player process to the end; false if it was stopped.
    fn play(&self, mut cmd: Command, generation: u64) -> bool {
        let Ok(child) = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else { return false };
        if let Ok(mut c) = self.child.lock() {
            *c = Some(child);
        }
        loop {
            std::thread::sleep(Duration::from_millis(20));
            if self.current() != generation {
                return false;
            }
            let Ok(mut c) = self.child.lock() else { return false };
            match c.as_mut().map(|ch| ch.try_wait()) {
                Some(Ok(Some(_))) | None | Some(Err(_)) => {
                    c.take();
                    return self.current() == generation;
                }
                Some(Ok(None)) => {}
            }
        }
    }
}

/// Speak `text`. `on_sentence(envelope, seconds)` runs as each sentence starts playing (the envelope is
/// loudness at 30 Hz, for the orb). Blocks until done or stopped.
pub fn speak(
    speaker: Option<&Speaker>,
    text: &str,
    scratch: &Path,
    playback: &Arc<Playback>,
    on_sentence: impl Fn(Vec<f32>, f32) + Send + 'static,
) -> Result<(), String> {
    let generation = playback.current();
    let Some(speaker) = speaker else {
        // No Kokoro yet: the system voice, so Alfred still answers out loud.
        let mut cmd = Command::new("say");
        cmd.arg(alfred_voice::text::speakable(text));
        on_sentence(Vec::new(), 0.0);
        playback.play(cmd, generation);
        return Ok(());
    };
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel::<(PathBuf, Vec<f32>, f32)>();
    let pb = playback.clone();
    let player = std::thread::spawn(move || {
        for (path, envelope, seconds) in rx {
            if pb.current() == generation {
                on_sentence(envelope, seconds);
                let mut cmd = Command::new("afplay");
                cmd.arg(&path);
                pb.play(cmd, generation);
            }
            std::fs::remove_file(&path).ok();
        }
    });
    let mut result = Ok(());
    let pid = std::process::id();
    speaker.speak_each(text, |i, audio| {
        if playback.current() != generation {
            return false;
        }
        let path = scratch.join(format!("speech-{pid}-{generation}-{i}.wav"));
        if let Err(e) = audio.write_wav(&path) {
            result = Err(e);
            return false;
        }
        tx.send((path, audio.envelope(30.0), audio.seconds())).is_ok()
    })?;
    drop(tx);
    player.join().ok();
    result
}
