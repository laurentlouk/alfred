//! Claude and Claude Code through their official links: a new Claude Code session in the Claude app
//! (`claude://code/new`) or in a terminal (`claude-cli://open`), and a new chat (`claude://claude.ai/new`),
//! each with the prompt filled in. A link never sends anything: the user reads the prompt and presses
//! Return, and the Claude app asks before it uses a folder that came from a link.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use alfred_core::Outcome;
use alfred_core::catalog::fold;

use crate::run;

/// Where a Claude Code session opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpensIn {
    /// The Claude app's Code tab.
    Desktop,
    /// Claude Code in a terminal window.
    Terminal,
}

/// How Claude Code is reached on this Mac.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeSettings {
    pub opens_in: OpensIn,
    /// Folders whose projects a spoken folder name is looked for in, before the home folder.
    pub roots: Vec<PathBuf>,
}

impl Default for ClaudeSettings {
    fn default() -> Self {
        Self { opens_in: OpensIn::Desktop, roots: Vec::new() }
    }
}

/// The longest prompt each link takes (the Claude app clips at about 14,000 characters, Claude Code's
/// link refuses more than 5,000), with room to spare.
const DESKTOP_LIMIT: usize = 12_000;
const TERMINAL_LIMIT: usize = 4_900;

/// Folders of the home folder that hold no projects, skipped when looking for one.
const NOT_PROJECTS: &[&str] = &["Library", "Applications", "Movies", "Music", "Pictures", "Public", "node_modules", "target"];

/// Open a new Claude Code session with `prompt`, in the folder the user named when it is found.
pub fn open_code(prompt: &str, folder: Option<&str>, settings: &ClaudeSettings, timeout: Duration) -> Result<Outcome, String> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let found = folder.and_then(|name| find_folder(name, &settings.roots, &home));
    let limit = if settings.opens_in == OpensIn::Desktop { DESKTOP_LIMIT } else { TERMINAL_LIMIT };
    let (prompt, clipped) = clip_words(prompt.trim(), limit);
    open_link(&code_link(&prompt, found.as_deref(), settings.opens_in), timeout)?;
    Ok(Outcome::new(code_summary(&prompt, folder, found.as_deref(), settings.opens_in, clipped)))
}

/// Open a new Claude chat with `prompt` in it.
pub fn open_chat(prompt: &str, timeout: Duration) -> Result<Outcome, String> {
    let (prompt, clipped) = clip_words(prompt.trim(), DESKTOP_LIMIT);
    open_link(&chat_link(&prompt), timeout)?;
    let mut s = "I've written your prompt in a new Claude chat: press Return to send it.".to_string();
    if clipped {
        s.push_str(" It was long, so I kept its beginning.");
    }
    Ok(Outcome::new(s))
}

fn open_link(link: &str, timeout: Duration) -> Result<(), String> {
    let mut cmd = Command::new("open");
    cmd.arg(link);
    run(cmd, timeout).map(|_| ()).map_err(|e| {
        if e.contains("-10814") || e.contains("No application") {
            "I couldn't find the Claude app or Claude Code on this Mac.".to_string()
        } else {
            e
        }
    })
}

/// The link for a new Claude Code session.
pub fn code_link(prompt: &str, folder: Option<&Path>, opens_in: OpensIn) -> String {
    let (base, folder_key) = match opens_in {
        OpensIn::Desktop => ("claude://code/new", "folder"),
        OpensIn::Terminal => ("claude-cli://open", "cwd"),
    };
    let mut params = Vec::new();
    if !prompt.is_empty() {
        params.push(format!("q={}", percent_encode(prompt)));
    }
    if let Some(f) = folder {
        params.push(format!("{folder_key}={}", percent_encode(&f.display().to_string())));
    }
    if params.is_empty() { base.to_string() } else { format!("{base}?{}", params.join("&")) }
}

/// The link for a new Claude chat.
pub fn chat_link(prompt: &str) -> String {
    if prompt.is_empty() { "claude://claude.ai/new".into() } else { format!("claude://claude.ai/new?q={}", percent_encode(prompt)) }
}

/// What Alfred says once the link is open.
pub fn code_summary(prompt: &str, asked: Option<&str>, found: Option<&Path>, opens_in: OpensIn, clipped: bool) -> String {
    let folder = found.and_then(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned());
    let send = "check it and press Return to send it.";
    let mut s = match (prompt.is_empty(), &folder, opens_in) {
        (true, Some(f), OpensIn::Desktop) => format!("I've opened a new Claude Code session in {f}."),
        (true, None, OpensIn::Desktop) => "I've opened a new Claude Code session.".to_string(),
        (true, Some(f), OpensIn::Terminal) => format!("I've opened Claude Code in a terminal, in {f}."),
        (true, None, OpensIn::Terminal) => "I've opened Claude Code in a terminal.".to_string(),
        (false, Some(f), OpensIn::Desktop) => format!("Claude Code is open in {f} with your prompt: {send}"),
        (false, None, OpensIn::Desktop) => format!("Claude Code is open with your prompt: {send}"),
        (false, Some(f), OpensIn::Terminal) => format!("Claude Code is open in a terminal, in {f}, with your prompt: {send}"),
        (false, None, OpensIn::Terminal) => format!("Claude Code is open in a terminal with your prompt: {send}"),
    };
    if folder.is_some() && opens_in == OpensIn::Desktop {
        s.push_str(" Claude asks you to confirm the folder first.");
    }
    if let (Some(name), None) = (asked, &folder) {
        s.push_str(&format!(" I didn't find a folder called “{name}”, so it uses its usual one."));
    }
    if clipped {
        s.push_str(" Your prompt was long, so I kept its beginning.");
    }
    s
}

