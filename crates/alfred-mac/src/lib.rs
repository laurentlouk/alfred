//! Alfred's hands on a Mac: `open` for apps and links, `osascript` for Notes, Reminders, Mail and quitting.
//!
//! User text never becomes AppleScript source. Every script here is a constant that reads its inputs from
//! `on run argv`, and the inputs travel as separate process arguments, so a note body such as
//! `" & do shell script "…` is just text.

pub mod focus;
pub mod mail;
pub mod permissions;
pub mod shortcuts;

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alfred_core::action::{BROWSER_APP, WebHands};
use alfred_core::normalize::search_url;
use alfred_core::{Action, Executor, Hands, Outcome};

/// Runs actions for real on this Mac.
#[derive(Clone)]
pub struct MacExecutor {
    /// `google`, `duckduckgo` or `bing`.
    pub search_engine: String,
    /// The browser for searches and links when the request doesn't name one; `None` for the default.
    pub browser: Option<String>,
    /// How long an AppleScript may run. The first one to an app can wait on the user's permission prompt.
    pub timeout: Duration,
    /// Bring Notes forward to show a note it just saved.
    pub show_notes: bool,
    /// How long a shortcut may run (one that asks for input waits until this runs out).
    pub shortcut_timeout: Duration,
    /// The on-screen engine for `use_app` and `press_button`, when `agent-desktop` is set up.
    pub hands: Option<Arc<dyn Hands>>,
    /// The browser engine for `use_browser`, its held clicks, and answering searches, when `agent-browser`
    /// is set up.
    pub web: Option<Arc<dyn WebHands>>,
    /// Check the Accessibility permission before on-screen actions, and ask for it when it's missing.
    pub check_accessibility: bool,
    /// macOS's own permission dialog was shown already: the next time, System Settings opens instead.
    asked_accessibility: Arc<std::sync::atomic::AtomicBool>,
}

