//! The UI's two helpers, each on its own thread so the window never waits:
//!
//! - the brain: the agent (router, model, tools) and the Mac executor;
//! - the voice: Kokoro, loaded once, and playback;
//! - the ears: the wake word, voice detection and Moonshine, on a thread of their own (`alfred-voice`).
//!
//! Results land on the UI thread through `AppHandle::update`, tagged with the request's attempt number
//! so an answer to a superseded request changes nothing.

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, OnceLock};

use alfred_core::Event;
use alfred_core::config::{self, Config};
use alfred_voice::{Ears, EarsConfig, Speaker};
use rune::AppHandle;

use crate::speech::{self, Playback};
use crate::ui::Ui;

pub enum BrainJob {
    Ask { attempt: u64, text: String },
    Confirm { attempt: u64, yes: bool },
}

pub enum VoiceJob {
    Speak { attempt: u64, text: String },
}

/// What the UI holds to reach its helpers.
pub struct Links {
    pub me: AppHandle<Ui>,
    pub brain: Sender<BrainJob>,
    pub voice: Sender<VoiceJob>,
    pub playback: Arc<Playback>,
    pub config: Config,
    /// The listening: the wake word, voice detection and transcription. Set once its models have loaded.
    pub ears: Arc<OnceLock<Ears>>,
}

pub fn spawn(config: Config, me: AppHandle<Ui>) -> Links {
    let (brain_tx, brain_rx) = mpsc::channel::<BrainJob>();
    let (voice_tx, voice_rx) = mpsc::channel::<VoiceJob>();
    let playback = Playback::new();

    // Like any Mac app that works in other apps: when on-screen control is set up but Accessibility isn't
    // allowed, macOS shows its own request at launch ("Alfred would like to control this computer…") and
    // System Settings opens at the list. agent-desktop's own view decides (it is what would fail);
    // macOS's answer to Alfred only when agent-desktop can't tell.
    if config.desktop.enabled
        && let Some(program) = alfred_mac::find_program(&config.desktop.command)
    {
        std::thread::Builder::new()
            .name("alfred-accessibility".into())
            .spawn(move || {
                use alfred_hands::desktop::Desktop;
                let engine = alfred_hands::desktop::AgentDesktop::new(program);
                let seen = engine
                    .call(&["status".to_string()], None)
                    .ok()
                    .and_then(|s| s["permissions"]["accessibility"]["state"].as_str().map(|state| state == "granted"));
                if seen.or_else(alfred_mac::permissions::accessibility_allowed) == Some(false) {
                    alfred_mac::permissions::ask_for_accessibility(false);
                }
            })
            .ok();
    }

    let cfg = config.clone();
    let ui = me.clone();
    std::thread::Builder::new()
        .name("alfred-brain".into())
        .spawn(move || {
            let (mut agent, exec) = crate::wiring::build(&cfg);
            let label = agent.model_name().unwrap_or_default();
            ui.update(move |s, _| s.model_label = label);
            for job in brain_rx {
                let (attempt, turn) = match job {
                    BrainJob::Ask { attempt, text } => {
                        let turn = agent.handle(&text, &exec, &mut progress(&ui, attempt));
                        (attempt, turn)
                    }
                    BrainJob::Confirm { attempt, yes } => (attempt, agent.answer_confirmation(yes, &exec, &mut progress(&ui, attempt))),
                };
                ui.update(move |s, _| s.on_turn(attempt, turn));
            }
        })
        .expect("spawn the brain thread");

    let cfg = config.clone();
    let ui = me.clone();
    let pb = playback.clone();
    std::thread::Builder::new()
        .name("alfred-voice".into())
        .spawn(move || {
            let models = config::models_dir();
            let speaker = Speaker::load(&models, &cfg.voice.voice, cfg.voice.speed).map_err(|e| eprintln!("alfred: {e}")).ok();
            let voice = speaker.is_some();
            ui.update(move |s, _| s.on_voice_ready(voice));
            for job in voice_rx {
                match job {
                    VoiceJob::Speak { attempt, text } => {
                        let me = ui.clone();
                        let result = speech::speak(speaker.as_ref(), &text, &pb, move |envelope, seconds| {
                            me.update(move |s, tx| s.on_sentence(attempt, envelope, seconds, tx));
                        });
                        if let Err(e) = result {
                            eprintln!("alfred: speech failed: {e}");
                        }
                        ui.update(move |s, _| s.on_spoken(attempt));
                    }
                }
            }
        })
        .expect("spawn the voice thread");

    // The listening loads its models on a thread of its own, so the window doesn't wait for them.
    let ears = Arc::new(OnceLock::new());
    let (slot, cfg, ui) = (ears.clone(), config.clone(), me.clone());
    std::thread::Builder::new()
        .name("alfred-ears-start".into())
        .spawn(move || {
            let settings = EarsConfig {
                wake_word: cfg.voice.wake_word,
                silence_slow_secs: cfg.voice.silence_secs,
                max_listen_secs: cfg.voice.max_listen_secs,
                ..EarsConfig::default()
            };
            let heard = ui.clone();
            let ready = match Ears::start(&config::models_dir(), settings, move |h| heard.update(move |s, _| s.on_ears(h))) {
                Ok(e) => slot.set(e).is_ok(),
                Err(e) => {
                    eprintln!("alfred: listening unavailable: {e}");
                    false
                }
            };
            ui.update(move |s, _| s.on_ears_ready(ready));
        })
        .expect("spawn the listening");

    Links { me, brain: brain_tx, voice: voice_tx, playback, config, ears }
}

/// Forward the agent's progress to the UI's chips.
fn progress(ui: &AppHandle<Ui>, attempt: u64) -> impl FnMut(Event) + '_ {
    move |e| match e {
        Event::Thinking => ui.update(move |s, _| s.on_thinking(attempt)),
        Event::Reasoning => ui.update(move |s, _| s.on_reasoning(attempt)),
        Event::Acting(a) => {
            let label = a.progress_label();
            ui.update(move |s, _| s.on_acting(attempt, label));
        }
        Event::Acted(d) => {
            let ok = d.result.is_ok();
            ui.update(move |s, _| s.on_acted(attempt, ok));
        }
    }
}
