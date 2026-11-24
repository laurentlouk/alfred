//! Headless commands. Each returns the process exit code.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use alfred_core::config::{self, Config, Provider};
use alfred_core::llm::{self, Ollama};
use alfred_core::{Agent, DryRun, Event, Executor};
use alfred_voice::{Listener, Speaker, models};

use crate::speech::{self, Playback};

const HELP: &str = "Alfred, a local assistant for your Mac.

USAGE
  alfred                      open Alfred (summon it any time with the shortcut, ctrl+shift+space by default)
  alfred ask [options] TEXT   do what TEXT asks and print the answer
      --dry-run                 show the actions instead of running them
      --yes                     answer yes when Alfred asks to confirm (including a Send it stopped before)
      --speak                   also say the answer out loud
  alfred chat [--dry-run]     a conversation: one request per line (typed, or piped in), each knowing
                              the ones before it and what Alfred's browser shows
  alfred say TEXT             speak TEXT with Alfred's voice
  alfred listen FILE.wav      transcribe a recording
  alfred setup                write the config, download the voice models, pull the language model
  alfred doctor               check what is ready and what is missing
  alfred shortcuts            list the Shortcuts Alfred can run
  alfred recordings [clear]   what Alfred has recorded of its own runs (when you turn recording on), or
                              delete it all (clear asks first; --yes skips the question)
  alfred eval [--model] [FILE]  score the router (and with --model, the language model) on a fixture
                              (eval/utterances.toml by default; eval/apps.toml covers shortcuts and apps)

FILES
  config  ~/Library/Application Support/Alfred/config.toml   (ALFRED_HOME overrides the folder)
";

/// The fixture shipped with the binary, used when `alfred eval` gets no file.
const FIXTURE: &str = include_str!("../../../eval/utterances.toml");

pub fn run(args: &[String]) -> i32 {
    let rest: Vec<String> = args[1..].to_vec();
    let result = match args[0].as_str() {
        "ask" => ask(&rest),
        "chat" => chat(&rest),
        "say" => say(&rest.join(" ")),
        "listen" => listen(&rest),
        "setup" => setup(),
        "doctor" => doctor(),
        "eval" => eval(&rest),
        "shortcuts" => list_shortcuts(),
        "recordings" => recordings(&rest),
        "version" | "--version" => {
            println!("alfred {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => {
            print!("{HELP}");
            Ok(())
        }
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("alfred: {e}");
            1
        }
    }
}

pub fn load_config() -> Result<Config, String> {
    Config::load(&config::config_path())
}

fn ask(args: &[String]) -> Result<(), String> {
    let flag = |f: &str| args.iter().any(|a| a == f);
    let text = args.iter().filter(|a| !a.starts_with("--")).cloned().collect::<Vec<_>>().join(" ");
    if text.trim().is_empty() {
        return Err("say what to do, for example: alfred ask \"open Notes\"".into());
    }
    let cfg = load_config()?;
    // A dry run is never recorded: it does nothing, so there is no outcome to learn from.
    let (mut agent, mac) = crate::wiring::build_for(&cfg, !flag("--dry-run"));
    // A dry run acts on nothing, but sees where the user is, so it decides as the real run would.
    let dry = DryRun::new().with_context(mac.context());
    let exec: &dyn Executor = if flag("--dry-run") { &dry } else { &mac };
    let mut show = |e: Event| match e {
        Event::Thinking => eprintln!("… thinking with {}", agent_label(&cfg)),
        Event::Reasoning => eprintln!("… thinking it through"),
        Event::Acting(a) => eprintln!("→ {}", a.progress_label()),
        Event::Acted(d) => match &d.result {
            Ok(_) => eprintln!("✓ {}", d.action.tool_name()),
            Err(e) => eprintln!("✗ {}: {e}", d.action.tool_name()),
        },
    };
    let mut turn = agent.handle(&text, exec, &mut show);
    if turn.pending.is_some() {
        println!("{}", turn.reply);
        let yes = flag("--yes") || (std::io::stdin().is_terminal() && confirm_on_terminal());
        turn = agent.answer_confirmation(yes, exec, &mut show);
    }
    println!("{}", turn.reply);
    if flag("--dry-run") {
        for a in dry.actions() {
            println!("  (dry run) {}", serde_json::to_string(&a).unwrap_or_default());
        }
    }
    if flag("--speak") {
        say(&turn.reply)?;
    }
    Ok(())
}

