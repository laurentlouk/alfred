//! Alfred's ears: one microphone stream for the wake word, voice activity and live text.
//!
//! A thread owns the microphone, the [`WakeWord`] spotter (when the wake word is on), a GTCRN
//! [`Denoiser`], a [`Vad`] and its own Moonshine [`Listener`]. It reports what it hears through a
//! callback, as [`Heard`] events. During a command the microphone audio is cleaned of steady noise
//! first; the VAD, which alone decides where speech ends, and the orb's level hear the cleaned audio,
//! and Moonshine a mix of cleaned and original (see [`crate::denoise`]). The wake word listens to the
//! original audio, which keeps it at a fraction of the CPU.
//!
//! Every duration is counted in samples heard, so the decisions are the same whether audio comes from
//! the microphone in real time or from a test, faster.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::Listener;
use crate::capture::Mic;
use crate::denoise::Denoiser;
use crate::vad::{self, SAMPLE_RATE, Vad, VadConfig};
use crate::wake::{self, WakeWord};

/// How the ears listen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EarsConfig {
    /// Keep the microphone open and listen for "Alfred". Off: the microphone opens only for
    /// [`Ears::listen_now`].
    pub wake_word: bool,
    /// The keyword spotter's threshold: lower hears "Alfred" more easily.
    pub wake_threshold: f32,
    /// Silence that ends a command whose text looks like a finished sentence.
    pub silence_fast_secs: f32,
    /// Silence that ends a command otherwise ("open Notes and…").
    pub silence_slow_secs: f32,
    /// The longest command.
    pub max_listen_secs: f32,
    /// A command with no speech this long ends with [`Heard::Nothing`].
    pub no_speech_secs: f32,
    /// Silero's threshold in a quiet room (it rises by up to 0.15 with the room's noise): how sure the
    /// VAD must be that a window is speech.
    pub vad_threshold: f32,
    /// Speech must last this long before it counts, so a click, a cough or a stray word doesn't start
    /// a command.
    pub min_speech_secs: f32,
    /// Speech must be this many dB above the room's noise floor, learned while no one speaks.
    pub above_noise_db: f32,
    /// Once the speaker's level is known (from "Alfred", or the command's first words), a voice this
    /// many dB quieter (someone further away) neither starts nor extends the command.
    pub below_voice_db: f32,
    /// Clean steady noise out of the audio before the VAD and Moonshine hear it.
    pub denoise: bool,
}

impl Default for EarsConfig {
    fn default() -> Self {
        let vad_defaults = VadConfig::default();
        Self {
            wake_word: false,
            wake_threshold: wake::DEFAULT_THRESHOLD,
            silence_fast_secs: 0.6,
            silence_slow_secs: 1.2,
            max_listen_secs: 20.0,
            no_speech_secs: 4.0,
            vad_threshold: vad_defaults.threshold,
            min_speech_secs: vad_defaults.min_speech_secs,
            above_noise_db: vad_defaults.above_noise_db,
            below_voice_db: vad_defaults.below_voice_db,
            denoise: true,
        }
    }
}

impl EarsConfig {
    fn vad(&self) -> VadConfig {
        VadConfig {
            threshold: self.vad_threshold,
            min_speech_secs: self.min_speech_secs,
            above_noise_db: self.above_noise_db,
            below_voice_db: self.below_voice_db,
        }
    }
}

/// What the ears report.
#[derive(Debug, Clone, PartialEq)]
pub enum Heard {
    /// "Alfred" was heard; the command after it is being captured.
    Wake,
    /// Loudness during a command, 0 to 1, about 30 times a second.
    Level(f32),
    /// The command's text so far, about every 0.6 s while the user speaks and once at each pause.
    Partial(String),
    /// The whole command's text, without the wake word.
    Final(String),
    /// The command ended without words.
    Nothing,
    Error(String),
}

enum Control {
    ListenNow,
    Finish,
    Cancel,
    Speaking(bool),
    Muted(bool),
    Stop,
}

/// The listening thread. Dropping it stops the thread and closes the microphone.
pub struct Ears {
    control: Sender<Control>,
    thread: Option<JoinHandle<()>>,
}

impl Ears {
    /// Load the models (a fraction of a second) and start listening. `on` runs on the listening thread
    /// for every event, so it should hand the event over rather than block. Fails when a model is
    /// missing or doesn't load; microphone problems arrive as [`Heard::Error`], and with the wake word
    /// on the microphone is retried every few seconds.
    pub fn start(models: &Path, cfg: EarsConfig, on: impl Fn(Heard) + Send + 'static) -> Result<Ears, String> {
        Self::start_with(models, cfg, || MicSource(None), on)
    }

    /// [`Ears::start`] with audio from `source` (built on the listening thread) instead of the
    /// microphone.
    pub(crate) fn start_with<S: Source>(
        models: &Path,
        cfg: EarsConfig,
        source: impl FnOnce() -> S + Send + 'static,
        on: impl Fn(Heard) + Send + 'static,
    ) -> Result<Ears, String> {
        let models = models.to_path_buf();
        let (control, commands) = mpsc::channel();
        let (ready, loaded) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("alfred-ears".into())
            .spawn(move || {
                let core = match Core::load(&models, cfg) {
                    Ok(core) => core,
                    Err(e) => {
                        ready.send(Err(e)).ok();
                        return;
                    }
                };
                ready.send(Ok(())).ok();
                run(core, source(), commands, on);
            })
            .map_err(|e| format!("couldn't start the listening thread: {e}"))?;
        match loaded.recv() {
            Ok(Ok(())) => Ok(Ears { control, thread: Some(thread) }),
            Ok(Err(e)) => {
                thread.join().ok();
                Err(e)
            }
            Err(_) => {
                thread.join().ok();
                Err("the listening thread stopped while loading".into())
            }
        }
    }

