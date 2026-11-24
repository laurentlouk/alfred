//! Alfred's hands on a Mac: `open` for apps and links, `osascript` for Notes, Reminders and quitting.
//!
//! User text never becomes AppleScript source. Every script here is a constant that reads its inputs from
//! `on run argv`, and the inputs travel as separate process arguments, so a note body such as
//! `" & do shell script "…` is just text.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use alfred_core::normalize::search_url;
use alfred_core::{Action, Executor, Outcome};

/// Runs actions for real on this Mac.
#[derive(Debug, Clone)]
pub struct MacExecutor {
    /// `google`, `duckduckgo` or `bing`.
    pub search_engine: String,
    /// The browser for searches and links when the request doesn't name one; `None` for the default.
    pub browser: Option<String>,
    /// How long an AppleScript may run. The first one to an app can wait on the user's permission prompt.
    pub timeout: Duration,
    /// Bring Notes forward to show a note it just saved.
    pub show_notes: bool,
}

impl Default for MacExecutor {
    fn default() -> Self {
        Self { search_engine: "google".into(), browser: None, timeout: Duration::from_secs(60), show_notes: true }
    }
}

impl MacExecutor {
    pub fn from_config(c: &alfred_core::Config) -> Self {
        Self {
            search_engine: c.search.engine.clone(),
            browser: Some(c.search.browser.trim().to_string()).filter(|b| !b.is_empty()),
            ..Self::default()
        }
    }

    fn open_in_browser(&self, url: &str, browser: Option<&str>) -> Result<(), String> {
        let mut cmd = Command::new("open");
        if let Some(b) = browser.or(self.browser.as_deref()) {
            cmd.arg("-a").arg(b);
        }
        cmd.arg(url);
        run(cmd, self.timeout).map(|_| ())
    }
}

/// Notes: make the note in the default folder of the default account, then show it.
const NOTE_SCRIPT: &str = r#"on run argv
    set noteBody to item 1 of argv
    set shouldShow to item 2 of argv
    tell application "Notes"
        set newNote to make new note at default folder of default account with properties {body:noteBody}
        if shouldShow is "1" then
            activate
            show newNote
        end if
        return name of newNote
    end tell
end run"#;

/// Reminders: in the default list, due after `minutes` when that is above zero.
const REMINDER_SCRIPT: &str = r#"on run argv
    set reminderTitle to item 1 of argv
    set delayMinutes to (item 2 of argv) as integer
    tell application "Reminders"
        if delayMinutes > 0 then
            make new reminder with properties {name:reminderTitle, due date:((current date) + delayMinutes * minutes)}
        else
            make new reminder with properties {name:reminderTitle}
        end if
    end tell
    return reminderTitle
end run"#;

const QUIT_SCRIPT: &str = r#"on run argv
    tell application (item 1 of argv) to quit
end run"#;

impl Executor for MacExecutor {
    fn execute(&self, action: &Action) -> Result<Outcome, String> {
        match action {
            Action::OpenApp { name } => {
                let mut cmd = Command::new("open");
                cmd.arg("-a").arg(name);
                run(cmd, self.timeout).map_err(|e| {
                    if e.contains("Unable to find application") { format!("I couldn't find an app called {name}.") } else { e }
                })?;
                Ok(Outcome::new(action.done_phrase()))
            }
            Action::QuitApp { name } => {
                osascript(QUIT_SCRIPT, &[name], self.timeout)?;
                Ok(Outcome::new(action.done_phrase()))
            }
            Action::CreateNote { title, body } => {
                let html = note_html(title, body);
                osascript(NOTE_SCRIPT, &[&html, if self.show_notes { "1" } else { "0" }], self.timeout)?;
                Ok(Outcome::new(action.done_phrase()))
            }
            Action::CreateReminder { title, minutes } => {
                osascript(REMINDER_SCRIPT, &[title, &minutes.unwrap_or(0).to_string()], self.timeout)?;
                Ok(Outcome::new(action.done_phrase()))
            }
            Action::WebSearch { query, browser } => {
                self.open_in_browser(&search_url(&self.search_engine, query), browser.as_deref())?;
                Ok(Outcome::new(action.done_phrase()))
            }
            Action::OpenUrl { url, browser } => {
                // Refuse anything that isn't http(s) even if a caller built the action by hand.
                if alfred_core::normalize::url(url).as_deref() != Some(url.as_str()) {
                    return Err(format!("{url} is not a web address I can open."));
                }
                self.open_in_browser(url, browser.as_deref())?;
                Ok(Outcome::new(action.done_phrase()))
            }
            Action::GetTime => {
                let mut cmd = Command::new("date");
                cmd.arg("+%H:%M on %A %-d %B");
                Ok(Outcome::new(format!("It's {}.", run(cmd, self.timeout)?.trim())))
            }
        }
    }
}