/// A conversation on the terminal: each line is a request to the same agent, so follow-ups ("what do you
/// see there?") know what came before. Confirmations are asked on the terminal, or refused when piped.
fn chat(args: &[String]) -> Result<(), String> {
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let cfg = load_config()?;
    let (mut agent, mac) = crate::wiring::build_for(&cfg, !dry_run);
    // A dry run acts on nothing, but sees where the user is, so it decides as the real run would.
    let dry = DryRun::new().with_context(mac.context());
    let exec: &dyn Executor = if dry_run { &dry } else { &mac };
    let mut show = |e: Event| match e {
        Event::Thinking => eprintln!("… thinking"),
        Event::Reasoning => eprintln!("… thinking it through"),
        Event::Acting(a) => eprintln!("→ {}", a.progress_label()),
        Event::Acted(d) => match &d.result {
            Ok(_) => eprintln!("✓ {}", d.action.tool_name()),
            Err(e) => eprintln!("✗ {}: {e}", d.action.tool_name()),
        },
    };
    let interactive = std::io::stdin().is_terminal();
    for line in std::io::stdin().lock().lines() {
        let text = line.map_err(|e| e.to_string())?;
        if text.trim().is_empty() {
            continue;
        }
        if !interactive {
            println!("you: {}", text.trim());
        }
        let mut turn = agent.handle(text.trim(), exec, &mut show);
        if turn.pending.is_some() {
            println!("alfred: {}", turn.reply);
            let yes = interactive && confirm_on_terminal();
            turn = agent.answer_confirmation(yes, exec, &mut show);
        }
        println!("alfred: {}", turn.reply);
    }
    Ok(())
}

/// Chrome is running with remote debugging on: it writes the port it listens on into its profile folder.
fn chrome_lets_alfred_in() -> bool {
    std::env::var_os("HOME")
        .map(|h| std::path::Path::new(&h).join("Library/Application Support/Google/Chrome/DevToolsActivePort").is_file())
        .unwrap_or(false)
}

fn agent_label(cfg: &Config) -> String {
    let server = if cfg.llm.provider == Provider::Ollama { "Ollama" } else { "OpenAI-compatible" };
    match cfg.llm.think_model.as_deref().filter(|m| !m.trim().is_empty() && *m != cfg.llm.model) {
        Some(thinker) => format!("{} ({server}; hard questions: {thinker})", cfg.llm.model),
        None => format!("{} ({server})", cfg.llm.model),
    }
}

fn confirm_on_terminal() -> bool {
    eprint!("[y/N] ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).ok();
    alfred_core::agent::yes_or_no(&line).unwrap_or(false)
}

fn say(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("give me something to say".into());
    }
    let cfg = load_config()?;
    let speaker = match Speaker::load(&config::models_dir(), &cfg.voice.voice, cfg.voice.speed) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("(using the system voice: {e})");
            None
        }
    };
    let playback = Playback::new();
    speech::speak(speaker.as_ref(), text, &playback, |_, _| {})
}

fn listen(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("give a WAV file to transcribe: alfred listen note.wav")?;
    let listener = Listener::load(&config::models_dir())?;
    println!("{}", listener.transcribe_file(Path::new(path))?);
    Ok(())
}

