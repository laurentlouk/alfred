//! The browser against the real web: agent-browser, headless, a throwaway session and profile, and the
//! model from Ollama. Off by default; run it with
//!
//! ```text
//! ALFRED_LIVE_BROWSER=1 ALFRED_LIVE_PROFILE=/tmp/alfred-live cargo test -p alfred-hands --test live_browser -- --ignored --nocapture
//! ```
//!
//! `ALFRED_LIVE_MODEL` picks the model (default `qwen3:8b`), `ALFRED_LIVE_TASK` the task done on
//! Wikipedia (empty for none), `ALFRED_LIVE_QUESTIONS` the questions answered from a search (separated by
//! `|`), `ALFRED_LIVE_ENGINES` the engines they start on (default `google`; empty for none), and
//! `ALFRED_LIVE_HEADED=1` shows the window.

use std::path::{Path, PathBuf};
use std::time::Instant;

use alfred_core::action::WebHands;
use alfred_core::config::LlmConfig;
use alfred_core::llm;
use alfred_hands::BrowserHands;
use alfred_hands::browser::{AgentBrowser, Browser, args};

fn program() -> Option<PathBuf> {
    ["/opt/homebrew/bin/agent-browser", "/usr/local/bin/agent-browser"].iter().map(PathBuf::from).find(|p| p.is_file())
}

fn backend(session: &str, profile: &Path) -> AgentBrowser {
    let headed = std::env::var_os("ALFRED_LIVE_HEADED").is_some();
    AgentBrowser { session: session.into(), ..AgentBrowser::new(program().unwrap(), profile.to_path_buf(), headed) }
}

fn hands(session: &str, profile: &Path, engine: &str) -> BrowserHands {
    let mut cfg = LlmConfig::default().with_model(&std::env::var("ALFRED_LIVE_MODEL").unwrap_or_else(|_| "qwen3:8b".into()));
    cfg.context_tokens = 16_384;
    cfg.max_reply_tokens = 512;
    cfg.timeout_secs = 120;
    BrowserHands::new(Box::new(backend(session, profile)), llm::from_config(&cfg), 10).with_search_engine(engine)
}

#[test]
#[ignore = "drives a real browser on the web and a local model"]
fn live_wikipedia_search_and_a_spoken_weather_answer() {
    if std::env::var_os("ALFRED_LIVE_BROWSER").is_none() || program().is_none() {
        return;
    }
    let profile = PathBuf::from(std::env::var("ALFRED_LIVE_PROFILE").unwrap_or_else(|_| "/tmp/alfred-live-browser".into()));
    let session = format!("alfred-live-{}", std::process::id());

    let task = std::env::var("ALFRED_LIVE_TASK")
        .unwrap_or_else(|_| "search Wikipedia for Rune programming language and tell me the title of the first result".into());
    let mut outcome = Ok(alfred_core::Outcome::new("no task"));
    if !task.is_empty() {
        let started = Instant::now();
        outcome = hands(&session, &profile, "google").browse(&task, Some("https://en.wikipedia.org/wiki/Main_Page"));
        println!("TASK: {task}\n  -> {outcome:?}\n  ({:.1} s)", started.elapsed().as_secs_f32());
    }

    let questions = std::env::var("ALFRED_LIVE_QUESTIONS")
        .unwrap_or_else(|_| "what is the weather in Paris|who won the last Champions League final".into());
    let engines = std::env::var("ALFRED_LIVE_ENGINES").unwrap_or_else(|_| "google".into());
    for engine in engines.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        for question in questions.split('|').map(str::trim).filter(|q| !q.is_empty()) {
            let started = Instant::now();
            let answer = hands(&session, &profile, engine).answer(question);
            let at = backend(&session, &profile).call(&args(["get", "url"])).map(|d| d["url"].to_string()).unwrap_or_default();
            println!("ANSWER from {engine}: {question}\n  -> {answer:?}\n  window on {at} ({:.1} s)", started.elapsed().as_secs_f32());
        }
    }

    backend(&session, &profile).call(&args(["close"])).ok();
    assert!(outcome.is_ok(), "{outcome:?}");
}