impl Default for MacExecutor {
    fn default() -> Self {
        Self {
            search_engine: "google".into(),
            browser: None,
            timeout: Duration::from_secs(60),
            show_notes: true,
            shortcut_timeout: Duration::from_secs(120),
            hands: None,
            web: None,
            check_accessibility: false,
            asked_accessibility: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

impl MacExecutor {
    pub fn from_config(c: &alfred_core::Config) -> Self {
        Self {
            search_engine: c.search.engine.clone(),
            browser: Some(c.search.browser.trim().to_string()).filter(|b| !b.is_empty()),
            check_accessibility: true,
            ..Self::default()
        }
    }

    pub fn with_hands(mut self, hands: Arc<dyn Hands>) -> Self {
        self.hands = Some(hands);
        self
    }

    pub fn with_web_hands(mut self, web: Arc<dyn WebHands>) -> Self {
        self.web = Some(web);
        self
    }

    fn web(&self) -> Result<&Arc<dyn WebHands>, String> {
        self.web.as_ref().ok_or_else(|| {
            "Doing things on websites needs agent-browser: `brew install agent-browser` (it uses Google Chrome).".to_string()
        })
    }

    fn hands(&self) -> Result<&Arc<dyn Hands>, String> {
        let hands = self.hands.as_ref().ok_or_else(|| {
            "Operating apps on screen needs agent-desktop: `brew install agent-desktop`, then allow Alfred in System Settings → Privacy & Security → Accessibility.".to_string()
        })?;
        // Without Accessibility every on-screen step fails: ask for it, the way any Mac app does. The engine's
        // own view comes first (it is what fails), macOS's answer to Alfred when it can't tell.
        if self.check_accessibility && hands.allowed().or_else(permissions::accessibility_allowed) == Some(false) {
            let asked_before = self.asked_accessibility.swap(true, std::sync::atomic::Ordering::SeqCst);
            permissions::ask_for_accessibility(asked_before);
            return Err(if asked_before { permissions::STILL_MISSING } else { permissions::ASKED }.to_string());
        }
        Ok(hands)
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
                // Alfred's own browser reads the results and answers, unless the request named a browser. When
                // it can't read them (a robot check with its window hidden, a broken install), the results open
                // as before.
                if browser.is_none()
                    && let Some(web) = &self.web
                    && let Ok(answer) = web.answer(query)
                {
                    return Ok(answer);
                }
                self.open_in_browser(&search_url(&self.search_engine, query), browser.as_deref())?;
                Ok(Outcome::new(action.done_phrase()))
            }
            Action::OpenUrl { url, browser } => {
                // Refuse anything that isn't http(s) even if a caller built the action by hand.
                if alfred_core::normalize::url(url).as_deref() != Some(url.as_str()) {
                    return Err(format!("{url} is not a web address I can open."));
                }
                // The site opens in Alfred's browser, where follow-ups ("what do you see?") can look at it and
                // the user's sign-ins there are kept. A browser the user named still gets it.
                if browser.is_none()
                    && let Some(web) = &self.web
                    && let Ok(shown) = web.show(url)
                {
                    return Ok(shown);
                }
                self.open_in_browser(url, browser.as_deref())?;
                Ok(Outcome::new(action.done_phrase()))
            }
            Action::RunShortcut { name, input } => {
                let output = shortcuts::run(name, input.as_deref(), self.shortcut_timeout)?;
                let said = output.trim();
                Ok(Outcome::new(if said.is_empty() { action.done_phrase() } else { clip(said, 400) }))
            }
            // A browser is driven through its pages, with agent-browser, never through its window: a task
            // "in Chrome" works on the tab it shows.
            Action::UseApp { app, task } if focus::is_browser(app) && self.web.is_some() => {
                let url = focus::tab_url(app, focus::TIMEOUT)
                    .ok_or_else(|| format!("I can't see a web page in {app}; Alfred may need permission to read it (System Settings → Privacy & Security → Automation)."))?;
                self.web()?.browse_theirs(task, &url)
            }
            Action::UseApp { app, task } => self.hands()?.use_app(app, task),
            Action::SendMessage { app, to, text } => self.hands()?.send_message(app, to.as_deref(), text),
            Action::PressButton { app, role, name } if app == BROWSER_APP => self.web()?.press(role, name),
            Action::PressButton { app, role, name } => self.hands()?.press(app, role, name),
            Action::UseBrowser { task, url } => self.web()?.browse(task, url.as_deref()),
            Action::UseTheirTab { task } => {
                let web = self.web()?;
                // The browser most recently in front: the user may ask from another app, or from Alfred.
                let app =
                    focus::front_browser(focus::TIMEOUT).ok_or("I don't see a browser window open; open the page and ask me again.")?;
                let url = focus::tab_url(&app, focus::TIMEOUT)
                    .ok_or_else(|| format!("I can't see a web page in {app}; Alfred may need permission to read it (System Settings → Privacy & Security → Automation)."))?;
                web.browse_theirs(task, &url)
            }
            Action::ReadMail { from } => mail::read(from.as_deref(), self.timeout),
            Action::DraftReply { from, text } => mail::reply(from.as_deref(), text, self.timeout),
            Action::GetTime => {
                let mut cmd = Command::new("date");
                cmd.arg("+%H:%M on %A %-d %B");
                Ok(Outcome::new(format!("It's {}.", run(cmd, self.timeout)?.trim())))
            }
        }
    }

    fn context(&self) -> alfred_core::context::Context {
        let app = focus::front_app(focus::TIMEOUT);
        let site = app.as_deref().filter(|a| focus::is_browser(a)).and_then(|a| focus::tab_url(a, focus::TIMEOUT));
        let alfred_site = self.web.as_ref().and_then(|w| w.page());
        alfred_core::context::Context::new(app.as_deref(), site.as_deref(), alfred_site.as_deref())
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { s.chars().take(max - 1).collect::<String>() + "…" }
}

/// Where a program lives: a full path as given, else the first match on PATH or in the folders Homebrew,
/// npm and friends install to. An app opened from Finder gets a minimal PATH, so these are searched too.
pub fn find_program(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let p = PathBuf::from(name);
        return p.is_file().then_some(p);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend(
        ["/opt/homebrew/bin", "/usr/local/bin"]
            .map(PathBuf::from)
            .into_iter()
            .chain([".local/bin", ".bun/bin", ".npm-global/bin"].map(|d| home.join(d))),
    );
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
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
pub fn run(mut cmd: Command, timeout: Duration) -> Result<String, String> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't start {program}: {e}"))?;
    // Read while it runs, so a chatty command can't block on a full pipe.
    let out_reader = child.stdout.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut text = String::new();
            s.read_to_string(&mut text).ok();
            text
        })
    });
    let err_reader = child.stderr.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut text = String::new();
            s.read_to_string(&mut text).ok();
            text
        })
    });
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
    let out = out_reader.and_then(|r| r.join().ok()).unwrap_or_default();
    let err = err_reader.and_then(|r| r.join().ok()).unwrap_or_default();
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
    fn on_screen_actions_explain_how_to_set_them_up() {
        let e = MacExecutor::default().execute(&Action::UseApp { app: "Mail".into(), task: "x".into() }).unwrap_err();
        assert!(e.contains("brew install agent-desktop") && e.contains("Accessibility"), "{e}");
    }

    /// Browser hands that record what they were asked and answer from a script.
    #[derive(Default)]
    struct FakeWeb {
        asked: std::sync::Mutex<Vec<String>>,
        answer: Option<String>,
    }

    impl WebHands for FakeWeb {
        fn browse(&self, task: &str, url: Option<&str>) -> Result<Outcome, String> {
            self.asked.lock().unwrap().push(format!("browse {task} {url:?}"));
            Ok(Outcome::new("browsed"))
        }

        fn press(&self, role: &str, name: &str) -> Result<Outcome, String> {
            self.asked.lock().unwrap().push(format!("press {role} {name}"));
            Ok(Outcome::new("pressed"))
        }

        fn answer(&self, query: &str) -> Result<Outcome, String> {
            self.asked.lock().unwrap().push(format!("answer {query}"));
            self.answer.clone().map(Outcome::new).ok_or_else(|| "robot check".to_string())
        }
    }

    /// An executor whose own browser can't be found, so a fallback to it fails instead of opening a window.
    fn with_web(web: Arc<FakeWeb>) -> MacExecutor {
        MacExecutor { browser: Some("Definitely Not A Browser 7f3a".into()), ..MacExecutor::default() }.with_web_hands(web)
    }

    #[test]
    fn browser_tasks_and_their_held_clicks_go_to_the_browser_hands() {
        let web = Arc::new(FakeWeb::default());
        let exec = with_web(web.clone());
        let task = Action::UseBrowser { task: "find the price".into(), url: Some("https://example.com".into()) };
        assert_eq!(exec.execute(&task).unwrap().summary, "browsed");
        let press = Action::PressButton { app: BROWSER_APP.into(), role: "button".into(), name: "Place order".into() };
        assert_eq!(exec.execute(&press).unwrap().summary, "pressed");
        assert_eq!(*web.asked.lock().unwrap(), ["browse find the price Some(\"https://example.com\")", "press button Place order"]);
        let e = exec.execute(&Action::PressButton { app: "Mail".into(), role: "button".into(), name: "Send".into() }).unwrap_err();
        assert!(e.contains("agent-desktop"), "a held click in an app still goes to the app hands: {e}");
    }

    #[test]
    fn without_agent_browser_browser_tasks_say_how_to_install_it() {
        let e = MacExecutor::default().execute(&Action::UseBrowser { task: "x".into(), url: None }).unwrap_err();
        assert!(e.contains("brew install agent-browser"), "{e}");
    }

    #[test]
    fn searches_are_answered_by_the_browser_hands() {
        let web =
            Arc::new(FakeWeb { answer: Some("It's 12°C in Paris, according to Google's weather card.".into()), ..FakeWeb::default() });
        let o = with_web(web.clone()).execute(&Action::WebSearch { query: "weather in Paris".into(), browser: None }).unwrap();
        assert_eq!(o.summary, "It's 12°C in Paris, according to Google's weather card.");
        assert_eq!(*web.asked.lock().unwrap(), ["answer weather in Paris"]);
    }

    #[test]
    fn a_search_falls_back_to_opening_the_results_when_they_cant_be_read() {
        // The fallback opens the results in the configured browser, which doesn't exist here: the error
        // shows the old path ran.
        let web = Arc::new(FakeWeb::default());
        let e = with_web(web.clone()).execute(&Action::WebSearch { query: "weather in Paris".into(), browser: None }).unwrap_err();
        assert!(e.contains("Definitely Not A Browser 7f3a") || e.contains("Unable to find"), "{e}");
        assert_eq!(*web.asked.lock().unwrap(), ["answer weather in Paris"]);
        // A browser the user named is used as before, without the hands.
        let web = Arc::new(FakeWeb { answer: Some("x".into()), ..FakeWeb::default() });
        let named = Action::WebSearch { query: "rune".into(), browser: Some("Definitely Not A Browser 7f3a".into()) };
        assert!(with_web(web.clone()).execute(&named).is_err());
        assert!(web.asked.lock().unwrap().is_empty());
        // No hands at all: today's behaviour, the results open in the browser.
        let plain = MacExecutor { browser: Some("Definitely Not A Browser 7f3a".into()), ..MacExecutor::default() };
        assert!(plain.execute(&Action::WebSearch { query: "rune".into(), browser: None }).is_err());
    }

    #[test]
    fn programs_are_found_by_name_or_path() {
        assert_eq!(find_program("/bin/ls"), Some(PathBuf::from("/bin/ls")));
        assert!(find_program("ls").is_some());
        assert_eq!(find_program("definitely-not-a-program-7f3a"), None);
        assert_eq!(find_program("/nope/agent-desktop"), None);
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
