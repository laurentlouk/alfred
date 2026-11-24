//! Saying things out loud: Kokoro sentence by sentence, each queued on the speakers as soon as it renders,
//! so the next sentence renders while the current one plays and they follow with no gap. macOS `say`
//! stands in until the Kokoro model is downloaded.

use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alfred_voice::Speaker;
use alfred_voice::player::Speakers;

/// The average level speech is brought to, about -16 dBFS like most apps' audio, so Alfred sounds as loud
/// as the rest of the Mac at the same system volume. Kokoro renders around -23 dBFS.
const TARGET_RMS: f32 = 0.16;
/// The loudest a sample may get, below clipping.
const MAX_PEAK: f32 = 0.92;

/// Raise a sentence to [`TARGET_RMS`] without its peak passing [`MAX_PEAK`]. Never makes it quieter, and
/// never more than 4 times louder (a near-silent clip stays near silent). The speakers then follow the
/// system volume.
pub fn louder(samples: &mut [f32]) {
    if samples.is_empty() {
        return;
    }
    let peak = samples.iter().fold(0f32, |m, x| m.max(x.abs()));
    let rms = (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt();
    if peak <= f32::EPSILON || rms <= f32::EPSILON {
        return;
    }
    let gain = (TARGET_RMS / rms).min(MAX_PEAK / peak).clamp(1.0, 4.0);
    for x in samples.iter_mut() {
        *x *= gain;
    }
}

/// What is playing now, so a new request (or Escape) can cut it off, and the speakers it plays on.
pub struct Playback {
    speakers: Arc<Speakers>,
    /// The system voice, while Kokoro isn't downloaded.
    say: Mutex<Option<Child>>,
    generation: AtomicU64,
}

impl Playback {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { speakers: Speakers::new(), say: Mutex::new(None), generation: AtomicU64::new(0) })
    }

    /// Open the speakers now, so a reply starts the moment it's rendered: call it when a request begins.
    pub fn warm(&self) {
        self.speakers.warm();
    }

    /// Stop the current speech and anything queued after it.
    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.speakers.stop();
        if let Ok(mut c) = self.say.lock()
            && let Some(mut child) = c.take()
        {
            child.kill().ok();
            child.wait().ok();
        }
    }

    fn current(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Run the system voice to the end; false if it was stopped.
    fn say(&self, text: &str, generation: u64) -> bool {
        let Ok(child) = Command::new("say").arg(text).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else {
            return false;
        };
        if let Ok(mut c) = self.say.lock() {
            *c = Some(child);
        }
        loop {
            std::thread::sleep(Duration::from_millis(20));
            if self.current() != generation {
                return false;
            }
            let Ok(mut c) = self.say.lock() else { return false };
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
/// loudness at 30 Hz, for the orb). Sentences are queued on the speakers as soon as Kokoro renders them, so
/// they follow each other with no gap. Blocks until done or stopped.
pub fn speak(
    speaker: Option<&Speaker>,
    text: &str,
    playback: &Arc<Playback>,
    on_sentence: impl Fn(Vec<f32>, f32) + Send + 'static,
) -> Result<(), String> {
    let generation = playback.current();
    let Some(speaker) = speaker else {
        // No Kokoro yet: the system voice, so Alfred still answers out loud.
        on_sentence(Vec::new(), 0.0);
        playback.say(&alfred_voice::text::speakable(text), generation);
        return Ok(());
    };
    playback.warm();
    // Each sentence's start, on the speakers' clock: the watcher tells the UI as playback reaches it.
    let (tx, rx) = mpsc::channel::<(f32, Vec<f32>, f32)>();
    let pb = playback.clone();
    let watcher = std::thread::spawn(move || {
        let mut end = 0.0f32;
        for (start, envelope, seconds) in rx {
            while pb.current() == generation && pb.speakers.position() + 0.005 < start {
                std::thread::sleep(Duration::from_millis(5));
            }
            if pb.current() != generation {
                return;
            }
            on_sentence(envelope, seconds);
            end = start + seconds;
        }
        // Wait for the last sentence to play out; bounded, in case the device stops (headphones unplugged).
        let limit = std::time::Instant::now() + Duration::from_secs_f32((end - pb.speakers.position()).max(0.0) + 2.0);
        while pb.current() == generation && !pb.speakers.drained() && std::time::Instant::now() < limit {
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let mut result = Ok(());
    speaker.speak_each(text, |_, mut audio| {
        if playback.current() != generation {
            return false;
        }
        louder(&mut audio.samples);
        match playback.speakers.push(&audio.samples, audio.sample_rate) {
            Ok(start) => tx.send((start, audio.envelope(30.0), audio.seconds())).is_ok(),
            Err(e) => {
                result = Err(e);
                false
            }
        }
    })?;
    playback.speakers.finish();
    drop(tx);
    watcher.join().ok();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    }

    #[test]
    fn quiet_speech_is_raised_to_the_target_without_clipping() {
        // Kokoro's own level: about -23 dBFS on average, peaks near -8 dBFS.
        // Mostly soft syllables with occasional peaks, as Kokoro renders speech.
        let mut s: Vec<f32> = (0..40_000).map(|i| (i as f32 * 0.05).sin() * if (i / 2_000) % 10 == 0 { 0.38 } else { 0.057 }).collect();
        let before = rms(&s);
        louder(&mut s);
        assert!(rms(&s) > before * 1.5, "louder: {} -> {}", before, rms(&s));
        assert!(s.iter().all(|x| x.abs() <= MAX_PEAK + 1e-4), "never clips");
    }

    #[test]
    fn loud_audio_and_silence_are_left_alone() {
        let mut loud = vec![0.9, -0.9, 0.5];
        louder(&mut loud);
        assert_eq!(loud, vec![0.9, -0.9, 0.5], "never quieter");
        let mut silent = vec![0.0; 10];
        louder(&mut silent);
        assert!(silent.iter().all(|x| *x == 0.0));
    }
}
