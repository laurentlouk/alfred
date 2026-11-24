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
}

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
        }
    }

    pub fn risk(&self) -> Risk {
        match self {
            Action::QuitApp { .. } => Risk::Confirm,
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
        }
    }

    /// The question Alfred asks before a [`Risk::Confirm`] action.
    pub fn confirm_question(&self) -> String {
        match self {
            Action::QuitApp { name } => format!("Shall I quit {name}?"),
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
        }
    }
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
}

impl Outcome {
    pub fn new(summary: impl Into<String>) -> Self {
        Self { summary: summary.into() }
    }
}

/// Performs actions. `alfred-mac` provides the real one; [`DryRun`] records instead.
pub trait Executor: Send + Sync {
    fn execute(&self, action: &Action) -> Result<Outcome, String>;
}

/// Records every action and pretends it worked. Used by tests, `alfred eval` and `--dry-run`.
#[derive(Default)]
pub struct DryRun {
    log: Mutex<Vec<Action>>,
}

impl DryRun {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every action executed so far, in order.
    pub fn actions(&self) -> Vec<Action> {
        self.log.lock().map(|l| l.clone()).unwrap_or_default()
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
