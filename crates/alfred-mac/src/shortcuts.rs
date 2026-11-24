//! The user's Shortcuts, through the `shortcuts` command. Input and output travel through files in a
//! private temporary folder, never on the command line.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// The user's shortcut names, as the Shortcuts app shows them. Empty when the command is missing.
pub fn list(timeout: Duration) -> Result<Vec<String>, String> {
    let mut cmd = Command::new("shortcuts");
    cmd.arg("list");
    let out = crate::run(cmd, timeout)?;
    Ok(parse_list(&out))
}

pub fn parse_list(out: &str) -> Vec<String> {
    let mut names: Vec<String> = out.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
    names.dedup();
    names
}

/// Run a shortcut and return its text output (empty when it has none).
pub fn run(name: &str, input: Option<&str>, timeout: Duration) -> Result<String, String> {
    let dir = scratch_dir()?;
    let result = run_in(&dir, name, input, timeout);
    std::fs::remove_dir_all(&dir).ok();
    result
}

fn run_in(dir: &std::path::Path, name: &str, input: Option<&str>, timeout: Duration) -> Result<String, String> {
    let output = dir.join("output.txt");
    let mut cmd = Command::new("shortcuts");
    cmd.arg("run").arg(name);
    if let Some(text) = input.filter(|t| !t.trim().is_empty()) {
        let path = dir.join("input.txt");
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        cmd.arg("--input-path").arg(&path);
    }
    cmd.arg("--output-path").arg(&output).arg("--output-type").arg("public.plain-text");
    crate::run(cmd, timeout).map_err(|e| explain(name, &e))?;
    Ok(std::fs::read_to_string(&output).unwrap_or_default().trim().to_string())
}

fn explain(name: &str, e: &str) -> String {
    let lower = e.to_lowercase();
    // The command's messages follow the system language: "Raccourci introuvable" on a French Mac.
    if ["couldn’t find", "couldn't find", "not found", "introuvable"].iter().any(|w| lower.contains(w)) {
        format!("I couldn't find a shortcut called “{name}”.")
    } else if lower.contains("timed out") {
        format!("The shortcut “{name}” didn't finish in time (it may be waiting for an answer in the Shortcuts app).")
    } else {
        format!("The shortcut “{name}” failed: {}", e.trim().trim_start_matches("Error:").trim())
    }
}

/// A folder only this process uses, for one run's files.
fn scratch_dir() -> Result<PathBuf, String> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("alfred-shortcut-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_are_trimmed_lines() {
        assert_eq!(parse_list("Focus Timer\n  Bonjour \n\nRéveil\n"), vec!["Focus Timer", "Bonjour", "Réveil"]);
        assert!(parse_list("").is_empty());
    }

    #[test]
    fn a_missing_shortcut_is_reported_by_name() {
        if !cfg!(target_os = "macos") || crate::find_program("shortcuts").is_none() {
            return;
        }
        // Never runs a real shortcut: the name doesn't exist.
        let e = run("Alfred test shortcut that does not exist 7f3a", Some("hello"), Duration::from_secs(30)).unwrap_err();
        assert_eq!(e, "I couldn't find a shortcut called “Alfred test shortcut that does not exist 7f3a”.");
    }

    #[test]
    fn errors_are_explained() {
        assert_eq!(explain("X", "Error: Couldn’t find shortcut “X”"), "I couldn't find a shortcut called “X”.");
        assert_eq!(
            explain("X", "Error: L’opération n’a pas pu s’achever. Raccourci introuvable"),
            "I couldn't find a shortcut called “X”."
        );
        assert!(explain("X", "shortcuts timed out after 120s").contains("didn't finish"));
        assert_eq!(explain("X", "Error: boom"), "The shortcut “X” failed: boom");
    }
}
