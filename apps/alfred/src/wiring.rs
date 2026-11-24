//! Building the brain and the hands from the config, the same way for the app and the command line.

use std::sync::Arc;
use std::time::Duration;

use alfred_core::{Agent, Catalog, Config, llm};
use alfred_hands::browser::AgentBrowser;
use alfred_hands::desktop::AgentDesktop;
use alfred_hands::{BrowserHands, DesktopHands};
use alfred_mac::{MacExecutor, find_program, shortcuts};

/// The agent and the executor, with whatever optional capabilities this Mac has.
pub fn build(cfg: &Config) -> (Agent, MacExecutor) {
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
        let hands = DesktopHands::new(Box::new(AgentDesktop::new(program)), llm::from_config(&hands_llm), cfg.desktop.max_steps as usize);
        exec = exec.with_hands(Arc::new(hands));
        catalog.hands = true;
    }
    if cfg.browser.enabled
        && let Some(program) = find_program(&cfg.browser.command)
    {
        let backend = AgentBrowser::new(program, cfg.browser.profile_dir(), cfg.browser.headed);
        let web = BrowserHands::new(Box::new(backend), llm::from_config(&hands_llm), cfg.browser.max_steps as usize)
            .with_search_engine(&cfg.search.engine);
        exec = exec.with_web_hands(Arc::new(web));
        catalog.browser = true;
    }
    let mut agent = Agent::new(Some(llm::from_config(&cfg.llm)))
        .with_history_turns(cfg.llm.history_turns)
        .with_tool_mode(cfg.llm.tool_mode)
        .with_thinking(cfg.llm.thinking)
        .with_catalog(catalog)
        .with_trusted_shortcuts(&cfg.shortcuts.trusted);
    if let Some(thinker) = llm::thinker_from_config(&cfg.llm) {
        agent = agent.with_thinker(thinker);
    }
    (agent, exec)
}
