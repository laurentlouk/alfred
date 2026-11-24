//! Alfred's brain, free of UI and native code so every part of it is testable on its own.
//!
//! A request flows: [`router::route`] (deterministic) → [`agent::Agent`] (the model with [`tools`]) →
//! an [`action::Action`] → an [`action::Executor`] (`alfred-mac` on a Mac, [`action::DryRun`] in tests).

pub mod action;
pub mod agent;
pub mod catalog;
pub mod config;
pub mod context;
pub mod eval;
pub mod llm;
pub mod normalize;
pub mod record;
pub mod router;
pub mod tools;

pub use action::{Action, DryRun, Executor, Hands, Outcome, Risk};
pub use agent::{Agent, Done, Event, Source, Turn};
pub use catalog::Catalog;
pub use config::Config;