fn setup() -> Result<(), String> {
    let path = config::config_path();
    let cfg = load_config()?;
    if !path.exists() {
        cfg.save(&path)?;
        println!("✓ wrote {}", path.display());
    } else {
        println!("✓ config {}", path.display());
    }
    let dir = config::models_dir();
    for m in models::ALL {
        if m.is_ready(&dir) {
            println!("✓ {} already downloaded", m.label);
            continue;
        }
        println!("↓ {} (about {} MB)", m.label, m.size_mb);
        let mut last = 0;
        m.ensure(&dir, &mut |done, total| {
            let pct = total.map(|t| done * 100 / t.max(1)).unwrap_or(0);
            if pct >= last + 10 {
                last = pct;
                eprint!("  {pct}%");
                std::io::stderr().flush().ok();
            }
        })?;
        eprintln!();
        println!("✓ {}", m.label);
    }
    if cfg.llm.provider == Provider::Ollama {
        let ollama = Ollama::new(&cfg.llm.base_url, &cfg.llm.model, 10);
        match ollama.installed() {
            Ok(list) if list.iter().any(|m| same_model(m, &cfg.llm.model)) => println!("✓ {} is installed in Ollama", cfg.llm.model),
            Ok(_) => {
                println!("↓ pulling {} with Ollama", cfg.llm.model);
                let ok = std::process::Command::new("ollama").args(["pull", &cfg.llm.model]).status().map(|s| s.success()).unwrap_or(false);
                if !ok {
                    return Err(format!("`ollama pull {}` failed", cfg.llm.model));
                }
            }
            Err(_) => println!(
                "! Ollama isn't running at {}. Install it (brew install ollama), run `ollama serve`, then `ollama pull {}`.\n  Alfred still follows simple commands without it.",
                cfg.llm.base_url, cfg.llm.model
            ),
        }
    }
    println!("\nReady. Run `alfred` and press {} to summon it.", cfg.ui.shortcut);
    Ok(())
}

/// Ollama lists "qwen3:0.6b"; a config may say "qwen3" (":latest").
fn same_model(installed: &str, wanted: &str) -> bool {
    installed == wanted || installed == format!("{wanted}:latest") || installed.strip_suffix(":latest") == Some(wanted)
}

fn doctor() -> Result<(), String> {
    let mut ok = true;
    let mut line = |good: bool, what: String| {
        ok &= good;
        println!("{} {what}", if good { "✓" } else { "✗" });
    };
    let path = config::config_path();
    let cfg = match load_config() {
        Ok(c) => {
            line(true, format!("config {}{}", path.display(), if path.exists() { "" } else { " (defaults; `alfred setup` writes it)" }));
            c
        }
        Err(e) => {
            line(false, format!("config: {e}"));
            Config::default()
        }
    };
    let dir = config::models_dir();
    for m in models::ALL {
        line(m.is_ready(&dir), format!("{} in {}", m.label, m.dir(&dir).display()));
    }
    line(alfred_voice::voice_id(&cfg.voice.voice).is_some(), format!("voice {}", cfg.voice.voice));
    match cfg.llm.provider {
        Provider::Ollama => match Ollama::new(&cfg.llm.base_url, &cfg.llm.model, 5).installed() {
            Ok(list) => {
                line(true, format!("Ollama at {}", cfg.llm.base_url));
                line(
                    list.iter().any(|m| same_model(m, &cfg.llm.model)),
                    format!("model {} (ollama pull {})", cfg.llm.model, cfg.llm.model),
                );
                if let Some(thinker) = cfg.llm.think_model.as_deref().filter(|m| !m.trim().is_empty() && *m != cfg.llm.model) {
                    line(list.iter().any(|m| same_model(m, thinker)), format!("thinking model {thinker} (ollama pull {thinker})"));
                }
            }
            Err(e) => line(false, format!("Ollama: {e}")),
        },
        Provider::OpenAi => println!("· OpenAI-compatible server at {} (not checked)", cfg.llm.base_url),
    }
    let shortcut_count =
        alfred_mac::find_program("shortcuts").map(|_| alfred_mac::shortcuts::list(Duration::from_secs(15)).map(|l| l.len()));
    match shortcut_count {
        Some(Ok(n)) => println!("· {n} shortcuts available (`alfred shortcuts` lists them)"),
        Some(Err(e)) => line(false, format!("shortcuts: {e}")),
        None => println!("· the shortcuts command isn't available on this Mac"),
    }
    match alfred_mac::find_program(&cfg.desktop.command) {
        Some(program) => {
            let status = alfred_hands::desktop::Desktop::call(
                &alfred_hands::desktop::AgentDesktop::new(program.clone()),
                &["status".to_string()],
                None,
            );
            let access = status
                .as_ref()
                .ok()
                .and_then(|s| s["permissions"]["accessibility"]["state"].as_str().map(str::to_string))
                .unwrap_or_else(|| "unknown".into());
            line(true, format!("agent-desktop at {}", program.display()));
            line(
                access == "granted",
                format!(
                    "Accessibility for on-screen actions: {access} (System Settings → Privacy & Security → Accessibility: allow Alfred, or the terminal running it)"
                ),
            );
            if cfg.llm.provider == Provider::Ollama
                && let Ok(list) = Ollama::new(&cfg.llm.base_url, &cfg.desktop.model, 5).installed()
            {
                line(
                    list.iter().any(|m| same_model(m, &cfg.desktop.model)),
                    format!("on-screen model {} (ollama pull {})", cfg.desktop.model, cfg.desktop.model),
                );
            }
        }
        None => println!("· on-screen actions are off: `brew install agent-desktop` turns them on"),
    }
    match alfred_mac::find_program(&cfg.browser.command) {
        Some(program) if cfg.browser.enabled && cfg.browser.use_chrome => {
            line(true, format!("agent-browser at {} (website tasks and spoken search answers, in your Chrome)", program.display()));
            line(
                chrome_lets_alfred_in(),
                "Chrome allows remote debugging, so Alfred can work in a tab of its own (turn it on at chrome://inspect/#remote-debugging, with Chrome open)".to_string(),
            );
        }
        Some(program) if cfg.browser.enabled => line(
            true,
            format!(
                "agent-browser at {} (website tasks and spoken search answers; profile {})",
                program.display(),
                cfg.browser.profile_dir().display()
            ),
        ),
        Some(_) => println!("· website tasks are off in the config ([browser] enabled = false)"),
        None => println!("· website tasks are off and searches only open the results: `brew install agent-browser` turns them on"),
    }
    let mic = rune::Recorder::permission();
    println!("· microphone permission: {mic:?} (asked the first time you talk to Alfred)");
    let kept = alfred_core::record::summary(&config::recordings_dir());
    println!(
        "· recording is {} ({} kept; `alfred recordings` says more)",
        if cfg.recording.enabled { "on" } else { "off" },
        megabytes(kept.bytes)
    );
    println!("· Notes and Reminders ask for Automation permission the first time Alfred uses them");
    if ok { Ok(()) } else { Err("some pieces are missing: `alfred setup` fixes most of them".into()) }
}