/// The note as Notes stores it: a heading (its title) and the text, HTML-escaped, one `<div>` per line.
pub fn note_html(title: &str, body: &str) -> String {
    let mut html = format!("<h1>{}</h1>", escape_html(title));
    // A body that is only the title (short notes) doesn't repeat it.
    if body.trim() != title.trim() {
        for line in body.lines() {
            html.push_str(&if line.trim().is_empty() {
                "<div><br></div>".to_string()
            } else {
                format!("<div>{}</div>", escape_html(line))
            });
        }
    }
    html
}

pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Run an AppleScript with arguments, never interpolated into the script.
pub fn osascript(script: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut cmd = Command::new("osascript");
    cmd.arg("-e").arg(script).arg("--");
    cmd.args(args);
    run(cmd, timeout).map_err(|e| explain_applescript_error(&e))
}

fn explain_applescript_error(e: &str) -> String {
    if e.contains("-1743") || e.contains("Not authorized to send Apple events") {
        "macOS didn't allow me to control that app. Allow Alfred in System Settings → Privacy & Security → Automation.".into()
    } else if e.contains("-1712") || e.contains("timed out") {
        "The app took too long to answer (a permission prompt may be waiting).".into()
    } else if e.contains("-1728") || e.contains("-10814") || e.contains("Can't get application") {
        "I couldn't find that app.".into()
    } else {
        e.trim().trim_start_matches("execution error: ").to_string()
    }
}

/// Run a command with a timeout; Ok(stdout) on success, Err(stderr or a description) otherwise.
fn run(mut cmd: Command, timeout: Duration) -> Result<String, String> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't start {program}: {e}"))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > timeout => {
                child.kill().ok();
                child.wait().ok();
                return Err(format!("{program} timed out after {}s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(e) => return Err(format!("{program}: {e}")),
        }
    };
    let mut out = String::new();
    let mut err = String::new();
    if let Some(mut s) = child.stdout.take() {
        s.read_to_string(&mut out).ok();
    }
    if let Some(mut s) = child.stderr.take() {
        s.read_to_string(&mut err).ok();
    }
    if status.success() { Ok(out) } else { Err(if err.trim().is_empty() { format!("{program} failed ({status})") } else { err }) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_html_escapes_and_keeps_lines() {
        assert_eq!(note_html("Milk & eggs", "Milk & eggs"), "<h1>Milk &amp; eggs</h1>");
        assert_eq!(
            note_html("Plan", "<b>step</b> one\n\nstep \"two\""),
            "<h1>Plan</h1><div>&lt;b&gt;step&lt;/b&gt; one</div><div><br></div><div>step &quot;two&quot;</div>"
        );
    }

    #[test]
    fn scripts_take_text_only_through_argv() {
        for s in [NOTE_SCRIPT, REMINDER_SCRIPT, QUIT_SCRIPT] {
            assert!(s.starts_with("on run argv"), "{s}");
            assert!(!s.contains("{}"), "no format holes in {s}");
        }
    }

    #[test]
    fn hostile_text_stays_text() {
        // A real osascript run: the argument comes back verbatim instead of executing.
        if !cfg!(target_os = "macos") {
            return;
        }
        let evil = "\" & (do shell script \"echo pwned\") & \"";
        let out = osascript("on run argv\nreturn item 1 of argv\nend run", &[evil], Duration::from_secs(20)).unwrap();
        assert_eq!(out.trim_end(), evil);
    }

    #[test]
    fn commands_time_out() {
        let mut cmd = Command::new("sleep");
        cmd.arg("5");
        let e = run(cmd, Duration::from_millis(100)).unwrap_err();
        assert!(e.contains("timed out"), "{e}");
    }

    #[test]
    fn errors_are_explained() {
        assert!(explain_applescript_error("execution error: Not authorized to send Apple events to Notes. (-1743)").contains("Automation"));
        assert_eq!(explain_applescript_error("execution error: boom"), "boom");
    }

    #[test]
    fn open_url_refuses_non_http_even_when_built_by_hand() {
        let e = MacExecutor::default().execute(&Action::OpenUrl { url: "file:///etc/passwd".into(), browser: None }).unwrap_err();
        assert!(e.contains("not a web address"));
    }

    #[test]
    fn the_time_is_read_from_the_system() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let o = MacExecutor::default().execute(&Action::GetTime).unwrap();
        assert!(o.summary.starts_with("It's ") && o.summary.contains(" on "), "{}", o.summary);
    }

    #[test]
    fn a_missing_app_is_reported_in_words() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let e = MacExecutor::default().execute(&Action::OpenApp { name: "Definitely Not An App 7f3a".into() }).unwrap_err();
        assert!(e.contains("couldn't find an app"), "{e}");
    }
}
