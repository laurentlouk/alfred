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
  alfred say TEXT             speak TEXT with Alfred's voice
  alfred listen FILE.wav      transcribe a recording
  alfred setup                write the config, download the voice models, pull the language model
  alfred doctor               check what is ready and what is missing
  alfred shortcuts            list the Shortcuts Alfred can run
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
        "say" => say(&rest.join(" ")),
        "listen" => listen(&rest),
        "setup" => setup(),
        "doctor" => doctor(),
        "eval" => eval(&rest),
        "shortcuts" => list_shortcuts(),
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
    let (mut agent, mac) = crate::wiring::build(&cfg);
    let dry = DryRun::new();
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

fn agent_label(cfg: &Config) -> String {
    format!("{} ({})", cfg.llm.model, if cfg.llm.provider == Provider::Ollama { "Ollama" } else { "OpenAI-compatible" })
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
    speech::speak(speaker.as_ref(), text, &config::cache_dir(), &playback, |_, _| {})
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
    println!("· Notes and Reminders ask for Automation permission the first time Alfred uses them");
    if ok { Ok(()) } else { Err("some pieces are missing: `alfred setup` fixes most of them".into()) }
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
        Agent::new(model).with_tool_mode(cfg.llm.tool_mode).with_thinking(cfg.llm.thinking).with_catalog(catalog.clone())
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
    fn the_embedded_fixture_parses() {
        assert!(alfred_core::eval::parse(FIXTURE).unwrap().len() >= 30);
    }
}
