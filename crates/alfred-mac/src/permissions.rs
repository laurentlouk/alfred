//! The Accessibility permission that on-screen actions need. macOS grants it to the app that launches the
//! tools (Alfred, or the terminal running `alfred`), so asking from a helper Alfred starts shows macOS's own
//! dialog for Alfred: "Alfred would like to control this computer using accessibility features", with
//! Open System Settings. Nothing here clicks or changes a setting: the user decides in that dialog.

use std::process::Command;
use std::time::Duration;

/// Whether this app is trusted for Accessibility, asking macOS to show its dialog when argv says `ask`. A
/// constant script; the only argument is that word.
const AX_SCRIPT: &str = r#"ObjC.import("ApplicationServices");
function run(argv) {
    return $.AXIsProcessTrustedWithOptions($({ AXTrustedCheckOptionPrompt: argv[0] === "ask" })) ? "yes" : "no";
}"#;

/// System Settings, opened at Privacy & Security, Accessibility.
pub const ACCESSIBILITY_SETTINGS: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

/// What Alfred says the first time it finds the permission missing, with System Settings open at the list.
pub const ASKED: &str = "To work in other apps I need your permission. I've opened System Settings at Privacy & Security, Accessibility: turn on Alfred, then ask me again.";

/// What Alfred says when it's still missing.
pub const STILL_MISSING: &str = "I still need the Accessibility permission. I've opened System Settings at Privacy & Security, Accessibility again: turn on Alfred, then ask me again.";

const TIMEOUT: Duration = Duration::from_secs(5);

/// Whether Alfred may use apps on screen. `None` when it couldn't be told, which never blocks an action.
pub fn accessibility_allowed() -> Option<bool> {
    trust("check")
}

/// Ask for the permission, the way Mac apps do: the first time, macOS's own request, which also puts Alfred
/// in the Accessibility list; every time, System Settings opened at that list, so turning Alfred on is one
/// click.
pub fn ask_for_accessibility(asked_before: bool) {
    if !asked_before {
        trust("ask");
    }
    let mut open = Command::new("/usr/bin/open");
    open.arg(ACCESSIBILITY_SETTINGS);
    crate::run(open, TIMEOUT).ok();
}

fn trust(mode: &str) -> Option<bool> {
    let mut cmd = Command::new("/usr/bin/osascript");
    cmd.args(["-l", "JavaScript", "-e", AX_SCRIPT, mode]);
    match crate::run(cmd, TIMEOUT).ok()?.trim() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_check_runs_and_says_yes_or_no() {
        // Whatever this machine's answer, the check itself works and never shows a dialog.
        assert!(accessibility_allowed().is_some());
    }

    #[test]
    fn the_script_takes_no_user_text() {
        assert!(AX_SCRIPT.contains(r#"argv[0] === "ask""#) && !AX_SCRIPT.contains("{}"));
        assert!(ACCESSIBILITY_SETTINGS.starts_with("x-apple.systempreferences:"));
    }
}
