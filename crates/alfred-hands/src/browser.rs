//! The `agent-browser` command: every call prints one JSON envelope, `{success, data, error}`.
//! [`Browser`] is the seam: [`AgentBrowser`] runs the real program, tests pass recorded pages.

use std::fmt;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// A failed call, in agent-browser's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserError {
    pub message: String,
    /// The command did nothing, so trying again cannot act twice: the ref was unknown or from a page that
    /// is gone, or agent-browser's own wait ran out before it acted.
    pub retry_safe: bool,
}

impl BrowserError {
    fn new(message: impl Into<String>, retry_safe: bool) -> Self {
        Self { message: message.into(), retry_safe }
    }
}

impl fmt::Display for BrowserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// One call to agent-browser: the command and its arguments, after the global options.
pub trait Browser: Send + Sync {
    fn call(&self, args: &[String]) -> Result<Value, BrowserError>;
    /// The browser window is on screen, so the user can step in (a robot check, a sign-in code).
    fn visible(&self) -> bool {
        false
    }
}

/// The real program, in one named session with its own profile folder.
pub struct AgentBrowser {
    pub program: PathBuf,
    /// agent-browser's session name: one browser, kept between runs, that no other agent shares.
    pub session: String,
    /// The Chrome profile folder (cookies and sign-ins survive between runs).
    pub profile: PathBuf,
    /// Show the window, so the user can watch and step in.
    pub headed: bool,
    pub timeout: Duration,
}

impl AgentBrowser {
    pub fn new(program: PathBuf, profile: PathBuf, headed: bool) -> Self {
        Self { program, session: "alfred".into(), profile, headed, timeout: Duration::from_secs(45) }
    }

    /// Everything after the program name: the global options, then the command.
    pub fn command_line(&self, args: &[String]) -> Vec<String> {
        let mut all = vec!["--session".to_string(), self.session.clone(), "--profile".into(), self.profile.display().to_string()];
        if self.headed {
            all.push("--headed".into());
        }
        all.push("--json".into());
        all.extend(args.iter().cloned());
        all
    }
}

impl Browser for AgentBrowser {
    fn call(&self, args: &[String]) -> Result<Value, BrowserError> {
        if let Err(e) = std::fs::create_dir_all(&self.profile) {
            return Err(BrowserError::new(format!("couldn't make the browser profile folder {}: {e}", self.profile.display()), true));
        }
        let mut child = Command::new(&self.program)
            .args(self.command_line(args))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| BrowserError::new(format!("couldn't start {}: {e}", self.program.display()), true))?;
        // Read while it runs: a snapshot of a long page is far bigger than a pipe's buffer, and a child that
        // can't finish writing never exits.
        let reader = child.stdout.take().map(|mut out| {
            std::thread::spawn(move || {
                let mut text = String::new();
                out.read_to_string(&mut text).ok();
                text
            })
        });
        let drain = child.stderr.take().map(|mut err| {
            std::thread::spawn(move || {
                let mut sink = Vec::new();
                err.read_to_end(&mut sink).ok();
            })
        });
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if started.elapsed() > self.timeout => {
                    child.kill().ok();
                    child.wait().ok();
                    // The click may have reached the page before the program hung: not safe to repeat.
                    return Err(BrowserError::new("agent-browser didn't answer in time", false));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => return Err(BrowserError::new(e.to_string(), true)),
            }
        }
        if let Some(d) = drain {
            d.join().ok();
        }
        let out = reader.and_then(|r| r.join().ok()).unwrap_or_default();
        parse_envelope(&out)
    }

    fn visible(&self) -> bool {
        self.headed
    }
}

/// Messages that mean nothing was done.
const NOTHING_DONE: &[&str] = &["unknown ref", "no longer loaded", "timed out", "timeout"];

/// `data` from a successful envelope, or the error it carries.
pub fn parse_envelope(out: &str) -> Result<Value, BrowserError> {
    // One JSON object, possibly after stray lines (a warning printed by an older version).
    let v: Value = serde_json::from_str(out.trim())
        .ok()
        .or_else(|| out.lines().rev().find_map(|l| serde_json::from_str::<Value>(l.trim()).ok().filter(Value::is_object)))
        .ok_or_else(|| BrowserError::new(format!("unexpected output: {}", out.chars().take(120).collect::<String>()), true))?;
    if v["success"].as_bool() == Some(true) {
        return Ok(v["data"].clone());
    }
    let message = match &v["error"] {
        Value::String(s) => s.clone(),
        Value::Object(o) => o.get("message").and_then(Value::as_str).unwrap_or("agent-browser failed").to_string(),
        _ => "agent-browser failed".to_string(),
    };
    let lower = message.to_lowercase();
    let retry_safe = NOTHING_DONE.iter().any(|m| lower.contains(m));
    Err(BrowserError { message, retry_safe })
}