/// What the recordings folder holds, in words; `clear` deletes it.
fn recordings(args: &[String]) -> Result<(), String> {
    let cfg = load_config()?;
    let dir = config::recordings_dir();
    let found = alfred_core::record::summary(&dir);
    if args.first().map(String::as_str) == Some("clear") {
        if found.files == 0 {
            println!("There are no recordings to delete.");
            return Ok(());
        }
        let sure = args.iter().any(|a| a == "--yes") || {
            eprint!("Delete {} of recordings in {}? ", megabytes(found.bytes), dir.display());
            std::io::stdin().is_terminal() && confirm_on_terminal()
        };
        if !sure {
            println!("Nothing was deleted.");
            return Ok(());
        }
        let n = alfred_core::record::clear(&dir).map_err(|e| format!("couldn't delete the recordings: {e}"))?;
        println!("Deleted {n} file(s) of recordings.");
        return Ok(());
    }
    println!("{}", recording_status(&cfg, &found, &dir));
    Ok(())
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1e6)
}

/// "Recording is on …" or "off …", with what has been kept so far.
fn recording_status(cfg: &Config, found: &alfred_core::record::Summary, dir: &Path) -> String {
    let mut lines = vec![if cfg.recording.enabled {
        format!(
            "Recording is on: Alfred keeps what it is asked, what its models see and answer, and what each action does, in {} (this Mac only).",
            dir.display()
        )
    } else {
        "Recording is off. Turn it on with `enabled = true` under `[recording]` in the config: Alfred then keeps what it is asked, what its models see and answer (including what is on screen in the apps and pages it works in), and what each action does, on this Mac only.".to_string()
    }];
    if found.files == 0 {
        lines.push("Nothing has been recorded yet.".into());
    } else {
        let n = |k: &str| found.kinds.get(k).copied().unwrap_or(0);
        let days = match (&found.first_day, &found.last_day) {
            (Some(a), Some(b)) if a == b => format!("on {a}"),
            (Some(a), Some(b)) => format!("from {a} to {b}"),
            _ => String::new(),
        };
        lines.push(format!(
            "Kept {days}: {} requests, {} model calls, {} actions ({} worked, {} failed), {} yes/no answers; {} of {} MB.",
            n("request"),
            n("model"),
            n("action"),
            found.worked,
            found.failed,
            n("confirmation"),
            megabytes(found.bytes),
            cfg.recording.max_mb
        ));
        if found.bytes >= cfg.recording.max_mb * 1_000_000 {
            lines.push("The folder is full, so nothing new is recorded: raise `max_mb` or run `alfred recordings clear`.".into());
        }
        lines.push("`alfred recordings clear` deletes them.".into());
    }
    lines.join("\n")
}

