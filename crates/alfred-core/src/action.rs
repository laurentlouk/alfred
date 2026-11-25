//! What Alfred can do on the Mac, as plain data, and the executor that carries it out.
//!
//! The router and the LLM both produce an [`Action`]; an [`Executor`] performs it. Keeping the action as
//! data (not a closure) lets the UI show it before it runs, ask for confirmation, log it, and replay it in
//! tests with [`DryRun`].

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// One thing Alfred can do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum Action {
    /// Open (or bring forward) an app by its name, such as "Notes".
    OpenApp { name: String },
    /// Ask an app to quit. Needs confirmation.
    QuitApp { name: String },
    /// Save a note in Apple Notes and show it.
    CreateNote { title: String, body: String },
    /// Add a reminder to Apple Reminders, optionally due in some minutes.
    CreateReminder { title: String, minutes: Option<u32> },
    /// Search the web in a browser (the default one unless `browser` names one).
    WebSearch { query: String, browser: Option<String> },
    /// Open an http(s) link in a browser.
    OpenUrl { url: String, browser: Option<String> },
    /// Say the local time and date.
    GetTime,
    /// Run one of the user's Shortcuts, optionally with text as its input.
    RunShortcut { name: String, input: Option<String> },
    /// Operate an app on screen to do a task no other tool covers (needs `agent-desktop`).
    UseApp { app: String, task: String },
    /// Write a message in a chat app (WhatsApp, Messages, Slack…): to `to` (a person or chat the user named),
    /// else in the newest conversation or the app's one message box. Sending waits for the user's yes.
    SendMessage { app: String, to: Option<String>, text: String },
    /// Click one named control in an app: the irreversible step a [`Action::UseApp`] or
    /// [`Action::UseBrowser`] run stopped before. `app` is [`BROWSER_APP`] for a web page.
    PressButton { app: String, role: String, name: String },
    /// Do a task on websites that takes more than one step (needs `agent-browser`), starting at `url` when
    /// the user gave one.
    UseBrowser { task: String, url: Option<String> },
    /// Do a task on the page the user is looking at in their own browser ("this page", "here"): in their
    /// tab when Alfred works in their Chrome, else on the same page in Alfred's tab.
    UseTheirTab { task: String },
    /// Read the newest email in Mail's inbox, or the newest from `from` (part of a name or an address).
    ReadMail { from: Option<String> },
    /// Open a reply to the newest email (from `from` when given) with `text` in it. Alfred never sends
    /// mail: the reply waits in Mail for the user to check and send.
    #[serde(rename = "reply_mail")]
    DraftReply { from: Option<String>, text: String },
    /// Find emails in Mail's inbox by their subject (`query`) and sender (`from`), and say the newest.
    SearchMail { query: Option<String>, from: Option<String> },
    /// Play, pause or skip music in Apple Music, say what's playing, or list the user's playlists. `what`
    /// is what to play: a playlist, a song, an artist or an album; `None` resumes.
    Music { command: MusicCommand, what: Option<String> },
    /// Find a video that explains or shows `query` on YouTube and play it.
    FindVideo { query: String },
    /// Open a new Claude Code session with `prompt` filled in, in the folder the user named (`folder`, a
    /// name such as "alfred"). The prompt waits there for the user to send it.
    ClaudeCode { prompt: String, folder: Option<String> },
    /// Send `text` through Messages to `handle` (a number or an address of `name`, said as its `label`:
    /// "mobile"). Only a `send_message` outcome produces it, once the person is found, and it waits for
    /// the user's yes.
    SendText { name: String, label: String, handle: String, text: String },
}

/// What [`Action::Music`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicCommand {
    Play,
    Pause,
    Next,
    Previous,
    NowPlaying,
    Playlists,
}

impl MusicCommand {
    /// The command a word stands for: "play", "pause", "skip", "now_playing"…
    pub fn from_word(word: &str) -> Option<Self> {
        Some(match crate::catalog::fold(word).replace(' ', "_").as_str() {
            "play" | "resume" | "start" | "shuffle" | "listen" => MusicCommand::Play,
            "pause" | "stop" => MusicCommand::Pause,
            "next" | "skip" | "next_track" | "next_song" | "forward" => MusicCommand::Next,
            "previous" | "back" | "previous_track" | "previous_song" | "last" => MusicCommand::Previous,
            "now_playing" | "current" | "current_track" | "what_s_playing" | "whats_playing" | "playing" | "song" => {
                MusicCommand::NowPlaying
            }
            "playlists" | "playlist" | "list_playlists" | "my_playlists" => MusicCommand::Playlists,
            _ => return None,
        })
    }
}

