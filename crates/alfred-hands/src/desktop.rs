//! The `agent-desktop` command: every call prints one JSON envelope, `{ok, data}` or `{ok: false, error}`.
//! [`Desktop`] is the seam: [`AgentDesktop`] runs the real program, tests pass a recorded app.

use std::fmt;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// A failed call, as agent-desktop reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopError {
    /// agent-desktop's code (`STALE_REF`, `PERM_DENIED`, …), or `SPAWN`, `TIMEOUT_PROCESS`, `BAD_OUTPUT`
    /// for failures around the program itself.
    pub code: String,
    pub message: String,
    /// Only `true` when agent-desktop says nothing was delivered, so trying again cannot act twice.
    pub retry_safe: bool,
}

impl DesktopError {
    fn local(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into(), retry_safe: true }
    }
}

impl fmt::Display for DesktopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

/// One call to agent-desktop: the arguments after the program name, and text for `--stdin`.
pub trait Desktop: Send + Sync {
    fn call(&self, args: &[String], stdin: Option<&str>) -> Result<Value, DesktopError>;
}

/// The real program.
pub struct AgentDesktop {
    pub program: PathBuf,
    pub timeout: Duration,
}

impl AgentDesktop {
    pub fn new(program: PathBuf) -> Self {
        Self { program, timeout: Duration::from_secs(40) }
    }
}

impl Desktop for AgentDesktop {
    fn call(&self, args: &[String], stdin: Option<&str>) -> Result<Value, DesktopError> {
        let mut child = Command::new(&self.program)
            .args(args)
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| DesktopError::local("SPAWN", format!("couldn't start {}: {e}", self.program.display())))?;
        if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
            pipe.write_all(text.as_bytes()).ok();
        }
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if started.elapsed() > self.timeout => {
                    child.kill().ok();
                    child.wait().ok();
                    // The action may have been delivered before the program hung: not safe to repeat.
                    return Err(DesktopError {
                        code: "TIMEOUT_PROCESS".into(),
                        message: "agent-desktop didn't answer in time".into(),
                        retry_safe: false,
                    });
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => return Err(DesktopError::local("SPAWN", e.to_string())),
            }
        }
        let mut out = String::new();
        if let Some(mut s) = child.stdout.take() {
            s.read_to_string(&mut out).ok();
        }
        parse_envelope(&out)
    }
}

/// `data` from a successful envelope, or the error it carries.
pub fn parse_envelope(out: &str) -> Result<Value, DesktopError> {
    let v: Value = serde_json::from_str(out.trim())
        .map_err(|_| DesktopError::local("BAD_OUTPUT", format!("unexpected output: {}", out.chars().take(120).collect::<String>())))?;
    if v["ok"].as_bool() == Some(true) {
        return Ok(v["data"].clone());
    }
    let e = &v["error"];
    Err(DesktopError {
        code: e["code"].as_str().unwrap_or("UNKNOWN").to_string(),
        message: e["message"].as_str().unwrap_or("agent-desktop failed").to_string(),
        retry_safe: e["disposition"]["retry"].as_str() == Some("safe"),
    })
}

/// Arguments as owned strings: `args(["click", "@s1:e2"])`.
pub fn args<const N: usize>(parts: [&str; N]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_give_data_or_a_typed_error() {
        assert_eq!(parse_envelope(r#"{"version":"2.6","ok":true,"command":"click","data":{"x":1}}"#).unwrap(), serde_json::json!({"x":1}));
        let e = parse_envelope(
            r#"{"ok":false,"error":{"code":"STALE_REF","message":"gone","disposition":{"delivery":"not_delivered","retry":"safe"}}}"#,
        )
        .unwrap_err();
        assert_eq!((e.code.as_str(), e.retry_safe), ("STALE_REF", true));
        let e =
            parse_envelope(r#"{"ok":false,"error":{"code":"ACTION_FAILED","message":"x","disposition":{"retry":"unsafe"}}}"#).unwrap_err();
        assert!(!e.retry_safe);
        assert_eq!(parse_envelope("panic!").unwrap_err().code, "BAD_OUTPUT");
    }

    #[test]
    fn the_real_program_reports_its_version() {
        let Some(program) =
            ["/opt/homebrew/bin/agent-desktop", "/usr/local/bin/agent-desktop"].iter().map(PathBuf::from).find(|p| p.is_file())
        else {
            return; // not installed here
        };
        let v = AgentDesktop::new(program).call(&args(["version"]), None).unwrap();
        assert!(v["version"].as_str().is_some_and(|s| !s.is_empty()), "{v}");
    }

    #[test]
    fn a_missing_program_is_a_spawn_error() {
        let e = AgentDesktop::new("/nope/agent-desktop".into()).call(&args(["version"]), None).unwrap_err();
        assert_eq!(e.code, "SPAWN");
    }
}