    /// Click-to-talk: capture a command now (opening the microphone if the wake word is off).
    pub fn listen_now(&self) {
        self.control.send(Control::ListenNow).ok();
    }

    /// End the current command now and transcribe it.
    pub fn finish(&self) {
        self.control.send(Control::Finish).ok();
    }

    /// Drop the current command without a result.
    pub fn cancel(&self) {
        self.control.send(Control::Cancel).ok();
    }

    /// Alfred is talking (or stopped). Saying "Alfred" still barges in: a hit emits [`Heard::Wake`]
    /// and captures the command after it, so the app should stop talking on `Wake`. Nothing else
    /// starts a capture by itself. While Alfred talks, click-to-talk leaves out the audio from before
    /// the click, which is its own voice.
    pub fn set_speaking(&self, on: bool) {
        self.control.send(Control::Speaking(on)).ok();
    }

    /// Mute: the microphone closes (macOS's indicator goes out), the command being heard is dropped, and
    /// neither "Alfred" nor click-to-talk captures anything until unmuted.
    pub fn set_muted(&self, muted: bool) {
        self.control.send(Control::Muted(muted)).ok();
    }
}

impl Drop for Ears {
    fn drop(&mut self) {
        self.control.send(Control::Stop).ok();
        if let Some(t) = self.thread.take() {
            // Dropped from its own callback, the thread stops by itself once the callback returns.
            if t.thread().id() != std::thread::current().id() {
                t.join().ok();
            }
        }
    }
}

/// Where the listening thread's audio comes from: the microphone, or a test.
pub(crate) trait Source {
    /// Start delivering audio.
    fn open(&mut self) -> Result<(), String>;
    /// Stop delivering audio.
    fn close(&mut self);
    fn is_open(&self) -> bool;
    /// The next 16 kHz mono audio, waiting at most `wait`: `Ok(None)` when none came. An error means
    /// the source closed.
    fn read(&mut self, wait: Duration) -> Result<Option<Vec<f32>>, String>;
}

struct MicSource(Option<Mic>);

impl Source for MicSource {
    fn open(&mut self) -> Result<(), String> {
        if self.0.is_none() {
            self.0 = Some(Mic::open()?);
        }
        Ok(())
    }

    fn close(&mut self) {
        if let Some(mic) = self.0.take() {
            mic.close();
        }
    }

    fn is_open(&self) -> bool {
        self.0.is_some()
    }

    fn read(&mut self, wait: Duration) -> Result<Option<Vec<f32>>, String> {
        match &mut self.0 {
            Some(mic) => mic.read(wait),
            None => Ok(None),
        }
    }
}

/// How long one read waits for audio; controls are handled between reads.
const READ_WAIT: Duration = Duration::from_millis(20);
/// After the microphone fails with the wake word on, try again this often.
const MIC_RETRY: Duration = Duration::from_secs(5);

fn run<S: Source>(mut core: Core, mut source: S, commands: Receiver<Control>, on: impl Fn(Heard)) {
    let mut retry_at: Option<Instant> = None;
    // The last microphone error, so retries in the background don't repeat it.
    let mut last_error: Option<String> = None;
    loop {
        let idle = !source.is_open() && (!core.wants_mic() || retry_at.is_some());
        let next = if idle {
            // Nothing to listen to: sleep until a command (or the next retry).
            match retry_at {
                Some(at) => match commands.recv_timeout(at.saturating_duration_since(Instant::now())) {
                    Ok(c) => Some(c),
                    Err(RecvTimeoutError::Timeout) => {
                        retry_at = None;
                        None
                    }
                    Err(RecvTimeoutError::Disconnected) => Some(Control::Stop),
                },
                None => Some(commands.recv().unwrap_or(Control::Stop)),
            }
        } else {
            match commands.try_recv() {
                Ok(c) => Some(c),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Control::Stop),
            }
        };
        if let Some(c) = next {
            match c {
                Control::Stop => break,
                Control::ListenNow => {
                    retry_at = None;
                    last_error = None;
                    core.listen_now(&on);
                }
                Control::Finish => core.finish(&on),
                Control::Cancel => core.cancel(),
                Control::Speaking(speaking) => core.speaking = speaking,
                Control::Muted(muted) => {
                    if muted {
                        core.cancel();
                    }
                    core.muted = muted;
                    retry_at = None;
                }
            }
            continue;
        }
        // The microphone is open exactly while the core needs audio.
        if core.wants_mic() && !source.is_open() {
            match source.open() {
                Ok(()) => last_error = None,
                Err(e) => {
                    if last_error.as_deref() != Some(e.as_str()) {
                        on(Heard::Error(e.clone()));
                    }
                    last_error = Some(e);
                    core.cancel();
                    retry_at = Some(Instant::now() + MIC_RETRY);
                }
            }
            continue;
        }
        if !core.wants_mic() {
            source.close();
            continue;
        }
        match source.read(READ_WAIT) {
            Ok(Some(chunk)) => core.feed(&chunk, &on),
            Ok(None) => {}
            Err(e) => {
                on(Heard::Error(e.clone()));
                last_error = Some(e);
                source.close();
                core.cancel();
                retry_at = Some(Instant::now() + MIC_RETRY);
            }
        }
    }
    source.close();
}