/// The `app` of a [`Action::PressButton`] held on a web page.
pub const BROWSER_APP: &str = "Browser";

/// How careful Alfred must be before running an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// Runs at once.
    Safe,
    /// Waits for a yes from the user.
    Confirm,
}

impl Action {
    /// The tool name the LLM uses for this action.
    pub fn tool_name(&self) -> &'static str {
        match self {
            Action::OpenApp { .. } => "open_app",
            Action::QuitApp { .. } => "quit_app",
            Action::CreateNote { .. } => "create_note",
            Action::CreateReminder { .. } => "create_reminder",
            Action::WebSearch { .. } => "web_search",
            Action::OpenUrl { .. } => "open_url",
            Action::GetTime => "get_time",
            Action::RunShortcut { .. } => "run_shortcut",
            Action::UseApp { .. } => "use_app",
            Action::SendMessage { .. } => "send_message",
            Action::PressButton { .. } => "press_button",
            Action::UseBrowser { .. } | Action::UseTheirTab { .. } => "use_browser",
            Action::ReadMail { .. } => "read_mail",
            Action::DraftReply { .. } => "reply_mail",
            Action::SearchMail { .. } => "search_mail",
            Action::Music { .. } => "music",
            Action::FindVideo { .. } => "find_video",
            Action::ClaudeCode { .. } => "claude_code",
            Action::SendText { .. } => "send_text",
        }
    }

    /// Its words carry text someone else wrote: an email's body, or its sender and subject (named in a
    /// reply's summary), an app's window or a web page (a search's answer is read off its results), or a
    /// button's name read off one. The conversation history keeps a plain phrase instead, so the model never
    /// reads that text as part of the conversation on a later turn.
    pub fn reads_outside_text(&self) -> bool {
        matches!(
            self,
            Action::WebSearch { .. }
                | Action::ReadMail { .. }
                | Action::DraftReply { .. }
                | Action::SearchMail { .. }
                | Action::FindVideo { .. }
                | Action::UseApp { .. }
                | Action::SendMessage { .. }
                | Action::PressButton { .. }
                | Action::UseBrowser { .. }
                | Action::UseTheirTab { .. }
        )
    }

    /// Shortcuts drive real devices and a small model can pick the wrong one, so running one confirms
    /// unless the user named it (the agent decides that); a press is the irreversible click itself, and a
    /// text is the message sent.
    pub fn risk(&self) -> Risk {
        match self {
            Action::QuitApp { .. } | Action::RunShortcut { .. } | Action::PressButton { .. } | Action::SendText { .. } => Risk::Confirm,
            _ => Risk::Safe,
        }
    }

    /// A short label while the action runs, for the UI's chips: "Opening Notes".
    pub fn progress_label(&self) -> String {
        match self {
            Action::OpenApp { name } => format!("Opening {name}"),
            Action::QuitApp { name } => format!("Quitting {name}"),
            Action::CreateNote { title, .. } => format!("Saving note “{}”", clip(title, 28)),
            Action::CreateReminder { title, .. } => format!("Adding reminder “{}”", clip(title, 28)),
            Action::WebSearch { query, .. } => format!("Searching “{}”", clip(query, 28)),
            Action::OpenUrl { url, .. } => format!("Opening {}", host_of(url)),
            Action::GetTime => "Checking the time".into(),
            Action::RunShortcut { name, .. } => format!("Running “{}”", clip(name, 32)),
            Action::UseApp { app, .. } => format!("Using {app}"),
            Action::SendMessage { app, to: Some(to), .. } => format!("Writing to {} in {app}", clip(to, 24)),
            Action::SendMessage { app, to: None, .. } => format!("Writing in {app}"),
            Action::PressButton { app, name, .. } => format!("Clicking “{}” in {}", clip(name, 24), place(app)),
            Action::UseBrowser { url: Some(u), .. } => format!("Browsing {}", host_of(u)),
            Action::UseBrowser { url: None, .. } => "Using the browser".into(),
            Action::UseTheirTab { .. } => "Working on your page".into(),
            Action::ReadMail { from: None } => "Reading your latest email".into(),
            Action::ReadMail { from: Some(f) } => format!("Reading the latest email from {}", clip(f, 24)),
            Action::DraftReply { from: None, .. } => "Drafting a reply".into(),
            Action::DraftReply { from: Some(f), .. } => format!("Drafting a reply to {}", clip(f, 24)),
            Action::SearchMail { query, from } => format!("Searching your email{}", mail_terms(query, from, 20)),
            Action::Music { command, what } => music_label(*command, what.as_deref(), true),
            Action::FindVideo { query } => format!("Finding a video on “{}”", clip(query, 28)),
            Action::ClaudeCode { .. } => "Opening Claude Code".into(),
            Action::SendText { name, .. } => format!("Texting {}", clip(name, 24)),
        }
    }

    /// The action as a short imperative, for a confirmation card: "Quit Safari".
    pub fn short_label(&self) -> String {
        match self {
            Action::OpenApp { name } => format!("Open {name}"),
            Action::QuitApp { name } => format!("Quit {name}"),
            Action::CreateNote { title, .. } => format!("Save note “{}”", clip(title, 28)),
            Action::CreateReminder { title, .. } => format!("Add reminder “{}”", clip(title, 28)),
            Action::WebSearch { query, .. } => format!("Search “{}”", clip(query, 28)),
            Action::OpenUrl { url, .. } => format!("Open {}", host_of(url)),
            Action::GetTime => "Tell the time".into(),
            Action::RunShortcut { name, .. } => format!("Run the shortcut “{}”", clip(name, 32)),
            Action::UseApp { app, task } => format!("In {app}: {}", clip(task, 40)),
            Action::SendMessage { app, text, .. } => format!("Message in {app}: {}", clip(text, 36)),
            Action::PressButton { app, name, .. } => format!("Click “{}” in {}", clip(name, 32), place(app)),
            Action::UseBrowser { task, url: Some(u) } => format!("On {}: {}", host_of(u), clip(task, 40)),
            Action::UseBrowser { task, url: None } => format!("In the browser: {}", clip(task, 40)),
            Action::UseTheirTab { task } => format!("On your page: {}", clip(task, 40)),
            Action::ReadMail { from: None } => "Read the latest email".into(),
            Action::ReadMail { from: Some(f) } => format!("Read the latest email from {}", clip(f, 28)),
            Action::DraftReply { from: None, .. } => "Draft a reply to the latest email".into(),
            Action::DraftReply { from: Some(f), .. } => format!("Draft a reply to {}", clip(f, 28)),
            Action::SearchMail { query, from } => format!("Search your email{}", mail_terms(query, from, 24)),
            Action::Music { command, what } => music_label(*command, what.as_deref(), false),
            Action::FindVideo { query } => format!("Find a video on “{}”", clip(query, 32)),
            Action::ClaudeCode { prompt, .. } => format!("Ask Claude Code: {}", clip(prompt, 36)),
            Action::SendText { name, text, .. } => format!("Text {}: {}", clip(name, 20), clip(text, 32)),
        }
    }

    /// The question Alfred asks before a [`Risk::Confirm`] action.
    pub fn confirm_question(&self) -> String {
        match self {
            Action::QuitApp { name } => format!("Shall I quit {name}?"),
            Action::RunShortcut { name, .. } => format!("Shall I run the shortcut “{name}”?"),
            Action::PressButton { app, name, .. } => format!("Shall I click “{name}” in {}?", place(app)),
            Action::SendText { name, label, text, .. } if label.is_empty() => format!("Shall I text {name}: “{text}”?"),
            Action::SendText { name, label, text, .. } => format!("Shall I text {name}'s {label}: “{text}”?"),
            other => format!("Shall I go ahead: {}?", other.progress_label().to_lowercase()),
        }
    }

    /// What Alfred says once the action succeeded, when the executor has nothing more specific.
    pub fn done_phrase(&self) -> String {
        match self {
            Action::OpenApp { name } => format!("{name} is open."),
            Action::QuitApp { name } => format!("{name} has been closed."),
            Action::CreateNote { title, .. } => format!("I've saved a note: “{title}”."),
            Action::CreateReminder { title, minutes: None } => format!("I've added a reminder: “{title}”."),
            Action::CreateReminder { title, minutes: Some(m) } => {
                format!("I've added a reminder: “{title}”, due in {}.", minutes_phrase(*m))
            }
            Action::WebSearch { query, .. } => format!("Here are the results for “{query}”."),
            Action::OpenUrl { url, .. } => format!("I've opened {}.", host_of(url)),
            Action::GetTime => "Here is the time.".into(),
            Action::RunShortcut { name, .. } => format!("I've run “{name}”."),
            Action::UseApp { app, .. } => format!("Done in {app}."),
            Action::SendMessage { app, .. } => format!("I've written your message in {app}."),
            Action::PressButton { app, name, .. } => format!("I've clicked “{name}” in {}.", place(app)),
            Action::UseBrowser { .. } => "Done in the browser.".into(),
            Action::UseTheirTab { .. } => "Done on your page.".into(),
            Action::ReadMail { .. } => "I've read you the latest email.".into(),
            Action::DraftReply { .. } => "I've opened a reply in Mail for you to check and send.".into(),
            Action::SearchMail { .. } => "Here are the emails I found.".into(),
            Action::Music { command: MusicCommand::Pause, .. } => "The music is paused.".into(),
            Action::Music { command: MusicCommand::Play, what: Some(w) } => format!("Playing {w}."),
            Action::Music { .. } => "Done in Music.".into(),
            Action::FindVideo { query } => format!("Here's a video about “{query}”."),
            Action::ClaudeCode { .. } => "Claude Code is open with your prompt: check it and press Return to send it.".into(),
            Action::SendText { name, .. } => format!("I've sent your message to {name}."),
        }
    }
}

