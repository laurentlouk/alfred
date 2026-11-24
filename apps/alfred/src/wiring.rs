//! Building the brain and the hands from the config, the same way for the app and the command line.

use std::sync::Arc;
use std::time::Duration;

use alfred_core::{Agent, Catalog, Config, llm};
use alfred_hands::DesktopHands;
use alfred_hands::desktop::AgentDesktop;
use alfred_mac::{MacExecutor, find_program, shortcuts};

/// The agent and the executor, with whatever optional capabilities this Mac has.
pub fn build(cfg: &Config) -> (Agent, MacExecutor) {
    let mut catalog = Catalog::default();
    if cfg.shortcuts.enabled && find_program("shortcuts").is_some() {
        catalog.shortcuts = shortcuts::list(Duration::from_secs(15)).unwrap_or_default();
    }
    let mut exec = MacExecutor::from_config(cfg);
    if cfg.desktop.enabled
        && let Some(program) = find_program(&cfg.desktop.command)
    {
        let model = llm::from_config(&cfg.llm.with_model(&cfg.desktop.model));
        let hands = DesktopHands::new(Box::new(AgentDesktop::new(program)), model, cfg.desktop.max_steps as usize);
        exec = exec.with_hands(Arc::new(hands));
        catalog.hands = true;
    }
    let agent = Agent::new(Some(llm::from_config(&cfg.llm)))
        .with_history_turns(cfg.llm.history_turns)
        .with_tool_mode(cfg.llm.tool_mode)
        .with_catalog(catalog)
        .with_trusted_shortcuts(&cfg.shortcuts.trusted);
    (agent, exec)
}
