//! Building the brain and the hands from the config, the same way for the app and the command line.

use std::sync::Arc;
use std::time::Duration;

use alfred_core::llm::ChatModel;
use alfred_core::record::{Recorder, recorded};
use alfred_core::{Agent, Catalog, Config, config, llm};
use alfred_hands::browser::AgentBrowser;
use alfred_hands::desktop::AgentDesktop;
use alfred_hands::{BrowserHands, DesktopHands};
use alfred_mac::{MacExecutor, find_program, shortcuts};

/// The agent and the executor, with whatever optional capabilities this Mac has. When the user turned
/// recording on, every model and the agent write to the recordings folder.
pub fn build(cfg: &Config) -> (Agent, MacExecutor) {
    build_for(cfg, true)
}

/// [`build`], or without recording when `real` is false: a dry run does nothing, so its "it worked" would
/// teach nothing true.
pub fn build_for(cfg: &Config, real: bool) -> (Agent, MacExecutor) {
    let recorder = (real && cfg.recording.enabled).then(|| Recorder::to_dir(&config::recordings_dir(), cfg.recording.max_mb));
    let model = |cfg: &alfred_core::config::LlmConfig, role: &'static str| -> Box<dyn ChatModel> {
        match &recorder {
            Some(r) => recorded(llm::from_config(cfg), role, r),
            None => llm::from_config(cfg),
        }
    };
    let mut catalog = Catalog::default();
    if cfg.shortcuts.enabled && find_program("shortcuts").is_some() {
        catalog.shortcuts = shortcuts::list(Duration::from_secs(15)).unwrap_or_default();
    }
    let mut exec = MacExecutor::from_config(cfg);
    // An app's window or a web page fills a few thousand tokens: give the model that drives the steps room
    // for them. The browser uses the same model, since its decisions need structured output too.
    let mut hands_llm = cfg.llm.with_model(&cfg.desktop.model);
    hands_llm.context_tokens = hands_llm.context_tokens.max(16_384);
    hands_llm.max_reply_tokens = hands_llm.max_reply_tokens.max(512);
    if cfg.desktop.enabled
        && let Some(program) = find_program(&cfg.desktop.command)
    {
        let hands = DesktopHands::new(Box::new(AgentDesktop::new(program)), model(&hands_llm, "app"), cfg.desktop.max_steps as usize);
        exec = exec.with_hands(Arc::new(hands));
        catalog.hands = true;
    }
    if cfg.browser.enabled
        && let Some(program) = find_program(&cfg.browser.command)
    {
        let backend = if cfg.browser.use_chrome {
            AgentBrowser::chrome(program)
        } else {
            AgentBrowser::new(program, cfg.browser.profile_dir(), cfg.browser.headed)
        };
        let web = BrowserHands::new(Box::new(backend), model(&hands_llm, "web"), cfg.browser.max_steps as usize)
            .with_search_engine(&cfg.search.engine);
        exec = exec.with_web_hands(Arc::new(web));
        catalog.browser = true;
    }
    let mut agent = Agent::new(Some(model(&cfg.llm, "decide")))
        .with_history_turns(cfg.llm.history_turns)
        .with_tool_mode(cfg.llm.tool_mode)
        .with_thinking(cfg.llm.thinking)
        .with_catalog(catalog)
        .with_trusted_shortcuts(&cfg.shortcuts.trusted);
    if let Some(thinker) = llm::thinker_from_config(&cfg.llm) {
        agent = agent.with_thinker(match &recorder {
            Some(r) => recorded(thinker, "think", r),
            None => thinker,
        });
    }
    if let Some(r) = recorder {
        agent = agent.with_recorder(r);
    }
    (agent, exec)
}