/// The folder a spoken name stands for: in `roots`, then in the home folder, two levels down, the first
/// whose name is the name said (ignoring case, accents and dashes), else the only one whose name has it.
pub fn find_folder(name: &str, roots: &[PathBuf], home: &Path) -> Option<PathBuf> {
    let want = fold(name);
    if want.is_empty() {
        return None;
    }
    let mut candidates = Vec::new();
    for root in roots.iter().map(|r| expand_home(r, home)).chain([home.to_path_buf()]) {
        for child in subfolders(&root) {
            candidates.push(child.clone());
            candidates.extend(subfolders(&child));
        }
    }
    let named = |p: &PathBuf| p.file_name().map(|n| fold(&n.to_string_lossy())).unwrap_or_default();
    if let Some(exact) = candidates.iter().find(|p| named(p) == want) {
        return Some(exact.clone());
    }
    if want.chars().count() < 4 {
        return None;
    }
    let mut partial = candidates.iter().filter(|p| named(p).contains(&want));
    match (partial.next(), partial.next()) {
        (Some(only), None) => Some(only.clone()),
        _ => None,
    }
}

fn expand_home(p: &Path, home: &Path) -> PathBuf {
    match p.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => p.to_path_buf(),
    }
}

/// The visible folders inside `dir`, in name order, without the ones that never hold projects.
fn subfolders(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            !n.starts_with('.') && !NOT_PROJECTS.contains(&n.as_str())
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// `s` within `max` characters, cut at a word, and whether it was cut.
fn clip_words(s: &str, max: usize) -> (String, bool) {
    if s.chars().count() <= max {
        return (s.to_string(), false);
    }
    let cut: String = s.chars().take(max).collect();
    let at = cut.rfind(char::is_whitespace).unwrap_or(cut.len());
    (cut[..at].trim_end().to_string(), true)
}

/// RFC 3986 percent-encoding: letters, digits and `-._~` stay, every other byte is `%XX` (a space is
/// `%20`, never `+`, which a link reader may keep as a plus).
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_carry_the_prompt_and_the_folder_encoded() {
        let folder = Path::new("/Users/me/Code/my app");
        assert_eq!(
            code_link("Fix the failing test\nthen run it", Some(folder), OpensIn::Desktop),
            "claude://code/new?q=Fix%20the%20failing%20test%0Athen%20run%20it&folder=%2FUsers%2Fme%2FCode%2Fmy%20app"
        );
        assert_eq!(
            code_link("Explain this repo", Some(folder), OpensIn::Terminal),
            "claude-cli://open?q=Explain%20this%20repo&cwd=%2FUsers%2Fme%2FCode%2Fmy%20app"
        );
        assert_eq!(code_link("", None, OpensIn::Desktop), "claude://code/new");
        assert_eq!(chat_link("What's new & why?"), "claude://claude.ai/new?q=What%27s%20new%20%26%20why%3F");
        assert_eq!(percent_encode("é"), "%C3%A9");
    }

    #[test]
    fn a_spoken_folder_is_found_two_levels_down() {
        let home = std::env::temp_dir().join(format!("alfred-claude-home-{}", std::process::id()));
        for d in ["Awesome/motion-video", "Awesome/alfred", "Library/alfred-not-this", "Code/Alfred Docs", "Code/notes"] {
            std::fs::create_dir_all(home.join(d)).unwrap();
        }
        assert_eq!(find_folder("alfred", &[], &home), Some(home.join("Awesome/alfred")));
        assert_eq!(find_folder("Motion Video", &[], &home), Some(home.join("Awesome/motion-video")), "dashes and case don't matter");
        assert_eq!(find_folder("doc", &[], &home), None, "too short to guess from a part");
        assert_eq!(find_folder("docs", &[], &home), Some(home.join("Code/Alfred Docs")), "the only folder with it");
        assert_eq!(find_folder("alfred docs", &[], &home), Some(home.join("Code/Alfred Docs")));
        assert_eq!(find_folder("not this", &[], &home), None, "Library is never searched");
        assert_eq!(find_folder("nowhere", &[], &home), None);
        // A configured root comes first.
        std::fs::create_dir_all(home.join("Work/clients/alfred")).unwrap();
        assert_eq!(find_folder("alfred", &[PathBuf::from("~/Work")], &home), Some(home.join("Work/clients/alfred")));
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn what_alfred_says_tells_what_to_do_next() {
        let f = Path::new("/Users/me/Awesome/alfred");
        assert_eq!(
            code_summary("Fix it", Some("alfred"), Some(f), OpensIn::Desktop, false),
            "Claude Code is open in alfred with your prompt: check it and press Return to send it. Claude asks you to confirm the folder first."
        );
        assert_eq!(
            code_summary("Fix it", Some("nowhere"), None, OpensIn::Desktop, false),
            "Claude Code is open with your prompt: check it and press Return to send it. I didn't find a folder called “nowhere”, so it uses its usual one."
        );
        assert_eq!(code_summary("", None, None, OpensIn::Terminal, false), "I've opened Claude Code in a terminal.");
        assert!(code_summary("x", None, None, OpensIn::Desktop, true).ends_with("so I kept its beginning."));
    }

    #[test]
    fn long_prompts_are_cut_at_a_word() {
        assert_eq!(clip_words("one two three", 9), ("one two".to_string(), true));
        assert_eq!(clip_words("short", 9), ("short".to_string(), false));
    }
}