/// Audio kept in wake mode, so the command said right after "Alfred" is there when the spotter fires
/// (about 0.3 s after the word ends).
const RECENT_SECS: f32 = 3.0;
/// Click-to-talk with the microphone already open keeps this much audio from before the click.
const PREROLL_SECS: f32 = 0.3;
const LEVEL_HZ: f32 = 30.0;
/// Live text this often, or less often when transcribing takes longer: a partial waits at least twice
/// as long as the last one took (Moonshine takes about 70 ms for 2 s of speech and 175 ms for 5 s), so
/// the thread keeps up with the microphone through a long command.
const PARTIAL_EVERY_SECS: f32 = 0.6;
/// Audio kept before the speech the VAD found when transcribing: it confirms speech about 0.1 s late.
const PAD_SECS: f32 = 0.3;
/// Audio kept after the speaker's last loud window, fading out: the soft end of their last word, and
/// as little as possible of whoever talks next.
const TAIL_SECS: f32 = 0.25;
/// A command after the wake word starts where the spotter places the end of "Alfred", and the word's
/// "d" can ring on past it for 0.2 s when it is said alone. The VAD skips this much, so that tail
/// doesn't count as the command's speech, while a word said in the same breath ("Alfred, stop") is
/// still long enough to be heard.
const KEYWORD_TAIL_SECS: f32 = 0.2;
/// The denoiser settles on this much of the audio before a command, when there is some.
const WARM_UP_SECS: f32 = 1.0;
/// The audio before a command's start that holds "Alfred", whose level tells the speaker's.
const KEYWORD_SECS: f32 = 0.7;

/// The listening decisions, fed 16 kHz audio.
struct Core {
    cfg: EarsConfig,
    denoiser: Option<Denoiser>,
    vad: Vad,
    wake: Option<WakeWord>,
    listener: Listener,
    /// Alfred is talking.
    speaking: bool,
    /// The user turned listening off: no microphone at all.
    muted: bool,
    /// The last seconds of audio while waiting for the wake word.
    recent: VecDeque<f32>,
    command: Option<Command>,
}

/// A command being captured. Positions are samples from its start.
#[derive(Default)]
struct Command {
    audio: Vec<f32>,
    /// Samples the VAD hears as silence from the start.
    deaf: usize,
    level_sum: f32,
    level_n: usize,
    since_partial: usize,
    /// Audio between partials, in samples.
    partial_every: usize,
    was_speech: bool,
    /// The latest transcription and the speech span it covers.
    transcribed: Option<((u64, u64), String)>,
}

impl Core {
    fn load(models: &Path, cfg: EarsConfig) -> Result<Self, String> {
        let wake = if cfg.wake_word { Some(WakeWord::load(models, cfg.wake_threshold)?) } else { None };
        Ok(Self {
            cfg,
            denoiser: if cfg.denoise { Some(Denoiser::load(models)?) } else { None },
            vad: Vad::load(models, cfg.vad())?,
            wake,
            listener: Listener::load(models)?,
            speaking: false,
            muted: false,
            recent: VecDeque::with_capacity(samples(RECENT_SECS)),
            command: None,
        })
    }

    fn wants_mic(&self) -> bool {
        !self.muted && (self.wake.is_some() || self.command.is_some())
    }

    fn listen_now(&mut self, emit: &dyn Fn(Heard)) {
        if self.command.is_some() || self.muted {
            return;
        }
        // While Alfred talks, the audio before the click is its own voice.
        let preroll = if self.speaking { 0 } else { samples(PREROLL_SECS).min(self.recent.len()) };
        let start = self.recent.len() - preroll;
        self.begin(start);
        let before = self.recent_from(start, preroll);
        self.hear(&before, emit);
    }

    /// `len` samples of the recent audio from index `from`.
    fn recent_from(&self, from: usize, len: usize) -> Vec<f32> {
        self.recent.range(from..(from + len).min(self.recent.len())).copied().collect()
    }

    fn feed(&mut self, chunk: &[f32], emit: &dyn Fn(Heard)) {
        if self.command.is_some() {
            self.hear(chunk, emit);
            return;
        }
        let Some(wake) = self.wake.as_mut() else { return };
        self.recent.extend(chunk);
        let excess = self.recent.len().saturating_sub(samples(RECENT_SECS));
        self.recent.drain(..excess);
        if let Some(hit) = wake.accept(chunk) {
            // A hit while Alfred talks is a barge-in: the command is captured all the same.
            emit(Heard::Wake);
            let since_keyword = ((wake.position() - hit.end) as usize).min(self.recent.len());
            let start = self.recent.len() - since_keyword;
            self.begin(start);
            let keyword = self.recent_from(start.saturating_sub(samples(KEYWORD_SECS)), samples(KEYWORD_SECS).min(start));
            self.vad.expect_voice(vad::peak_db(&keyword));
            if let Some(cmd) = self.command.as_mut() {
                cmd.deaf = samples(KEYWORD_TAIL_SECS);
            }
            let after = self.recent_from(start, since_keyword);
            self.hear(&after, emit);
        }
    }

    /// Start a command at index `start` of the recent audio; the denoiser settles on what came before.
    fn begin(&mut self, start: usize) {
        self.vad.reset();
        let warm = self.recent_from(start.saturating_sub(samples(WARM_UP_SECS)), samples(WARM_UP_SECS).min(start));
        if let Some(d) = self.denoiser.as_mut() {
            d.reset();
            d.warm_up(&warm);
        }
        self.command = Some(Command { partial_every: samples(PARTIAL_EVERY_SECS), ..Command::default() });
    }