/// " about “invoice” from Ada", for a mail search's labels.
fn mail_terms(query: &Option<String>, from: &Option<String>, max: usize) -> String {
    let about = query.as_deref().map(|q| format!(" about “{}”", clip(q, max))).unwrap_or_default();
    let from = from.as_deref().map(|f| format!(" from {}", clip(f, max))).unwrap_or_default();
    format!("{about}{from}")
}

/// "Playing “Focus”", "Pausing the music", "Next track", for a music action's labels (`ing`: the progress form).
fn music_label(command: MusicCommand, what: Option<&str>, ing: bool) -> String {
    let (doing, done) = match (command, what) {
        (MusicCommand::Play, Some(w)) => (format!("Playing “{}”", clip(w, 28)), format!("Play “{}”", clip(w, 32))),
        (MusicCommand::Play, None) => ("Playing music".into(), "Play music".into()),
        (MusicCommand::Pause, _) => ("Pausing the music".into(), "Pause the music".into()),
        (MusicCommand::Next, _) => ("Skipping to the next track".into(), "Next track".into()),
        (MusicCommand::Previous, _) => ("Going back a track".into(), "Previous track".into()),
        (MusicCommand::NowPlaying, _) => ("Checking what's playing".into(), "Say what's playing".into()),
        (MusicCommand::Playlists, _) => ("Looking at your playlists".into(), "List your playlists".into()),
    };
    if ing { doing } else { done }
}