/// Arguments as owned strings: `args(["click", "@e2"])`.
pub fn args<const N: usize>(parts: [&str; N]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_give_data_or_an_error_in_words() {
        let ok = r#"{"success":true,"data":{"title":"Wikipedia","url":"https://en.wikipedia.org/"},"error":null}"#;
        assert_eq!(parse_envelope(ok).unwrap()["title"], "Wikipedia");
        let e = parse_envelope(r#"{"success":false,"data":null,"error":"Unknown ref: e99"}"#).unwrap_err();
        assert_eq!(e, BrowserError { message: "Unknown ref: e99".into(), retry_safe: true });
        let gone = r#"{"success":false,"data":null,"error":"Ref @e29 is from a document that is no longer loaded (the page navigated or the frame was removed). Take a new snapshot and use a fresh ref."}"#;
        assert!(parse_envelope(gone).unwrap_err().retry_safe);
        assert!(parse_envelope(r#"{"success":false,"error":"Timeout 25000ms exceeded waiting for @e3"}"#).unwrap_err().retry_safe);
        let covered = parse_envelope(r#"{"success":false,"error":"Element @e4 is covered by <div class=overlay>"}"#).unwrap_err();
        assert!(!covered.retry_safe, "only what certainly did nothing is safe to repeat");
        assert_eq!(parse_envelope(r#"{"success":false,"error":{"message":"boom"}}"#).unwrap_err().message, "boom");
        let e = parse_envelope("panic!").unwrap_err();
        assert!(e.message.starts_with("unexpected output"), "{e}");
    }

    #[test]
    fn a_stray_line_before_the_envelope_is_skipped() {
        let out = "⚠ profile: ./a → ./b, relaunched browser\n{\"success\":true,\"data\":{\"url\":\"about:blank\"},\"error\":null}\n";
        assert_eq!(parse_envelope(out).unwrap()["url"], "about:blank");
    }

    #[test]
    fn every_call_names_the_session_profile_and_json_output() {
        let b = AgentBrowser::new("/opt/homebrew/bin/agent-browser".into(), "/tmp/alfred-profile".into(), true);
        assert_eq!(
            b.command_line(&args(["fill", "@e30", "--", "--json"])),
            args(["--session", "alfred", "--profile", "/tmp/alfred-profile", "--headed", "--json", "fill", "@e30", "--", "--json"])
        );
        let headless = AgentBrowser { headed: false, ..AgentBrowser::new("x".into(), "/p".into(), true) };
        assert!(!headless.command_line(&args(["get", "url"])).contains(&"--headed".to_string()));
    }

    #[test]
    fn output_bigger_than_a_pipe_does_not_hang() {
        // A stand-in program printing a 300 KB envelope, like a snapshot of a long page.
        let dir = std::env::temp_dir().join(format!("alfred-big-browser-output-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake-agent-browser");
        let filler = "x".repeat(300_000);
        std::fs::write(&script, format!("#!/bin/sh\necho '{{\"success\":true,\"data\":{{\"snapshot\":\"{filler}\"}},\"error\":null}}'\n"))
            .unwrap();
        std::process::Command::new("chmod").arg("+x").arg(&script).status().unwrap();
        let b = AgentBrowser { timeout: Duration::from_secs(10), ..AgentBrowser::new(script, dir.join("profile"), false) };
        let v = b.call(&args(["snapshot", "-i", "-c"])).unwrap();
        assert_eq!(v["snapshot"].as_str().unwrap().len(), 300_000);
        assert!(dir.join("profile").is_dir(), "the profile folder is made on first use");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_hung_program_times_out_and_is_not_retried() {
        let dir = std::env::temp_dir().join(format!("alfred-hung-browser-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("hung-agent-browser");
        std::fs::write(&script, "#!/bin/sh\nsleep 5\n").unwrap();
        std::process::Command::new("chmod").arg("+x").arg(&script).status().unwrap();
        let b = AgentBrowser { timeout: Duration::from_millis(200), ..AgentBrowser::new(script, dir.join("profile"), false) };
        let e = b.call(&args(["click", "@e1"])).unwrap_err();
        assert!(!e.retry_safe && e.message.contains("in time"), "{e:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_program_is_reported() {
        let dir = std::env::temp_dir().join(format!("alfred-missing-browser-{}", std::process::id()));
        let e = AgentBrowser::new("/nope/agent-browser".into(), dir.clone(), false).call(&args(["get", "url"])).unwrap_err();
        assert!(e.message.contains("couldn't start"), "{e}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