    fn hear(&mut self, raw: &[f32], emit: &dyn Fn(Heard)) {
        let Some(cmd) = self.command.as_mut() else { return };
        let (chunk, words) = match self.denoiser.as_mut() {
            Some(d) => {
                let out = d.run(raw);
                (out.speech, out.words)
            }
            None => (raw.to_vec(), raw.to_vec()),
        };
        let chunk = chunk.as_slice();
        cmd.audio.extend_from_slice(&words);
        let deaf = cmd.deaf.min(chunk.len());
        if deaf > 0 {
            self.vad.skip(deaf);
            cmd.deaf -= deaf;
        }
        self.vad.accept(&chunk[deaf..]);
        let level_step = samples(1.0 / LEVEL_HZ);
        for s in chunk {
            cmd.level_sum += s * s;
            cmd.level_n += 1;
            if cmd.level_n >= level_step {
                emit(Heard::Level(level(cmd.level_sum / cmd.level_n as f32)));
                cmd.level_sum = 0.0;
                cmd.level_n = 0;
            }
        }
        cmd.since_partial += chunk.len();
        let speech = self.vad.is_speech();
        let paused = cmd.was_speech && !speech;
        cmd.was_speech = speech;
        if self.vad.heard_speech() && ((speech && cmd.since_partial >= cmd.partial_every) || paused) {
            self.partial(emit);
        }
        let Some(cmd) = self.command.as_ref() else { return };
        let heard_secs = cmd.audio.len() as f32 / SAMPLE_RATE as f32;
        if !self.vad.heard_speech() {
            if heard_secs >= self.cfg.no_speech_secs.min(self.cfg.max_listen_secs) {
                self.end(emit);
            }
            return;
        }
        let text = cmd.transcribed.as_ref().map(|(_, t)| t.as_str()).unwrap_or("");
        let enough = if looks_complete(text) { self.cfg.silence_fast_secs } else { self.cfg.silence_slow_secs };
        if heard_secs >= self.cfg.max_listen_secs || (!speech && self.vad.silence_secs() >= enough) {
            self.end(emit);
        }
    }

    /// Transcribe the speech so far and report it if it changed.
    fn partial(&mut self, emit: &dyn Fn(Heard)) {
        let Some(span) = self.vad.speech_span() else { return };
        let Some(cmd) = self.command.as_mut() else { return };
        cmd.since_partial = 0;
        if cmd.transcribed.as_ref().is_some_and(|(s, _)| *s == span) {
            return;
        }
        let took = Instant::now();
        let text = transcribe(&self.listener, &cmd.audio, span);
        cmd.partial_every = samples(PARTIAL_EVERY_SECS).max(samples(2.0 * took.elapsed().as_secs_f32()));
        if text.is_empty() && !self.vad.is_speech() {
            // Sound without words (a cough, the ring of the wake word): it isn't the command yet.
            self.vad.forget_speech();
            cmd.transcribed = None;
            return;
        }
        let changed = cmd.transcribed.as_ref().is_none_or(|(_, t)| *t != text);
        if changed && !text.is_empty() {
            emit(Heard::Partial(text.clone()));
        }
        cmd.transcribed = Some((span, text));
    }

    /// End the command: its text, or Nothing.
    fn end(&mut self, emit: &dyn Fn(Heard)) {
        let Some(cmd) = self.command.take() else { return };
        let text = match (self.vad.speech_span(), cmd.transcribed) {
            (None, _) => String::new(),
            // Ending on silence: the pause's transcription already covers every word.
            (Some(span), Some((done, text))) if done == span => text,
            (Some(span), _) => transcribe(&self.listener, &cmd.audio, span),
        };
        emit(if text.is_empty() { Heard::Nothing } else { Heard::Final(text) });
        self.after_command();
    }

    fn finish(&mut self, emit: &dyn Fn(Heard)) {
        self.end(emit);
    }

    fn cancel(&mut self) {
        if self.command.take().is_some() {
            self.after_command();
        }
    }

    /// Back to waiting for the wake word, on fresh audio.
    fn after_command(&mut self) {
        self.vad.reset();
        self.recent.clear();
        if let Some(w) = self.wake.as_mut() {
            w.reset();
        }
    }
}

/// The text of the speech in `span`, with some audio around it, without a leading wake word.
fn transcribe(listener: &Listener, audio: &[f32], (start, end): (u64, u64)) -> String {
    let from = (start.saturating_sub(samples(PAD_SECS) as u64) as usize).min(audio.len());
    let end = (end as usize).clamp(from, audio.len());
    let to = (end + samples(TAIL_SECS)).min(audio.len());
    let mut clip = audio[from..to].to_vec();
    let tail = (to - end).max(1) as f32;
    for (i, s) in clip[end - from..].iter_mut().enumerate() {
        *s *= 1.0 - i as f32 / tail;
    }
    strip_wake_word(&listener.transcribe(&clip, SAMPLE_RATE))
}

fn samples(secs: f32) -> usize {
    (secs.max(0.0) * SAMPLE_RATE as f32) as usize
}

/// Loudness for the orb: -60 dBFS (a quiet room) is 0, -10 dBFS (speaking close to the mic) is 1.
fn level(mean_square: f32) -> f32 {
    let db = 10.0 * mean_square.max(1e-12).log10();
    ((db + 60.0) / 50.0).clamp(0.0, 1.0)
}

/// Words a sentence doesn't end on: when the text so far ends with one, the speaker is likely to go on.
const TRAILING: &[&str] = &[
    "and", "but", "so", "or", "um", "uh", "the", "a", "to", "of", // English
    "et", "mais", "donc", "ou", "euh", "le", "la", "les", "de", "du", "pour", // French
];

/// Whether `text` reads as a finished sentence, so a short pause can end the command. A filler or
/// conjunction at the end outweighs a final period, because Moonshine punctuates what it hears
/// ("Remind me to" comes out as "Remind me to."). A trailing ellipsis or comma reads as unfinished too.
pub fn looks_complete(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || t.ends_with("...") || t.ends_with('…') || t.ends_with(',') {
        return false;
    }
    let Some(last) = t.split_whitespace().last() else { return false };
    let word = last.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'').to_lowercase();
    !TRAILING.contains(&word.as_str())
}

