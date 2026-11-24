//! Alfred's window: a compact, transparent assistant that floats above other windows, with the green
//! fluid orb, a field to type or talk, and the latest exchange. A global shortcut, the Dock icon or ⌘Tab
//! bring it forward.

mod gfx;
pub mod orb;
mod scenes;
#[cfg(test)]
mod tests;
mod view;

use std::time::Duration;

use alfred_core::config::Config;
use alfred_core::{Action, Turn as AgentTurn};
use rune::{
    Align, App, AppHandle, AppSpec, DisplayTarget, Key, KeyEvent, MacWindowOptions, MicPermission, OverlayPlacement, Recorder, RunOptions,
    ScreenEdge, TextEdit, Tx, WindowOptions, WindowSpec,
};

use crate::workers::{self, BrainJob, Links, VoiceJob};
use alfred_voice::Heard;

pub const SIZE: (f32, f32) = (300.0, 480.0);
const MAX_TURNS: usize = 3;

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
    /// Alfred listens for "Alfred" all the time: the mic button then turns that listening off and on.
    pub wake_word: bool,
    /// The user turned listening off: the microphone is closed and nothing is heard.
    pub muted: bool,
    /// Alfred is recording its runs (the user turned it on): the footer says so.
    pub recording: bool,
    /// How long the conversation stays open after Alfred answers a spoken request, in seconds.
    pub follow_secs: f32,
    /// The follow-up window is open: what is said next needs no "Alfred".
    pub following: bool,
    /// The request under way was spoken, so the conversation stays open after its answer.
    spoken_turn: bool,
    /// The listening under way began in the follow-up window: if it was only noise, nothing is said.
    follow_listening: bool,
    pub shortcut: String,
    /// The panel has had the keyboard since it was last shown. Only then does losing it hide the panel:
    /// shown while another app is in front (at launch, from Finder), it never gets focus, and hiding on
    /// that first blur would take it away before anyone saw it.
    had_focus: bool,
    /// The panel has the keyboard now.
    focused: bool,
    /// The model is thinking a hard question through.
    reasoning: bool,
    /// What was in the field when listening began: the live text follows it.
    typed: String,
    /// The microphone's level while listening, 0 to 1, as the ears report it.
    mic_level: f32,
    attempt: u64,
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
            wake_word: false,
            recording: false,
            follow_secs: 30.0,
            following: false,
            spoken_turn: false,
            follow_listening: false,
            muted: false,
            shortcut: Config::default().ui.shortcut,
            had_focus: false,
            focused: false,
            reasoning: false,
            typed: String::new(),
            mic_level: 0.0,
            attempt: 0,
            speech: None,
            links: None,
        }
    }

    fn live(links: Links) -> Self {
        Self {
            shortcut: links.config.ui.shortcut.clone(),
            wake_word: links.config.voice.wake_word,
            recording: links.config.recording.enabled,
            follow_secs: links.config.voice.follow_up_secs,
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
            Mode::Idle if self.muted => "Not listening.".into(),
            Mode::Idle if self.following => "I'm listening. “Alfred, stop” to end.".into(),
            Mode::Idle => "At your service.".into(),
            Mode::Listening => "Listening…".into(),
            Mode::Transcribing => "One moment…".into(),
            Mode::Thinking if self.reasoning => "Thinking it through…".into(),
            Mode::Thinking => "Thinking…".into(),
            Mode::Speaking => "Speaking…".into(),
        }
    }

    fn config(&self) -> Option<&Config> {
        self.links.as_ref().map(|l| &l.config)
    }

    /// Answers are spoken: then they aren't written too, which keeps Alfred small.
    pub fn speaks_aloud(&self) -> bool {
        self.voice_ready && self.config().is_none_or(|c| c.voice.speak)
    }

    // ---- user actions ---------------------------------------------------------------------------

    pub fn submit(&mut self) {
        let text = self.input.text.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.input.clear();
        // A typed request isn't a spoken conversation: nothing stays open after its answer.
        self.spoken_turn = false;
        self.ask(text);
    }

    /// Alfred has answered a spoken request: keep the conversation open, so the next words need no
    /// "Alfred". Every answer starts the time again.
    fn open_follow_up(&mut self) {
        if !self.spoken_turn || self.muted || self.follow_secs <= 0.0 {
            return;
        }
        self.following = true;
        if let Some(e) = self.ears() {
            e.follow_up(self.follow_secs);
        }
    }

    /// "Alfred, stop": stop talking, close the conversation, and wait for "Alfred" again.
    fn end_conversation(&mut self) {
        self.stop_speaking();
        self.spoken_turn = false;
        self.following = false;
        self.follow_listening = false;
        if let Some(e) = self.ears() {
            e.follow_up(0.0);
        }
        self.restore_typed();
        self.mode = Mode::Idle;
        self.notice = None;
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
            // Open the speakers while the model thinks, so the answer is heard the moment it's rendered.
            l.playback.warm();
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
            l.playback.warm();
            l.brain.send(BrainJob::Confirm { attempt: self.attempt, yes }).ok();
        }
    }

    /// The mic button. With "Alfred" listened for all the time, it turns that listening off and back on;
    /// otherwise it starts and stops dictation.
    pub fn press_mic(&mut self) {
        if self.wake_word { self.set_muted(!self.muted) } else { self.toggle_listening() }
    }

    /// Turn listening off (the microphone closes; nothing is heard, not even "Alfred") or back on.
    pub fn set_muted(&mut self, muted: bool) {
        if muted && matches!(self.mode, Mode::Listening | Mode::Transcribing) {
            self.cancel_listening();
        }
        if muted {
            // Muting closes the conversation too (the ears close their window themselves).
            self.following = false;
            self.spoken_turn = false;
        }
        self.muted = muted;
        self.notice = None;
        if let Some(e) = self.ears() {
            e.set_muted(muted);
        }
    }

    /// ⌘L, or the mic button without the wake word: start dictating, or finish.
    pub fn toggle_listening(&mut self) {
        if self.mode == Mode::Listening { self.stop_listening() } else { self.start_listening() }
    }

    fn start_listening(&mut self) {
        if self.muted {
            self.notice = Some("Listening is off: click the mic to turn it back on.".into());
            return;
        }
        if !self.ears_ready {
            self.notice = Some("Dictation isn't ready yet: run `alfred setup` to download it.".into());
            return;
        }
        if matches!(Recorder::permission(), MicPermission::Denied | MicPermission::Restricted) {
            self.notice = Some("Microphone access is off: System Settings → Privacy & Security → Microphone.".into());
            return;
        }
        self.stop_speaking();
        self.begin_listening();
        if let Some(e) = self.ears() {
            e.listen_now();
        }
    }

    /// Listening has begun (a click, ⌘L or "Alfred"): a new request, with the field's text kept in front.
    fn begin_listening(&mut self) {
        // A request is coming: open the speakers now, while you speak.
        if let Some(l) = &self.links {
            l.playback.warm();
        }
        self.attempt += 1;
        self.typed = self.input.text.trim().to_string();
        self.mic_level = 0.0;
        self.mode = Mode::Listening;
        self.notice = None;
    }

    /// The user is done (a second click): the ears transcribe what they heard.
    fn stop_listening(&mut self) {
        self.mode = Mode::Transcribing;
        if let Some(e) = self.ears() {
            e.finish();
        }
    }

    fn cancel_listening(&mut self) {
        if let Some(e) = self.ears() {
            e.cancel();
        }
        self.restore_typed();
        self.mode = Mode::Idle;
    }

    fn ears(&self) -> Option<&alfred_voice::Ears> {
        self.links.as_ref().and_then(|l| l.ears.get())
    }

    /// Put back what was typed before listening, dropping the live text.
    fn restore_typed(&mut self) {
        let typed = std::mem::take(&mut self.typed);
        self.input.replace_text(typed);
    }

    /// What the ears report: "Alfred" was said, the voice's level, the words so far, the whole command.
    pub fn on_ears(&mut self, heard: Heard) {
        let listening = matches!(self.mode, Mode::Listening | Mode::Transcribing);
        match heard {
            Heard::Wake => {
                // Like "Hey Siri": stop talking, come forward, chime, and listen.
                self.stop_speaking();
                self.begin_listening();
                if let Some(l) = &self.links
                    && !(l.me.is_visible() && self.focused)
                {
                    summon(&l.me, false);
                }
                std::process::Command::new("afplay").arg("/System/Library/Sounds/Tink.aiff").spawn().ok();
                self.follow_listening = false;
            }
            // Speech in the follow-up window: listen, with no chime and without coming forward.
            Heard::FollowUp => {
                self.begin_listening();
                self.follow_listening = true;
            }
            Heard::FollowEnded => self.following = false,
            Heard::Level(x) => self.mic_level = x,
            Heard::Partial(text) => self.on_partial(self.attempt, text),
            Heard::Final(text) if listening => self.on_heard(self.attempt, text),
            // A sound in the follow-up window that held no words: back to waiting, without a word about it.
            Heard::Nothing if listening && self.follow_listening => {
                self.follow_listening = false;
                self.restore_typed();
                self.mode = Mode::Idle;
            }
            Heard::Nothing if listening => {
                self.restore_typed();
                self.mode = Mode::Idle;
                self.notice = Some(
                    if self.wake_word {
                        "I didn't hear anything. Say “Alfred” and speak."
                    } else {
                        "I didn't hear anything. Click the mic and speak."
                    }
                    .into(),
                );
            }
            Heard::Error(e) => {
                if listening {
                    self.restore_typed();
                    self.mode = Mode::Idle;
                }
                self.notice = Some(format!("The microphone: {e}"));
            }
            Heard::Final(_) | Heard::Nothing => {}
        }
    }

    pub fn stop_speaking(&mut self) {
        if let Some(l) = &self.links {
            l.playback.stop();
        }
        if let Some(e) = self.ears() {
            e.set_speaking(false);
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

    /// The shortcut: put the panel away when it is in front, else bring it forward (also when it sits
    /// behind another app's windows).
    fn toggle_visible(&mut self) {
        let Some(l) = &self.links else { return };
        if l.me.is_visible() && self.focused {
            self.hide_panel();
        } else {
            self.had_focus = false;
            summon(&l.me, false);
        }
    }

    /// A click on the Dock icon (or opening Alfred again): bring the panel back if it was put away.
    /// When it already shows, macOS brings it forward itself.
    fn on_reopen(&mut self, has_visible_windows: bool) -> bool {
        let Some(l) = &self.links else { return false };
        if has_visible_windows && l.me.is_visible() {
            return false;
        }
        self.had_focus = false;
        summon(&l.me, false);
        true
    }

    fn on_focus(&mut self, focused: bool) {
        self.focused = focused;
        if focused {
            self.had_focus = true;
            return;
        }
        let hide = self.config().is_some_and(|c| c.ui.hide_on_blur);
        if self.had_focus && hide && self.mode != Mode::Listening {
            self.hide_panel();
        }
    }

    /// Put the panel away, keeping its window and the conversation. (Closing the window would end the
    /// app's run in Rune, and the shortcut with it: the panel could never come back.)
    fn hide_panel(&mut self) {
        self.had_focus = false;
        if let Some(l) = &self.links {
            l.me.hide();
        }
    }

    // ---- results from the workers ---------------------------------------------------------------

    pub fn on_thinking(&mut self, attempt: u64) {
        if attempt == self.attempt {
            self.mode = Mode::Thinking;
            self.reasoning = false;
        }
    }

    /// A hard question: the caption says Alfred is thinking it through, which takes a few seconds more.
    pub fn on_reasoning(&mut self, attempt: u64) {
        if attempt == self.attempt {
            self.mode = Mode::Thinking;
            self.reasoning = true;
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
        self.reasoning = false;
        match self.turns.last_mut() {
            Some(t) if t.alfred.is_empty() => t.alfred = turn.reply.clone(),
            _ => self.turns.push(Turn { you: String::new(), alfred: turn.reply.clone(), chips: Vec::new() }),
        }
        self.pending = turn.pending;
        let speak = self.config().is_some_and(|c| c.voice.speak) && !turn.reply.trim().is_empty();
        if speak && let Some(l) = &self.links {
            self.mode = Mode::Speaking;
            l.voice.send(VoiceJob::Speak { attempt, text: turn.reply }).ok();
            // While Alfred talks, only "Alfred" counts: it stops the speech and listens.
            if let Some(e) = l.ears.get() {
                e.set_speaking(true);
            }
        } else {
            self.mode = Mode::Idle;
            self.open_follow_up();
        }
    }

    pub fn on_voice_ready(&mut self, voice: bool) {
        self.voice_ready = voice;
    }

    pub fn on_ears_ready(&mut self, ready: bool) {
        self.ears_ready = ready;
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
        if let Some(e) = self.ears() {
            e.set_speaking(false);
        }
        if attempt == self.attempt {
            self.open_follow_up();
        }
    }

    pub fn on_heard(&mut self, attempt: u64, text: String) {
        if attempt != self.attempt {
            return;
        }
        if text.trim().is_empty() {
            self.restore_typed();
            self.mode = Mode::Idle;
            self.notice = Some("I didn't catch that. Try again, or type it.".into());
        } else if self.typed.is_empty() && ends_conversation(&text) {
            self.end_conversation();
        } else {
            let said = join_said(&std::mem::take(&mut self.typed), &text);
            self.input.clear();
            self.follow_listening = false;
            self.spoken_turn = true;
            self.ask(said);
        }
    }

    /// The live text: what has been heard so far, shown in the field after anything typed before.
    pub fn on_partial(&mut self, attempt: u64, text: String) {
        let live = matches!(self.mode, Mode::Listening | Mode::Transcribing);
        if attempt == self.attempt && live && !text.trim().is_empty() {
            self.input.replace_text(join_said(&self.typed, &text));
        }
    }
}

/// Typed words and spoken ones, as one request.
fn join_said(typed: &str, heard: &str) -> String {
    [typed.trim(), heard.trim()].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ")
}

/// Words that end a spoken conversation, said after "Alfred" or in the follow-up window.
const STOP_PHRASES: &[&str] = &[
    "stop",
    "stop it",
    "stop now",
    "stop listening",
    "stop alfred",
    "thats all",
    "that is all",
    "that will be all",
    "thatll be all",
    "goodbye",
    "good bye",
    "bye",
    "arrete",
    "arrete toi",
    "cest tout",
];

/// Whether what was heard asks Alfred to stop and close the conversation ("Stop.", "That's all.").
pub fn ends_conversation(heard: &str) -> bool {
    let said = alfred_core::catalog::fold(&heard.replace(['\'', '’'], ""));
    STOP_PHRASES.contains(&said.trim())
}

/// Bring Alfred forward, taking the keyboard (`open` also creates the window the first time). At launch it
/// goes to the bottom-right corner of the main display; after that it stays where the user dragged it.
fn summon(me: &AppHandle<Ui>, place: bool) {
    if place {
        me.place(DisplayTarget::Primary, OverlayPlacement::Edge { edge: ScreenEdge::Bottom, margin: 24.0, align: Align::End, inset: 24.0 });
    }
    me.open();
}

/// Each frame: follow the microphone or the speech envelope. (The ears decide when a command ends.)
fn poll(s: &mut Ui, t: f64, _tx: &mut Tx) -> bool {
    s.clock = t;
    let target = match s.mode {
        Mode::Listening => s.mic_level,
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
                _ => s.hide_panel(),
            }
            true
        }
        Key::Command('q') => {
            tx.quit();
            true
        }
        Key::Command('l') => {
            s.toggle_listening();
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
        min_size: Some((260.0, 380.0)),
        ..AppSpec::new("Alfred", SIZE, init, view::view)
    }
}

/// AppKit's NSWindowCollectionBehavior: can join all Spaces (1 << 0), stationary in Mission Control
/// (1 << 4), shown beside full-screen apps (1 << 8).
const EVERY_SPACE: usize = (1 << 0) | (1 << 4) | (1 << 8);

/// Flags that ask Rune for a headless render instead of the app.
const TOOLING: &[&str] = &["--snap", "--tree", "--list-scenes", "--script", "--video", "--frames", "--strip", "--html", "--list-windows"];

pub fn run(args: Vec<String>) {
    if args.iter().any(|a| TOOLING.contains(&a.as_str())) {
        // Rendered as the real window is: borderless and transparent, so snapshots show what the desktop sees.
        App::new(spec(Ui::demo))
            .window_options(WindowOptions { borderless: true, transparent: true, ..Default::default() })
            .compositor(gfx::Gfx::default())
            .run();
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
    // keep_running: if the window is ever closed (rather than hidden), the app and its shortcut stay.
    let app = app
        .init(move || Ui::live(links))
        .keep_running()
        .global_shortcut(&shortcut, |s, _| s.toggle_visible())
        .on_reopen(|s, has_visible, _| s.on_reopen(has_visible))
        .on_focus(|s, focused, _| s.on_focus(focused))
        .compositor(gfx::Gfx::default());
    if !args.iter().any(|a| a == "--background") {
        let first = me.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            summon(&first, true);
        });
    }
    // A regular app (Dock icon, ⌘Tab) whose window floats above the others with nothing but its own
    // content showing: borderless and transparent. (A launcher panel never makes Alfred the active app, so
    // switching to Alfred couldn't bring it forward.)
    // No system shadow: it would trace the orb and the text with a dark outline. On every Space, steady
    // in Mission Control, and over full-screen apps, so Alfred follows you from desktop to desktop.
    let window =
        WindowOptions { visible: false, borderless: true, transparent: true, always_on_top: true, resizable: false, ..Default::default() };
    let native = MacWindowOptions::default().shadow(false).collection_behavior(EVERY_SPACE);
    rune::run_windows_with_args(RunOptions { accessory: false }, Vec::<String>::new(), move |_| {
        vec![WindowSpec::from_app("alfred", app).configured_options(window.macos(native))]
    });
}