/// Where a held click happens, in words: an app's name, or "the browser".
fn place(app: &str) -> &str {
    if app == BROWSER_APP { "the browser" } else { app }
}

/// "1 minute", "20 minutes", "1 hour", "2 hours 30 minutes".
pub fn minutes_phrase(m: u32) -> String {
    let plural = |n: u32, unit: &str| if n == 1 { format!("1 {unit}") } else { format!("{n} {unit}s") };
    match (m / 60, m % 60) {
        (0, mins) => plural(mins, "minute"),
        (h, 0) => plural(h, "hour"),
        (h, mins) => format!("{} {}", plural(h, "hour"), plural(mins, "minute")),
    }
}

/// The host of a URL, without "www.": "https://www.youtube.com/x" → "youtube.com".
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The result of a successful action, in words Alfred can say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub summary: String,
    /// A risky step the action stopped before (an on-screen run reaching a Send button). The agent asks
    /// about it like any confirm-level action.
    pub follow_up: Option<Action>,
}

impl Outcome {
    pub fn new(summary: impl Into<String>) -> Self {
        Self { summary: summary.into(), follow_up: None }
    }

    pub fn with_follow_up(mut self, action: Action) -> Self {
        self.follow_up = Some(action);
        self
    }
}

/// Operating apps on screen. `alfred-hands` implements it with `agent-desktop`; the executor delegates
/// [`Action::UseApp`] and [`Action::PressButton`] to it.
pub trait Hands: Send + Sync {
    /// Work towards `task` in `app`, one observed step at a time. Stops before irreversible clicks and
    /// returns them as the outcome's follow-up.
    fn use_app(&self, app: &str, task: &str) -> Result<Outcome, String>;
    /// Find the control by role and name in `app` and click it.
    fn press(&self, app: &str, role: &str, name: &str) -> Result<Outcome, String>;
    /// Write `text` in a chat app, to `to` or in the newest conversation, and stop before sending (the send
    /// button comes back as the outcome's follow-up).
    fn send_message(&self, app: &str, to: Option<&str>, text: &str) -> Result<Outcome, String> {
        self.use_app(app, &message_task(to, text))
    }
    /// Whether the on-screen engine is allowed to work (macOS's Accessibility permission); `None` when it
    /// can't tell.
    fn allowed(&self) -> Option<bool> {
        None
    }
}