/// `text` without a leading "Alfred" or "Hey Alfred", in case the transcription caught the end of
/// the keyword.
fn strip_wake_word(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let bare = |w: &str| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    let skip = match words.as_slice() {
        [a, ..] if bare(a) == "alfred" => 1,
        [a, b, ..] if matches!(bare(a).as_str(), "hey" | "ok" | "okay") && bare(b) == "alfred" => 2,
        _ => return text.trim().to_string(),
    };
    // "Alfred, open notes." becomes "Open notes.": the sentence keeps its capital.
    let rest = words[skip..].join(" ");
    let mut chars = rest.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sentence_is_complete_unless_it_trails_off() {
        for done in ["Open Notes.", "What time is it?", "Stop!", "open notes and save buy milk", "Ouvre Notes.", "Quelle heure est-il ?"] {
            assert!(looks_complete(done), "{done:?}");
        }
        for open in [
            "",
            "  ",
            "Open Notes and",
            "Open Notes and.",
            "open notes, um",
            "Remind me to",
            "Play the",
            "a cup of",
            "Ouvre Notes et",
            "Rappelle-moi de",
            "Mets la musique, euh",
            "Note pour",
            "Open Notes...",
            "Open Notes,",
            "Open Notes or…",
            "AND",
        ] {
            assert!(!looks_complete(open), "{open:?}");
        }
    }

    #[test]
    fn the_wake_word_is_cut_from_the_text() {
        assert_eq!(strip_wake_word("Alfred, open Notes."), "Open Notes.");
        assert_eq!(strip_wake_word("Hey Alfred open notes"), "Open notes");
        assert_eq!(strip_wake_word("alfred."), "");
        assert_eq!(strip_wake_word("Open Notes."), "Open Notes.");
        assert_eq!(strip_wake_word("Hey, open Notes."), "Hey, open Notes.");
        assert_eq!(strip_wake_word("Ask Alfred later."), "Ask Alfred later.");
    }

    #[test]
    fn loudness_maps_to_zero_to_one() {
        assert_eq!(level(0.0), 0.0);
        assert!((level(0.1f32.powi(2)) - 0.8).abs() < 1e-4, "-20 dBFS");
        assert_eq!(level(1.0), 1.0);
    }

    #[test]
    fn defaults_are_as_documented() {
        let d = EarsConfig::default();
        assert!(!d.wake_word);
        assert_eq!((d.silence_fast_secs, d.silence_slow_secs, d.max_listen_secs, d.no_speech_secs), (0.6, 1.2, 20.0, 4.0));
        assert_eq!(d.wake_threshold, wake::DEFAULT_THRESHOLD);
    }
}

/// The listening thread end to end, with Kokoro's speech from a feeder instead of the microphone.
#[cfg(test)]
mod listening {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;
    use crate::models;
    use crate::testkit::*;

    /// Hands out its audio in 10 ms chunks, as fast as it is read, up to `limit` samples.
    struct Feeder {
        audio: Vec<f32>,
        at: usize,
        open: Arc<AtomicBool>,
        opens: Arc<AtomicUsize>,
        /// How far the ears have read; an event carries the value at the time it was emitted.
        pos: Arc<AtomicUsize>,
        limit: Arc<AtomicUsize>,
        /// Where the feeder last ran out of audio to give: the ears handled every chunk before it.
        dry_at: Arc<AtomicUsize>,
    }

    impl Source for Feeder {
        fn open(&mut self) -> Result<(), String> {
            self.open.store(true, Ordering::SeqCst);
            self.opens.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn close(&mut self) {
            self.open.store(false, Ordering::SeqCst);
        }

        fn is_open(&self) -> bool {
            self.open.load(Ordering::SeqCst)
        }

        fn read(&mut self, wait: Duration) -> Result<Option<Vec<f32>>, String> {
            let end = (self.at + 160).min(self.audio.len()).min(self.limit.load(Ordering::SeqCst));
            if end <= self.at {
                self.dry_at.store(self.at, Ordering::SeqCst);
                std::thread::sleep(wait);
                return Ok(None);
            }
            let chunk = self.audio[self.at..end].to_vec();
            self.at = end;
            self.pos.store(end, Ordering::SeqCst);
            Ok(Some(chunk))
        }
    }

    struct Run {
        ears: Ears,
        events: mpsc::Receiver<(f32, Heard)>,
        open: Arc<AtomicBool>,
        opens: Arc<AtomicUsize>,
        limit: Arc<AtomicUsize>,
        dry_at: Arc<AtomicUsize>,
        len: usize,
    }

