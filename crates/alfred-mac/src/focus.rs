//! What the user is looking at: the app they were in before turning to Alfred, and the address of their
//! browser tab. Only the app's name and the tab's address are read, never a window's title or contents.

use std::process::Command;
use std::time::Duration;

/// Alfred's own bundle: skipped, so the app found is the one the user was in before Alfred came forward.
const ALFRED_BUNDLE: &str = "com.laurentlouk.alfred";

/// The longest a look at the user's app or tab may take: it runs before every request the model decides.
pub const TIMEOUT: Duration = Duration::from_secs(3);

/// How many of the frontmost apps are looked at to get past Alfred.
const LOOK_AT: usize = 3;
/// How far down the front-to-back list a browser is looked for.
const BROWSER_LOOK_AT: usize = 8;

/// Browsers built on Chrome, which share its scripting terms (`URL of active tab of front window`).
const CHROME_FAMILY: &[&str] = &["Google Chrome", "Google Chrome Canary", "Brave Browser", "Microsoft Edge", "Chromium", "Vivaldi", "Arc"];

/// The address of the active tab in the Chrome-family browser named in argv: a constant script, the app's
/// name passed as data.
const CHROME_TAB_SCRIPT: &str = r#"on run argv
    using terms from application "Google Chrome"
        tell application (item 1 of argv) to return URL of active tab of front window
    end using terms from
end run"#;

const SAFARI_TAB_SCRIPT: &str = r#"tell application "Safari" to return URL of current tab of front window"#;

/// The app the user was in: the frontmost visible app that isn't Alfred, by its display name.
pub fn front_app(timeout: Duration) -> Option<String> {
    front_where(timeout, LOOK_AT, |_| true)
}

/// The browser most recently in front, by its display name: what "this page" means when the user asks
/// from another app (Alfred's own panel, an editor beside the browser).
pub fn front_browser(timeout: Duration) -> Option<String> {
    front_where(timeout, BROWSER_LOOK_AT, is_browser)
}

/// The frontmost visible app, among the first `look_at`, that isn't Alfred and passes `wanted`.
fn front_where(timeout: Duration, look_at: usize, wanted: impl Fn(&str) -> bool) -> Option<String> {
    let list = lsappinfo(&["visibleProcessList"], timeout)?;
    for asn in asns(&list).into_iter().take(look_at) {
        let info = lsappinfo(&["info", "-only", "name", "-only", "bundleID", &asn], timeout)?;
        let (name, bundle) = (field(&info, "LSDisplayName"), field(&info, "CFBundleIdentifier"));
        if bundle.as_deref() == Some(ALFRED_BUNDLE) {
            continue;
        }
        if let Some(n) = name.filter(|n| wanted(n)) {
            return Some(n);
        }
    }
    None
}

/// The address of the tab the user is looking at, when `app` is a browser Alfred knows how to ask. The
/// first time, macOS asks the user whether Alfred may control that browser.
pub fn tab_url(app: &str, timeout: Duration) -> Option<String> {
    let url = if CHROME_FAMILY.contains(&app) {
        crate::osascript(CHROME_TAB_SCRIPT, &[app], timeout).ok()?
    } else if app == "Safari" {
        crate::osascript(SAFARI_TAB_SCRIPT, &[], timeout).ok()?
    } else {
        return None;
    };
    let url = url.trim().to_string();
    alfred_core::normalize::url(&url).filter(|u| *u == url)
}

/// Whether `app` is a browser whose tab Alfred can read.
pub fn is_browser(app: &str) -> bool {
    app == "Safari" || CHROME_FAMILY.contains(&app)
}

fn lsappinfo(args: &[&str], timeout: Duration) -> Option<String> {
    let mut cmd = Command::new("/usr/bin/lsappinfo");
    cmd.args(args);
    crate::run(cmd, timeout).ok()
}

/// The application serial numbers in `lsappinfo visibleProcessList` output, front to back.
pub fn asns(list: &str) -> Vec<String> {
    list.split("ASN:")
        .skip(1)
        .filter_map(|part| {
            let id: String = part.chars().take_while(|c| c.is_ascii_hexdigit() || *c == 'x' || *c == '-').collect();
            let id = id.trim_end_matches('-');
            (id.starts_with("0x") && id.contains('-')).then(|| format!("ASN:{id}"))
        })
        .collect()
}

/// A `"key"="value"` line's value from `lsappinfo info` output.
pub fn field(info: &str, key: &str) -> Option<String> {
    let prefix = format!("\"{key}\"=\"");
    info.lines().find_map(|l| l.trim().strip_prefix(&prefix).and_then(|v| v.strip_suffix('"')).map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apps_are_read_front_to_back() {
        let list = r#"ASN:0x0-0x11ce6cd5-"Claude": ASN:0x0-0x10ff7fe7-"Google_Chrome": ASN:0x0-0x123da3c8-"Alfred": "#;
        assert_eq!(asns(list), ["ASN:0x0-0x11ce6cd5", "ASN:0x0-0x10ff7fe7", "ASN:0x0-0x123da3c8"]);
        assert_eq!(asns("nothing here"), Vec::<String>::new());
    }

    #[test]
    fn info_fields_are_read_by_key() {
        let info = "\"LSDisplayName\"=\"Google Chrome\"\n\"CFBundleIdentifier\"=\"com.google.Chrome\"\n";
        assert_eq!(field(info, "LSDisplayName").as_deref(), Some("Google Chrome"));
        assert_eq!(field(info, "CFBundleIdentifier").as_deref(), Some("com.google.Chrome"));
        assert_eq!(field(info, "Missing"), None);
    }

    #[test]
    fn only_known_browsers_are_asked_for_their_tab() {
        assert!(is_browser("Google Chrome") && is_browser("Safari") && is_browser("Arc"));
        assert!(!is_browser("Claude") && !is_browser("Mail"));
        assert_eq!(tab_url("Claude", Duration::from_millis(10)), None, "no script runs for other apps");
    }

    #[test]
    fn the_tab_scripts_take_no_user_text() {
        assert!(!CHROME_TAB_SCRIPT.contains("{}") && CHROME_TAB_SCRIPT.contains("item 1 of argv"));
        assert!(!SAFARI_TAB_SCRIPT.contains("argv"));
    }
}