/// A message to write, as a task for the step-by-step loop.
pub fn message_task(to: Option<&str>, text: &str) -> String {
    match to {
        Some(to) => format!("open the conversation with {to}, write this in the message box, and stop before sending: {text}"),
        None => format!("open the newest conversation, write this in the message box, and stop before sending: {text}"),
    }
}

/// Operating web pages. `alfred-hands` implements it with `agent-browser`; the executor delegates
/// [`Action::UseBrowser`], and a [`Action::PressButton`] in [`BROWSER_APP`], to it.
pub trait WebHands: Send + Sync {
    /// Work towards `task` on the web, starting at `url` when given, else on the page already open. Stops
    /// before irreversible clicks and returns them as the outcome's follow-up.
    fn browse(&self, task: &str, url: Option<&str>) -> Result<Outcome, String>;
    /// Find the control by role and name on the open page and click it.
    fn press(&self, role: &str, name: &str) -> Result<Outcome, String>;
    /// Search the web for `query`, leave the results open, and answer from them in a sentence or two. An
    /// error means the results couldn't be read, so the caller opens them some other way.
    fn answer(&self, query: &str) -> Result<Outcome, String>;
    /// Open `url` in Alfred's tab and leave it there. An error means the caller opens it some other way.
    fn show(&self, url: &str) -> Result<Outcome, String> {
        Err(format!("I can't open {url} in my tab."))
    }
    /// Do `task` on the page the user is looking at, whose address is `url`: in their own tab when Alfred
    /// works in their browser, else on the same page in Alfred's tab.
    fn browse_theirs(&self, task: &str, url: &str) -> Result<Outcome, String> {
        self.browse(task, Some(url))
    }
    /// The address Alfred's tab shows, once it has opened one.
    fn page(&self) -> Option<String> {
        None
    }
    /// The first video YouTube offers for `query` (never a Short or an ad), read off its results in
    /// Alfred's tab, which stays on them. An error means the caller opens the results some other way.
    fn find_video(&self, query: &str) -> Result<FoundVideo, String> {
        Err(format!("I can't look for videos about {query} in my tab."))
    }
}

/// A video found on YouTube.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundVideo {
    pub title: String,
    /// The channel's name, "" when the results didn't show it.
    pub channel: String,
    /// `https://www.youtube.com/watch?v=…`.
    pub url: String,
}

impl FoundVideo {
    /// What Alfred says once it plays: "Playing “Transformers explained” from 3Blue1Brown on YouTube."
    pub fn summary(&self) -> String {
        if self.channel.is_empty() {
            format!("Playing “{}” on YouTube.", self.title)
        } else {
            format!("Playing “{}” from {} on YouTube.", self.title, self.channel)
        }
    }
}

/// Performs actions. `alfred-mac` provides the real one; [`DryRun`] records instead.
pub trait Executor: Send + Sync {
    fn execute(&self, action: &Action) -> Result<Outcome, String>;
    /// What the user is looking at right now (the app they were in, their tab's site) and what Alfred's own
    /// browser tab shows. Asked only when the model is about to decide.
    fn context(&self) -> crate::context::Context {
        crate::context::Context::default()
    }
}