fn list_shortcuts() -> Result<(), String> {
    let cfg = load_config()?;
    let names = alfred_mac::shortcuts::list(Duration::from_secs(15))?;
    if names.is_empty() {
        println!("No shortcuts yet: make some in the Shortcuts app, then say \"run <name>\".");
        return Ok(());
    }
    let trusted: Vec<String> = cfg.shortcuts.trusted.iter().map(|t| alfred_core::catalog::fold(t)).collect();
    for n in &names {
        let mark = if trusted.contains(&alfred_core::catalog::fold(n)) { "  (runs without asking)" } else { "" };
        println!("{n}{mark}");
    }
    println!(
        "\nSay \"run <name>\" to run one. When Alfred picks one from looser words it asks first, unless the name is in [shortcuts] trusted."
    );
    Ok(())
}

fn eval(args: &[String]) -> Result<(), String> {
    let with_model = args.iter().any(|a| a == "--model");
    let (cases, catalog) = match args.iter().find(|a| !a.starts_with("--")) {
        Some(file) => alfred_core::eval::load_with_catalog(&PathBuf::from(file))?,
        None => alfred_core::eval::parse_with_catalog(FIXTURE)?,
    };
    let cfg = load_config()?;
    if with_model {
        println!("Scoring the router and {} on {} cases (dry run)…\n", agent_label(&cfg), cases.len());
    } else {
        println!("Scoring the router alone on {} cases (add --model to include the language model)…\n", cases.len());
    }
    let report = alfred_core::eval::run(&cases, || {
        let model = with_model.then(|| llm::from_config(&cfg.llm));
        let agent = Agent::new(model).with_tool_mode(cfg.llm.tool_mode).with_thinking(cfg.llm.thinking).with_catalog(catalog.clone());
        match with_model.then(|| llm::thinker_from_config(&cfg.llm)).flatten() {
            Some(thinker) => agent.with_thinker(thinker),
            None => agent,
        }
    });
    print!("{}", report.render());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names_match_with_or_without_latest() {
        assert!(same_model("qwen3:0.6b", "qwen3:0.6b"));
        assert!(same_model("llama3:latest", "llama3"));
        assert!(!same_model("qwen3:1.7b", "qwen3:0.6b"));
    }

    #[test]
    fn recording_status_says_what_is_kept_and_how_to_delete_it() {
        let dir = std::env::temp_dir().join(format!("alfred-cli-recordings-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let mut cfg = Config::default();
        let off = recording_status(&cfg, &alfred_core::record::summary(&dir), &dir);
        assert!(
            off.starts_with("Recording is off.") && off.contains("on this Mac only") && off.contains("Nothing has been recorded"),
            "{off}"
        );
        cfg.recording.enabled = true;
        let rec = alfred_core::record::Recorder::to_dir(&dir, 1);
        rec.next_turn();
        rec.record("request", serde_json::json!({"text": "open notes"}));
        rec.record("action", serde_json::json!({"ok": true}));
        let on = recording_status(&cfg, &alfred_core::record::summary(&dir), &dir);
        assert!(on.starts_with("Recording is on") && on.contains("1 requests") && on.contains("(1 worked, 0 failed)"), "{on}");
        assert!(on.contains("alfred recordings clear"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_embedded_fixture_parses() {
        assert!(alfred_core::eval::parse(FIXTURE).unwrap().len() >= 30);
    }
}
