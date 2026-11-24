//! The UI's two helpers, each on its own thread so the window never waits:
//!
//! - the brain: the agent (router, model, tools) and the Mac executor;
//! - the voice: Kokoro and Moonshine, loaded once, plus playback.
//!
//! Results land on the UI thread through `AppHandle::update`, tagged with the request's attempt number
//! so an answer to a superseded request changes nothing.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};

use alfred_core::Event;
use alfred_core::config::{self, Config};
use alfred_voice::{Listener, Speaker};
use rune::AppHandle;

use crate::speech::{self, Playback};
use crate::ui::Ui;

pub enum BrainJob {
    Ask { attempt: u64, text: String },
    Confirm { attempt: u64, yes: bool },
}

pub enum VoiceJob {
    Speak { attempt: u64, text: String },
    Transcribe { attempt: u64, path: PathBuf },
}

/// What the UI holds to reach its helpers.
pub struct Links {
    pub me: AppHandle<Ui>,
    pub brain: Sender<BrainJob>,
    pub voice: Sender<VoiceJob>,
    pub playback: Arc<Playback>,
    pub config: Config,
}

pub fn spawn(config: Config, me: AppHandle<Ui>) -> Links {
    let (brain_tx, brain_rx) = mpsc::channel::<BrainJob>();
    let (voice_tx, voice_rx) = mpsc::channel::<VoiceJob>();
    let playback = Playback::new();

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
            let listener = Listener::load(&models).map_err(|e| eprintln!("alfred: {e}")).ok();
            let (voice, ears) = (speaker.is_some(), listener.is_some());
            ui.update(move |s, _| s.on_voice_ready(voice, ears));
            for job in voice_rx {
                match job {
                    VoiceJob::Speak { attempt, text } => {
                        let me = ui.clone();
                        let result = speech::speak(speaker.as_ref(), &text, &config::cache_dir(), &pb, move |envelope, seconds| {
                            me.update(move |s, tx| s.on_sentence(attempt, envelope, seconds, tx));
                        });
                        if let Err(e) = result {
                            eprintln!("alfred: speech failed: {e}");
                        }
                        ui.update(move |s, _| s.on_spoken(attempt));
                    }
                    VoiceJob::Transcribe { attempt, path } => {
                        let text = match &listener {
                            Some(l) => l.transcribe_file(&path).unwrap_or_default(),
                            None => String::new(),
                        };
                        std::fs::remove_file(&path).ok();
                        ui.update(move |s, _| s.on_heard(attempt, text));
                    }
                }
            }
        })
        .expect("spawn the voice thread");

    Links { me, brain: brain_tx, voice: voice_tx, playback, config }
}

/// Forward the agent's progress to the UI's chips.
fn progress(ui: &AppHandle<Ui>, attempt: u64) -> impl FnMut(Event) + '_ {
    move |e| match e {
        Event::Thinking => ui.update(move |s, _| s.on_thinking(attempt)),
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