    /// The ears on `audio`, gated: they hear nothing until [`Run::play`] or [`Run::feed_until`].
    fn start(models: &Path, cfg: EarsConfig, audio: Vec<f32>) -> Run {
        let (open, opens, pos) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (limit, dry_at) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(usize::MAX)));
        let len = audio.len();
        let feeder = Feeder {
            audio,
            at: 0,
            open: open.clone(),
            opens: opens.clone(),
            pos: pos.clone(),
            limit: limit.clone(),
            dry_at: dry_at.clone(),
        };
        let (tx, events) = mpsc::channel();
        let ears = Ears::start_with(models, cfg, move || feeder, move |h| tx.send((secs(pos.load(Ordering::SeqCst)), h)).unwrap()).unwrap();
        Run { ears, events, open, opens, limit, dry_at, len }
    }

    impl Run {
        /// Let the ears hear all of the audio.
        fn play(&self) {
            self.limit.store(usize::MAX, Ordering::SeqCst);
        }

        /// Events up to the command's end (`Final` or `Nothing`), with the audio time of each.
        fn command(&self) -> Vec<(f32, Heard)> {
            let mut out = Vec::new();
            while let Ok((t, h)) = self.events.recv_timeout(Duration::from_secs(30)) {
                let done = matches!(h, Heard::Final(_) | Heard::Nothing);
                out.push((t, h));
                if done {
                    return out;
                }
            }
            panic!("the command never ended: {out:?}");
        }

        /// Lets the ears hear the audio up to `limit` samples, waits until they have, and returns the
        /// events so far.
        fn feed_until(&self, limit: usize) -> Vec<Heard> {
            self.limit.store(limit, Ordering::SeqCst);
            let t = Instant::now();
            while self.dry_at.load(Ordering::SeqCst) != limit.min(self.len) {
                assert!(t.elapsed() < Duration::from_secs(30), "the ears stopped reading");
                std::thread::sleep(Duration::from_millis(5));
            }
            self.events.try_iter().map(|(_, h)| h).collect()
        }
    }

    fn last_text(events: &[(f32, Heard)]) -> (f32, Heard) {
        events.last().cloned().unwrap()
    }

    fn models_ready() -> Option<std::path::PathBuf> {
        ready(&[models::MOONSHINE, models::SILERO_VAD, models::KWS])
    }

    /// Where the sound of `speech` ends, in seconds from its start.
    fn sound_ends(speech: &[f32]) -> f32 {
        let e: Vec<f32> = speech.chunks(160).map(|c| c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).collect();
        let peak = e.iter().copied().fold(0.0, f32::max);
        (e.iter().rposition(|&v| v > peak * 1e-3).unwrap_or(0) + 1) as f32 * 0.01
    }

    fn wake_on() -> EarsConfig {
        EarsConfig { wake_word: true, ..EarsConfig::default() }
    }

    #[test]
    fn alfred_then_a_command_in_one_breath() {
        let Some(m) = models_ready() else { return };
        let speech = say("Alfred, open Notes and save buy milk.");
        let mut audio = silence(1.0);
        audio.extend(&speech);
        audio.extend(silence(3.0));
        let run = start(&m, wake_on(), audio);
        run.play();
        let events = run.command();
        assert_eq!(events[0].1, Heard::Wake, "{events:?}");
        let (at, end) = last_text(&events);
        let Heard::Final(text) = end else { panic!("{events:?}") };
        let text = text.to_lowercase();
        assert!(text.contains("open notes") && text.contains("milk") && !text.contains("alfred"), "{text:?}");
        let partials = events.iter().filter(|(_, h)| matches!(h, Heard::Partial(_))).count();
        assert!(partials >= 2, "live text while speaking: {events:?}");
        // Levels about 30 times a second through the command, louder while it's spoken.
        let levels: Vec<f32> = events.iter().filter_map(|(_, h)| if let Heard::Level(l) = h { Some(*l) } else { None }).collect();
        let wake_at = events[1].0;
        assert!((levels.len() as f32 / (at - wake_at + 0.3) - 30.0).abs() < 6.0, "{} levels over {} s", levels.len(), at - wake_at);
        assert!(levels.iter().copied().fold(0.0, f32::max) > 0.5 && levels.iter().all(|l| (0.0..=1.0).contains(l)));
        // The sentence is complete, so a short silence ends it.
        let latency = at - (1.0 + sound_ends(&speech));
        eprintln!("one breath: {text:?}; ended {latency:.2} s after the speech; {partials} partials");
        assert!(latency > 0.4 && latency < 0.95, "ended {latency:.2} s after the speech");
        assert!(run.open.load(Ordering::SeqCst), "the microphone stays open for the wake word");
    }

    #[test]
    fn alfred_a_pause_then_the_command() {
        let Some(m) = models_ready() else { return };
        let mut audio = silence(0.5);
        audio.extend(say("Alfred."));
        audio.extend(silence(1.5));
        audio.extend(say("Open Notes and save buy milk."));
        audio.extend(silence(3.0));
        let run = start(&m, wake_on(), audio);
        run.play();
        let events = run.command();
        assert_eq!(events[0].1, Heard::Wake);
        let Heard::Final(text) = last_text(&events).1 else { panic!("{events:?}") };
        let text = text.to_lowercase();
        assert!(text.starts_with("open notes") && text.contains("milk"), "{text:?}");
    }

    /// Waits until `done` holds, at most 10 s.
    fn wait_for(done: impl Fn() -> bool) {
        let t = Instant::now();
        while !done() {
            assert!(t.elapsed() < Duration::from_secs(10), "it never happened");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn muting_closes_the_microphone_and_ignores_everything_until_unmuted() {
        let Some(m) = models_ready() else { return };
        let mut audio = say("Alfred, open Notes.");
        audio.extend(silence(1.0));
        let run = start(&m, wake_on(), audio);
        wait_for(|| run.open.load(Ordering::SeqCst));
        run.ears.set_muted(true);
        wait_for(|| !run.open.load(Ordering::SeqCst));
        run.ears.listen_now();
        run.play();
        std::thread::sleep(Duration::from_millis(400));
        assert!(!run.open.load(Ordering::SeqCst), "click-to-talk is off while muted");
        assert!(run.events.try_iter().all(|(_, h)| matches!(h, Heard::Level(_))), "nothing is heard while muted");
        run.ears.set_muted(false);
        wait_for(|| run.open.load(Ordering::SeqCst));
    }

    #[test]
    fn without_alfred_nothing_happens() {
        let Some(m) = models_ready() else { return };
        let mut audio = say("Open Notes and save buy milk.");
        audio.extend(silence(1.0));
        audio.extend(say("Remind me to call my mother."));
        audio.extend(silence(1.0));
        let run = start(&m, wake_on(), audio);
        assert_eq!(run.feed_until(usize::MAX), vec![]);
    }

    #[test]
    fn a_finished_sentence_ends_sooner_than_a_dangling_and() {
        let Some(m) = models_ready() else { return };
        let mut ended = Vec::new();
        for sentence in ["Open Notes.", "Open Notes and"] {
            let speech = say(sentence);
            let mut audio = speech.clone();
            audio.extend(silence(3.0));
            let run = start(&m, EarsConfig::default(), audio);
            run.play();
            assert!(!run.open.load(Ordering::SeqCst), "no microphone before a click");
            run.ears.listen_now();
            let events = run.command();
            let (at, end) = last_text(&events);
            assert!(matches!(end, Heard::Final(_)), "{events:?}");
            ended.push(at - sound_ends(&speech));
            // The microphone closes after the command.
            let t = Instant::now();
            while run.open.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(!run.open.load(Ordering::SeqCst) && run.opens.load(Ordering::SeqCst) == 1);
        }
        eprintln!("end of speech: {:.2} s after \"Open Notes.\", {:.2} s after \"Open Notes and\"", ended[0], ended[1]);
        assert!(ended[0] > 0.2 && ended[0] < 0.95, "{ended:?}");
        assert!(ended[1] > 1.0 && ended[1] < 1.55, "{ended:?}");
    }

    #[test]
    fn silence_ends_with_nothing() {
        let Some(m) = models_ready() else { return };
        let run = start(&m, EarsConfig::default(), silence(8.0));
        run.play();
        run.ears.listen_now();
        let events = run.command();
        let (at, end) = last_text(&events);
        assert_eq!(end, Heard::Nothing);
        assert!((at - 4.0).abs() < 0.1, "after {at} s");
    }

    #[test]
    fn a_long_command_stops_at_the_limit() {
        let Some(m) = models_ready() else { return };
        let mut audio = say("Remind me tomorrow morning to call the bank about the mortgage and then to buy milk, bread and eggs.");
        audio.extend(silence(2.0));
        let run = start(&m, EarsConfig { max_listen_secs: 2.0, ..EarsConfig::default() }, audio);
        run.play();
        run.ears.listen_now();
        let (at, end) = last_text(&run.command());
        // Moonshine sometimes mishears "Remind me" in some renderings; the middle of the sentence is solid.
        assert!(matches!(&end, Heard::Final(t) if t.to_lowercase().replace('-', "").contains("tomorrow morning")), "{end:?}");
        assert!((at - 2.0).abs() < 0.05, "after {at} s");
    }

    #[test]
    fn finish_transcribes_now() {
        let Some(m) = models_ready() else { return };
        let speech = say("Open Notes and save buy milk.");
        let run = start(&m, EarsConfig::default(), speech.clone());
        run.ears.listen_now();
        run.feed_until(speech.len() * 2 / 3);
        run.ears.finish();
        let Heard::Final(text) = last_text(&run.command()).1 else { panic!() };
        assert!(text.to_lowercase().contains("open notes") && !text.contains("milk"), "{text:?}");
    }

    #[test]
    fn cancel_drops_the_command() {
        let Some(m) = models_ready() else { return };
        let speech = say("Open Notes and save buy milk.");
        let run = start(&m, EarsConfig::default(), speech.clone());
        run.ears.listen_now();
        let mut events = run.feed_until(speech.len() / 2);
        assert!(events.iter().any(|h| matches!(h, Heard::Level(_))), "it was listening");
        run.ears.cancel();
        run.play();
        std::thread::sleep(Duration::from_millis(300));
        events.extend(run.events.try_iter().map(|(_, h)| h));
        assert!(!events.iter().any(|h| matches!(h, Heard::Final(_) | Heard::Nothing)), "{events:?}");
        assert!(!run.open.load(Ordering::SeqCst), "the microphone closes");
    }

    #[test]
    fn while_alfred_speaks_only_the_wake_word_counts() {
        let Some(m) = models_ready() else { return };
        let mut audio = say("Open Notes and save buy milk.");
        audio.extend(silence(1.0));
        let barge = audio.len();
        audio.extend(say("Alfred, what time is it?"));
        audio.extend(silence(2.0));
        let run = start(&m, wake_on(), audio);
        run.feed_until(0);
        run.ears.set_speaking(true);
        assert_eq!(run.feed_until(barge), vec![], "speech without the wake word starts nothing");
        run.play();
        let events = run.command();
        assert_eq!(events[0].1, Heard::Wake);
        // Moonshine hears "What time is it?" or, from some renderings, "What tone is it?".
        assert!(matches!(&last_text(&events).1, Heard::Final(t) if t.to_lowercase().contains("is it")), "{events:?}");
    }

    /// Someone across the room, `db_below` dB under `speaker`.
    fn far_voice(models: &Path, speaker: &[f32], db_below: f32) -> Vec<f32> {
        let sp = crate::Speaker::load(models, "af_heart", 1.0).unwrap();
        let a = sp.synthesize("The game starts at eight tonight, so we should leave early.").unwrap();
        let far = crate::capture::resample(&a.samples, a.sample_rate, 16_000);
        let gain = speech_rms(speaker) / speech_rms(&far) * 10f32.powf(-db_below / 20.0);
        far.iter().map(|s| s * gain).collect()
    }

    fn levels(events: &[(f32, Heard)], from: f32, to: f32) -> Vec<f32> {
        events
            .iter()
            .filter(|(t, _)| (from..to).contains(t))
            .filter_map(|(_, h)| if let Heard::Level(l) = h { Some(*l) } else { None })
            .collect()
    }

    #[test]
    fn a_command_ends_in_steady_noise() {
        let Some(m) = models_ready() else { return };
        let speech = say("Open Notes and save buy milk.");
        for (kind, seed) in [("pink", 1u32), ("white", 2)] {
            let mut clip = silence(1.0);
            clip.extend(&speech);
            clip.extend(silence(4.0));
            // The noise is 10 dB under the speech and goes on after it.
            let noise = if kind == "pink" { pink(clip.len(), seed) } else { white(clip.len(), seed) };
            let audio = mix_noise(&clip, &speech, &noise, 10.0);
            let run = start(&m, EarsConfig::default(), audio);
            run.play();
            run.ears.listen_now();
            let events = run.command();
            let (at, end) = last_text(&events);
            let Heard::Final(text) = end else { panic!("{kind}: {events:?}") };
            let latency = at - (1.0 + sound_ends(&speech));
            eprintln!("{kind} noise 10 dB under: {text:?}, ended {latency:.2} s after the speech");
            assert!(text.to_lowercase().contains("open notes"), "{kind}: {text:?}");
            assert!(
                latency > 0.3 && latency < EarsConfig::default().silence_slow_secs + 0.3,
                "{kind}: ended {latency:.2} s after the speech"
            );
            // The orb follows the voice, not the noise.
            let (noise_only, voice) = (levels(&events, 0.5, 0.95), levels(&events, 1.2, 1.0 + sound_ends(&speech)));
            let peak = voice.iter().copied().fold(0.0, f32::max);
            assert!(noise_only.iter().all(|l| *l < 0.35) && peak > 0.6, "{kind}: noise {noise_only:?}, voice peak {peak}");
        }
    }

    #[test]
    fn a_far_voice_does_not_hold_the_command_open() {
        let Some(m) = models_ready() else { return };
        let speech = say("Open Notes.");
        let mut audio = silence(0.5);
        audio.extend(&speech);
        audio.extend(far_voice(&m, &speech, 25.0));
        audio.extend(silence(2.0));
        let audio = mix_noise(&audio, &speech, &pink(audio.len(), 3), 20.0);
        let run = start(&m, EarsConfig::default(), audio);
        run.play();
        run.ears.listen_now();
        let events = run.command();
        let (at, end) = last_text(&events);
        let Heard::Final(text) = end else { panic!("{end:?}") };
        let latency = at - (0.5 + sound_ends(&speech));
        eprintln!("a far voice after the command: {text:?}, ended {latency:.2} s after the speaker");
        // Moonshine sometimes hears "Open nuts" in this noise; the far voice's words never get in.
        let text = text.to_lowercase();
        assert!(text.contains("open") && !["game", "eight", "tonight", "leave"].iter().any(|w| text.contains(w)), "{text:?}");
        assert!(latency < EarsConfig::default().silence_slow_secs + 0.3, "ended {latency:.2} s after the speaker");
    }

    #[test]
    fn after_alfred_noise_and_a_far_voice_start_nothing() {
        let Some(m) = models_ready() else { return };
        let alfred = say("Alfred.");
        let mut audio = silence(0.5);
        audio.extend(&alfred);
        audio.extend(silence(0.5));
        audio.extend(far_voice(&m, &alfred, 25.0));
        audio.extend(silence(3.0));
        let audio = mix_noise(&audio, &alfred, &pink(audio.len(), 4), 20.0);
        let run = start(&m, wake_on(), audio);
        run.play();
        let events = run.command();
        assert_eq!(events[0].1, Heard::Wake);
        let (at, end) = last_text(&events);
        assert_eq!(end, Heard::Nothing, "{events:?}");
        assert!(at > 4.0, "the command waited for its speaker: {at} s");
    }

    struct Broken;

    impl Source for Broken {
        fn open(&mut self) -> Result<(), String> {
            Err("microphone access denied".into())
        }
        fn close(&mut self) {}
        fn is_open(&self) -> bool {
            false
        }
        fn read(&mut self, _: Duration) -> Result<Option<Vec<f32>>, String> {
            Ok(None)
        }
    }

    #[test]
    fn a_microphone_that_fails_reports_an_error() {
        let Some(m) = models_ready() else { return };
        let (tx, rx) = mpsc::channel();
        let ears = Ears::start_with(&m, EarsConfig::default(), || Broken, move |h| tx.send(h).unwrap()).unwrap();
        ears.listen_now();
        assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), Heard::Error("microphone access denied".into()));
        ears.listen_now();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            Heard::Error("microphone access denied".into()),
            "a new click tries again"
        );
    }

    #[test]
    fn ears_can_be_shared_with_the_ui() {
        fn shareable<T: Send + Sync>() {}
        shareable::<Ears>();
    }

    /// Run with `cargo test --release -p alfred-voice -- --ignored --nocapture measure`.
    #[test]
    #[ignore]
    fn measure_ears() {
        let Some(m) = models_ready() else { return };
        let t = Instant::now();
        let ears = Ears::start_with(&m, wake_on(), || Broken, |_| {}).unwrap();
        eprintln!("ears: models load in {:.2} s", t.elapsed().as_secs_f32());
        drop(ears);
        let mut vad = Vad::load(&m, VadConfig::default()).unwrap();
        let speech = say("Remind me tomorrow morning to call the bank about the mortgage and then to buy milk, bread and eggs.");
        let t = Instant::now();
        for _ in 0..10 {
            vad.reset();
            vad.accept(&speech);
        }
        eprintln!("vad: {:.1} ms of CPU per second of audio", t.elapsed().as_secs_f32() * 1000.0 / (10.0 * secs(speech.len())));
        let mut denoiser = Denoiser::load(&m).unwrap();
        let t = Instant::now();
        for _ in 0..10 {
            denoiser.reset();
            for c in speech.chunks(170) {
                denoiser.run(c);
            }
        }
        eprintln!("gtcrn: {:.1} ms of CPU per second of audio", t.elapsed().as_secs_f32() * 1000.0 / (10.0 * secs(speech.len())));
        let listener = Listener::load(&m).unwrap();
        for len in [2.0f32, 5.0] {
            let clip = &speech[..samples(len).min(speech.len())];
            let t = Instant::now();
            for _ in 0..5 {
                listener.transcribe(clip, SAMPLE_RATE);
            }
            eprintln!("moonshine: {:.0} ms per partial of {len} s", t.elapsed().as_secs_f32() * 1000.0 / 5.0);
        }
    }

    #[test]
    fn missing_models_fail_to_start() {
        let empty = std::env::temp_dir().join(format!("alfred-no-models-{}", std::process::id()));
        let err = Ears::start(&empty, EarsConfig::default(), |_| {}).err().unwrap();
        assert!(err.contains("alfred setup"), "{err}");
    }
}
