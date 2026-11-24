//! The panel: a launcher-style window summoned by a global shortcut, with the orb, a text field, the
//! conversation and the actions as chips.

pub mod orb;
mod scenes;
#[cfg(test)]
mod tests;
mod view;

use std::time::Duration;

use alfred_core::config::{self, Config};
use alfred_core::{Action, Turn as AgentTurn};
use rune::{
    Align, App, AppHandle, AppSpec, DisplayTarget, Key, KeyEvent, MicPermission, OverlayPlacement, RecordFormat, Recorder, RunOptions,
    ScreenEdge, TextEdit, Tx, WindowOptions, WindowSpec,
};

use crate::workers::{self, BrainJob, Links, VoiceJob};

pub const SIZE: (f32, f32) = (560.0, 640.0);
const MAX_TURNS: usize = 3;
/// A recording is "voice" above this meter level (0 to 1).
const VOICE_LEVEL: f32 = 0.16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Idle,
    Listening,
    Transcribing,
    Thinking,
    Speaking,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub you: String,
    pub alfred: String,
    pub chips: Vec<Chip>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chip {
    pub label: String,
    pub state: ChipState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipState {
    Running,
    Done,
    Failed,
}

struct Listen {
    recorder: Recorder,
    started: f64,
    last_voice: f64,
    heard: bool,
}

struct Speech {
    start: f64,
    envelope: Vec<f32>,
}

pub struct Ui {
    pub input: TextEdit,
    pub mode: Mode,
    pub turns: Vec<Turn>,
    pub pending: Option<Action>,
    /// A one-off message that replaces the caption (a missing permission, nothing heard).
    pub notice: Option<String>,
    pub clock: f64,
    /// What the orb ripples with, 0 to 1.
    pub level: f32,
    pub model_label: String,
    pub voice_ready: bool,
    pub ears_ready: bool,
    pub shortcut: String,
    attempt: u64,
    listen: Option<Listen>,
    speech: Option<Speech>,
    links: Option<Links>,
}

impl Ui {
    /// The state headless snapshots and tests start from: no threads, no files.
    pub fn demo() -> Self {
        let mut input = TextEdit::new("");
        input.focused = true;
        Self {
            input,
            mode: Mode::Idle,
            turns: Vec::new(),
            pending: None,
            notice: None,
            clock: 0.0,
            level: 0.0,
            model_label: "qwen3:0.6b via Ollama".into(),
            voice_ready: true,
            ears_ready: true,
            shortcut: Config::default().ui.shortcut,
            attempt: 0,
            listen: None,
            speech: None,
            links: None,
        }
    }

    fn live(links: Links) -> Self {
        Self {
            shortcut: links.config.ui.shortcut.clone(),
            model_label: String::new(),
            voice_ready: false,
            ears_ready: false,
            links: Some(links),
            ..Self::demo()
        }
    }

    pub fn caption(&self) -> String {
        if let Some(n) = &self.notice {
            return n.clone();
        }
        match self.mode {
            Mode::Idle => "At your service.".into(),
            Mode::Listening => "Listening…".into(),
            Mode::Transcribing => "One moment…".into(),
            Mode::Thinking => "Thinking…".into(),
            Mode::Speaking => "Speaking…".into(),
        }
    }

    fn config(&self) -> Option<&Config> {
        self.links.as_ref().map(|l| &l.config)
    }

    // ---- user actions ---------------------------------------------------------------------------

    pub fn submit(&mut self) {
        let text = self.input.text.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.input.clear();
        self.ask(text);
    }

    pub fn ask(&mut self, text: String) {
        self.stop_speaking();
        self.attempt += 1;
        self.notice = None;
        self.pending = None;
        self.turns.push(Turn { you: text.clone(), alfred: String::new(), chips: Vec::new() });
        if self.turns.len() > MAX_TURNS {
            self.turns.remove(0);
        }
        self.mode = Mode::Thinking;
        if let Some(l) = &self.links {
            l.brain.send(BrainJob::Ask { attempt: self.attempt, text }).ok();
        }
    }

    pub fn confirm(&mut self, yes: bool) {
        if self.pending.take().is_none() {
            return;
        }
        self.attempt += 1;
        self.mode = Mode::Thinking;
        if let Some(l) = &self.links {
            l.brain.send(BrainJob::Confirm { attempt: self.attempt, yes }).ok();
        }
    }

    pub fn toggle_listening(&mut self, tx: &mut Tx) {
        if self.mode == Mode::Listening { self.stop_listening(tx) } else { self.start_listening(tx) }
    }

    fn start_listening(&mut self, tx: &mut Tx) {
        if !self.ears_ready {
            self.notice = Some("Dictation isn't ready yet: run `alfred setup` to download it.".into());
            return;
        }
        if matches!(Recorder::permission(), MicPermission::Denied | MicPermission::Restricted) {
            self.notice = Some("Microphone access is off: System Settings → Privacy & Security → Microphone.".into());
            return;
        }
        self.stop_speaking();
        self.attempt += 1;
        let path = config::cache_dir().join(format!("heard-{}-{}.wav", std::process::id(), self.attempt));
        std::fs::create_dir_all(config::cache_dir()).ok();
        let recorder = Recorder::to_file(path).format(RecordFormat::Wav).sample_rate(16_000).channels(1);
        recorder.start();
        self.listen = Some(Listen { recorder, started: self.clock, last_voice: self.clock, heard: false });
        self.mode = Mode::Listening;
        self.notice = None;
        tx.wake_after(Recorder::METER_INTERVAL);
    }

    fn stop_listening(&mut self, _tx: &mut Tx) {
        let Some(l) = self.listen.take() else { return };
        match l.recorder.stop() {
            Ok(recording) if l.heard => {
                self.mode = Mode::Transcribing;
                if let Some(links) = &self.links {
                    links.voice.send(VoiceJob::Transcribe { attempt: self.attempt, path: recording.path }).ok();
                }
            }
            Ok(recording) => {
                std::fs::remove_file(&recording.path).ok();
                self.mode = Mode::Idle;
                self.notice = Some("I didn't hear anything. Click the orb and speak.".into());
            }
            Err(e) => {
                self.mode = Mode::Idle;
                self.notice = Some(format!("The microphone stopped: {e}"));
            }
        }
    }

    fn cancel_listening(&mut self) {
        if let Some(l) = self.listen.take() {
            l.recorder.cancel();
        }
        self.mode = Mode::Idle;
    }

    pub fn stop_speaking(&mut self) {
        if let Some(l) = &self.links {
            l.playback.stop();
        }
        self.speech = None;
        if self.mode == Mode::Speaking {
            self.mode = Mode::Idle;
        }
    }

    pub fn clear(&mut self) {
        self.turns.clear();
        self.pending = None;
        self.notice = None;
    }

    fn toggle_visible(&mut self) {
        let Some(l) = &self.links else { return };
        if l.me.is_visible() {
            l.me.hide();
        } else {
            summon(&l.me);
        }
    }

    fn on_focus(&mut self, focused: bool, tx: &mut Tx) {
        let hide = self.config().is_some_and(|c| c.ui.hide_on_blur);
        if !focused && hide && self.mode != Mode::Listening {
            tx.close_window();
        }
    }

    // ---- results from the workers ---------------------------------------------------------------

    pub fn on_thinking(&mut self, attempt: u64) {
        if attempt == self.attempt {
            self.mode = Mode::Thinking;
        }
    }

    pub fn on_acting(&mut self, attempt: u64, label: String) {
        if attempt != self.attempt {
            return;
        }
        if let Some(t) = self.turns.last_mut() {
            t.chips.push(Chip { label, state: ChipState::Running });
        }
    }

    pub fn on_acted(&mut self, attempt: u64, ok: bool) {
        if attempt != self.attempt {
            return;
        }
        if let Some(chip) = self.turns.last_mut().and_then(|t| t.chips.iter_mut().rev().find(|c| c.state == ChipState::Running)) {
            chip.state = if ok { ChipState::Done } else { ChipState::Failed };
        }
    }

    pub fn on_turn(&mut self, attempt: u64, turn: AgentTurn) {
        if attempt != self.attempt {
            return;
        }
        match self.turns.last_mut() {
            Some(t) if t.alfred.is_empty() => t.alfred = turn.reply.clone(),
            _ => self.turns.push(Turn { you: String::new(), alfred: turn.reply.clone(), chips: Vec::new() }),
        }
        self.pending = turn.pending;
        let speak = self.config().is_some_and(|c| c.voice.speak) && !turn.reply.trim().is_empty();
        if speak && let Some(l) = &self.links {
            self.mode = Mode::Speaking;
            l.voice.send(VoiceJob::Speak { attempt, text: turn.reply }).ok();
        } else {
            self.mode = Mode::Idle;
        }
    }

    pub fn on_voice_ready(&mut self, voice: bool, ears: bool) {
        self.voice_ready = voice;
        self.ears_ready = ears;
    }

    pub fn on_sentence(&mut self, attempt: u64, envelope: Vec<f32>, _seconds: f32, tx: &mut Tx) {
        if attempt == self.attempt && self.mode == Mode::Speaking {
            self.speech = Some(Speech { start: tx.clock(), envelope });
        }
    }

    pub fn on_spoken(&mut self, attempt: u64) {
        if attempt == self.attempt && self.mode == Mode::Speaking {
            self.mode = Mode::Idle;
            self.speech = None;
        }
    }

    pub fn on_heard(&mut self, attempt: u64, text: String) {
        if attempt != self.attempt {
            return;
        }
        if text.trim().is_empty() {
            self.mode = Mode::Idle;
            self.notice = Some("I didn't catch that. Try again, or type it.".into());
        } else {
            self.ask(text);
        }
    }
}

/// Bring the panel up near the top of the screen the pointer is on.
fn summon(me: &AppHandle<Ui>) {
    me.place(
        DisplayTarget::UnderPointer,
        OverlayPlacement::Edge { edge: ScreenEdge::Top, margin: 110.0, align: Align::Center, inset: 0.0 },
    );
    me.show();
    me.focus();
}

/// Each frame: follow the microphone or the speech envelope, and end dictation on silence.
fn poll(s: &mut Ui, t: f64, tx: &mut Tx) -> bool {
    s.clock = t;
    let (silence, longest) = s.config().map(|c| (c.voice.silence_secs as f64, c.voice.max_listen_secs as f64)).unwrap_or((1.4, 20.0));
    let target = match s.mode {
        Mode::Listening => {
            let mut stop = false;
            let mut level = 0.0;
            if let Some(l) = s.listen.as_mut() {
                level = l.recorder.level().meter();
                if level > VOICE_LEVEL {
                    l.heard = true;
                    l.last_voice = t;
                }
                stop = (l.heard && t - l.last_voice > silence) || t - l.started > longest;
            }
            if stop {
                s.stop_listening(tx);
            }
            level
        }
        Mode::Speaking => match &s.speech {
            Some(sp) if !sp.envelope.is_empty() => sp.envelope.get(((t - sp.start) * 30.0).max(0.0) as usize).copied().unwrap_or(0.0),
            _ => 0.45 + 0.25 * ((t * 7.0).sin() as f32),
        },
        Mode::Thinking | Mode::Transcribing => 0.35,
        Mode::Idle => 0.0,
    };
    // Rise fast, fall slowly, like a VU meter.
    let k = if target > s.level { 0.45 } else { 0.12 };
    s.level += (target - s.level) * k;
    true
}

fn on_key(s: &mut Ui, ev: &KeyEvent, tx: &mut Tx) -> bool {
    match ev.key {
        Key::Escape => {
            match s.mode {
                Mode::Speaking => s.stop_speaking(),
                Mode::Listening => s.cancel_listening(),
                _ if s.pending.is_some() => s.confirm(false),
                _ => tx.close_window(),
            }
            true
        }
        Key::Command('q') => {
            tx.quit();
            true
        }
        Key::Command('l') => {
            s.toggle_listening(tx);
            true
        }
        Key::Command('k') => {
            s.clear();
            true
        }
        _ => false,
    }
}

fn spec(init: fn() -> Ui) -> AppSpec<Ui> {
    AppSpec {
        scenes: scenes::SCENES,
        poll: Some(poll),
        on_key: Some(on_key),
        min_size: Some((420.0, 480.0)),
        ..AppSpec::new("Alfred", SIZE, init, view::view)
    }
}

/// Flags that ask Rune for a headless render instead of the app.
const TOOLING: &[&str] = &["--snap", "--tree", "--list-scenes", "--script", "--video", "--frames", "--strip", "--html", "--list-windows"];

pub fn run(args: Vec<String>) {
    if args.iter().any(|a| TOOLING.contains(&a.as_str())) {
        rune::run(spec(Ui::demo));
        return;
    }
    let config = crate::cli::load_config().unwrap_or_else(|e| {
        eprintln!("alfred: {e}; using the defaults");
        Config::default()
    });
    let app = App::new(spec(|| unreachable!("the app's state comes from init"))).windowless();
    let me = app.handle();
    let links = workers::spawn(config.clone(), me.clone());
    let shortcut = config.ui.shortcut.clone();
    let app = app
        .init(move || Ui::live(links))
        .global_shortcut(&shortcut, |s, _| s.toggle_visible())
        .on_focus(|s, focused, tx| s.on_focus(focused, tx));
    if !args.iter().any(|a| a == "--background") {
        let first = me.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            summon(&first);
        });
    }
    rune::run_windows_with_args(RunOptions { accessory: true }, Vec::<String>::new(), move |_| {
        vec![WindowSpec::from_app("alfred", app).configured_options(WindowOptions::launcher_panel())]
    });
}