/// Records every action and pretends it worked. Used by tests, `alfred eval` and `--dry-run`.
#[derive(Default)]
pub struct DryRun {
    log: Mutex<Vec<Action>>,
    context: crate::context::Context,
    context_reads: std::sync::atomic::AtomicUsize,
}

impl DryRun {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every action executed so far, in order.
    pub fn actions(&self) -> Vec<Action> {
        self.log.lock().map(|l| l.clone()).unwrap_or_default()
    }

    /// Pretend the user is looking at this.
    pub fn with_context(mut self, context: crate::context::Context) -> Self {
        self.context = context;
        self
    }

    /// How many times the context was asked for.
    pub fn context_reads(&self) -> usize {
        self.context_reads.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn clear(&self) {
        if let Ok(mut l) = self.log.lock() {
            l.clear();
        }
    }
}

impl Executor for DryRun {
    fn execute(&self, action: &Action) -> Result<Outcome, String> {
        if let Ok(mut l) = self.log.lock() {
            l.push(action.clone());
        }
        Ok(Outcome::new(match action {
            Action::GetTime => "It's 09:41 on Tuesday 24 November.".to_string(),
            other => other.done_phrase(),
        }))
    }

    fn context(&self) -> crate::context::Context {
        self.context_reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.context.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quitting_is_the_only_risky_action() {
        assert_eq!(Action::QuitApp { name: "Safari".into() }.risk(), Risk::Confirm);
        assert_eq!(Action::OpenApp { name: "Safari".into() }.risk(), Risk::Safe);
        assert_eq!(Action::GetTime.risk(), Risk::Safe);
    }

    #[test]
    fn phrases_read_naturally() {
        assert_eq!(Action::OpenApp { name: "Notes".into() }.done_phrase(), "Notes is open.");
        assert_eq!(
            Action::CreateReminder { title: "Call mum".into(), minutes: Some(90) }.done_phrase(),
            "I've added a reminder: “Call mum”, due in 1 hour 30 minutes."
        );
        assert_eq!(Action::QuitApp { name: "Music".into() }.confirm_question(), "Shall I quit Music?");
        assert_eq!(
            Action::OpenUrl { url: "https://www.youtube.com/watch?v=1".into(), browser: None }.progress_label(),
            "Opening youtube.com"
        );
    }

    #[test]
    fn mail_actions_are_safe_and_reading_one_hides_its_text_from_history() {
        let read = Action::ReadMail { from: Some("Ada".into()) };
        let reply = Action::DraftReply { from: None, text: "See you at 3.".into() };
        assert_eq!((read.risk(), reply.risk()), (Risk::Safe, Risk::Safe), "a draft is never sent");
        assert_eq!(read.progress_label(), "Reading the latest email from Ada");
        assert_eq!(reply.tool_name(), "reply_mail");
        assert!(read.reads_outside_text() && reply.reads_outside_text(), "a reply's summary names the sender and subject");
        assert_eq!(serde_json::to_value(&reply).unwrap()["text"], "See you at 3.");
    }

    #[test]
    fn browser_tasks_are_safe_and_keep_page_text_out_of_history() {
        let browse =
            Action::UseBrowser { task: "find the price of a Kindle".into(), url: Some("https://www.amazon.com/s?k=kindle".into()) };
        assert_eq!(browse.tool_name(), "use_browser");
        assert_eq!(browse.risk(), Risk::Safe);
        assert!(browse.reads_outside_text(), "page text is written by others");
        assert_eq!(browse.progress_label(), "Browsing amazon.com");
        assert_eq!(Action::UseBrowser { task: "x".into(), url: None }.progress_label(), "Using the browser");
        assert_eq!(browse.short_label(), "On amazon.com: find the price of a Kindle");
        assert_eq!(browse.done_phrase(), "Done in the browser.");
        let v = serde_json::to_value(&browse).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"tool": "use_browser", "task": "find the price of a Kindle", "url": "https://www.amazon.com/s?k=kindle"})
        );
        assert_eq!(serde_json::from_value::<Action>(v).unwrap(), browse);
    }

    #[test]
    fn a_search_answers_from_web_pages_so_its_words_stay_out_of_history() {
        let search = Action::WebSearch { query: "weather in Paris".into(), browser: None };
        assert_eq!(search.risk(), Risk::Safe);
        assert!(search.reads_outside_text(), "the answer is read off the results page");
        assert!(!Action::OpenUrl { url: "https://example.com".into(), browser: None }.reads_outside_text());
        assert_eq!(search.done_phrase(), "Here are the results for “weather in Paris”.");
    }

    #[test]
    fn a_click_held_in_the_browser_says_so_in_words() {
        let press = Action::PressButton { app: BROWSER_APP.into(), role: "button".into(), name: "Place order".into() };
        assert_eq!(press.risk(), Risk::Confirm);
        assert_eq!(press.confirm_question(), "Shall I click “Place order” in the browser?");
        assert_eq!(press.done_phrase(), "I've clicked “Place order” in the browser.");
        assert_eq!(press.progress_label(), "Clicking “Place order” in the browser");
        let mail = Action::PressButton { app: "Mail".into(), role: "button".into(), name: "Send".into() };
        assert_eq!(mail.confirm_question(), "Shall I click “Send” in Mail?");
    }

    #[test]
    fn app_card_actions_say_what_they_do_and_only_a_text_waits_for_a_yes() {
        let play = Action::Music { command: MusicCommand::Play, what: Some("Focus".into()) };
        assert_eq!((play.tool_name(), play.risk(), play.progress_label()), ("music", Risk::Safe, "Playing “Focus”".to_string()));
        assert_eq!(
            serde_json::to_value(&play).unwrap(),
            serde_json::json!({"tool": "music", "command": "play", "what": "Focus"}),
            "the shape recordings keep"
        );
        assert!(!play.reads_outside_text(), "the user's own library");
        let pause = Action::Music { command: MusicCommand::Pause, what: None };
        assert_eq!(pause.done_phrase(), "The music is paused.");
        let mail = Action::SearchMail { query: Some("invoice".into()), from: Some("Ada".into()) };
        assert_eq!(mail.progress_label(), "Searching your email about “invoice” from Ada");
        assert!(mail.reads_outside_text(), "subjects are written by others");
        let video = Action::FindVideo { query: "how transformers work".into() };
        assert!(video.reads_outside_text() && video.risk() == Risk::Safe, "titles are written by others");
        let code = Action::ClaudeCode { prompt: "fix the failing test".into(), folder: Some("alfred".into()) };
        assert_eq!((code.tool_name(), code.risk()), ("claude_code", Risk::Safe), "the prompt only waits in the composer");
        let text = Action::SendText {
            name: "Ada Lovelace".into(),
            label: "mobile".into(),
            handle: "+33612345678".into(),
            text: "See you at 7".into(),
        };
        assert_eq!(text.risk(), Risk::Confirm);
        assert_eq!(text.confirm_question(), "Shall I text Ada Lovelace's mobile: “See you at 7”?");
        let bare = Action::SendText { name: "+33 6 12".into(), label: String::new(), handle: "+33612".into(), text: "Hi".into() };
        assert_eq!(bare.confirm_question(), "Shall I text +33 6 12: “Hi”?");
        assert_eq!(MusicCommand::from_word("skip"), Some(MusicCommand::Next));
        assert_eq!(MusicCommand::from_word("now playing"), Some(MusicCommand::NowPlaying));
        assert_eq!(MusicCommand::from_word("dance"), None);
    }

    #[test]
    fn minutes_phrase_handles_units() {
        assert_eq!(minutes_phrase(1), "1 minute");
        assert_eq!(minutes_phrase(45), "45 minutes");
        assert_eq!(minutes_phrase(60), "1 hour");
        assert_eq!(minutes_phrase(125), "2 hours 5 minutes");
    }

    #[test]
    fn long_labels_are_clipped() {
        let a = Action::WebSearch { query: "a".repeat(60), browser: None };
        assert!(a.progress_label().chars().count() < 45);
        assert!(a.progress_label().ends_with("…”"));
    }

    #[test]
    fn dry_run_records_in_order() {
        let d = DryRun::new();
        d.execute(&Action::GetTime).unwrap();
        d.execute(&Action::OpenApp { name: "Notes".into() }).unwrap();
        assert_eq!(d.actions(), vec![Action::GetTime, Action::OpenApp { name: "Notes".into() }]);
    }
}
