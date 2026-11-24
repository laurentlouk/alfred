# Alfred v1: design

Status: approved (roasted 2026-11-24; the owner asked for the roast's questions to be self-answered).

## Problem

A personal assistant for the Mac that you summon, talk or type to, and that *does things*: opens apps,
saves a note in Notes, runs a Google search in the browser. It must run locally on a really small model,
speak with Kokoro, be MIT open source, and be built with [Rune](https://github.com/anomalyco/rune).

## Goals

- Summon a panel from anywhere, type or speak a request, and see and hear Alfred act on it.
- Act on the Mac: open and quit apps, create notes and reminders, search the web, open links.
- Run fully on-device: a sub-1B LLM through Ollama, Kokoro-82M for speech, Moonshine tiny for dictation.
- Stay reliable on a tiny model: common commands must not depend on the model getting JSON right.
- Be safe by default: nothing destructive without a confirmation, no arbitrary scripts or shell.
- Be testable headlessly: every layer has tests that need no model, microphone or window.

## Non-goals (v1)

- An on-screen companion that follows the cursor or draws on screen. Rune overlays can do it later; v1 is a
  summonable panel.
- Generic AppleScript or shell execution, file editing, and clicking in other apps (Accessibility).
- Cloud models, accounts, sync, scheduled jobs, skills marketplaces, MCP.
- Windows and Linux. The core crates stay portable; the actions and the app target macOS.

## Decisions

1. **Rust workspace, UI in Rune pinned to a `dev` revision** (`b3503d9`, toolchain 1.99.1, every
   `[patch.crates-io]` entry at the same rev, as Rune's README requires). *Rejected:* SwiftUI (not Rune),
   Tauri plus a web UI (heavier, and not what was asked).
2. **A regular app (Dock icon, ⌘Tab) whose one window is drawn as a panel and summoned by a global
   shortcut**, default `ctrl+shift+space` (configurable). `alt+space` is avoided because the Alfred launcher
   and ChatGPT already claim it; `ctrl+alt+space`, the first default, turned out to be macOS's "Select next
   source in Input menu", on whenever a Mac has two keyboard layouts, which takes the keystroke first. Escape
   puts the panel away (hidden, never closed: in Rune a closed window ends the app's run, shortcut
   included); a click on the Dock icon brings it back. *Changed after the owner's first use (2026-11-24):*
   v1 was an accessory app with a floating launcher panel that hid when it lost focus. The owner wanted
   Alfred in the Dock and in ⌘Tab like other apps, and a launcher panel never makes its app the active
   one, so switching to Alfred couldn't bring it back. The panel now stays behind other apps' windows
   (`[ui] hide_on_blur = true` restores hiding). Rune has no macOS status item yet, so there is no
   menu-bar icon. *Rejected:* an always-on overlay (v2).
3. **Two-tier brain: a deterministic intent router first, then the LLM agent with tools, then plain chat.**
   Measured on 2026-11-24: `qwen3:0.6b` picked `create_note` for "save a note that I need to buy milk
   tomorrow" in 1.9 s, but returned `body` as an object instead of a string. The router handles the common
   phrasings (open, quit, search, note, remind, time) with no model at all; the LLM handles the rest.
   *Rejected:* LLM only (a 0.6B model is too unreliable), rules only (too rigid).
4. **Argument repair before execution.** A string parameter that arrives as an object is unwrapped (`text`,
   `body`, `value`, `content`, then the first string field). Required fields still missing fall back to
   the user's own words where that makes sense, like a note body. Unknown tools are reported to the model,
   not executed.
5. **LLM provider: Ollama's native `/api/chat` (with `think: false` and `tools`) by default, plus an
   OpenAI-compatible provider** for llama-server, LM Studio and mlx-lm. The default model is `qwen3:0.6b`
   (about 520 MB), which is small enough to test with. The README recommends `qwen3.5:0.8b` or `qwen3:4b`
   for daily use. *Rejected:* in-process llama.cpp (slow builds and model management; a v2 candidate);
   the RunAnywhere SDK (Swift only, and its license restricts larger companies).
   *Thinking on demand (added 2026-11-24):* hard questions are asked again with `think: true`, optionally
   on a second model (`think_model`); see `2026-11-24-listening-thinking-orb-design.md`, decisions 9 to 11.
6. **Typed tool registry with risk levels.** Each tool has a name, a description, a JSON schema, a risk
   (`Safe` or `Confirm`) and an executor. Confirm-level tools pause on a Yes/No chip in the panel, or a
   prompt in the CLI. v1 tools: `open_app`, `quit_app` (Confirm), `create_note`, `create_reminder`,
   `web_search`, `open_url`, `get_time`.
7. **macOS actions through `open` and `osascript` with arguments passed through `on run argv`.** User text
   never goes into the script source, so AppleScript injection is impossible by construction. `open_url`
   accepts `http` and `https` only. App names must not contain a path. A `DryRun` executor records actions
   instead of running them, for tests and the eval.
8. **Voice out:** (superseded 2026-11-24: now the float32 build on the performance cores, brought to a
   speaking loudness; see `2026-11-24-listening-thinking-orb-design.md`, decisions 7 and 8) **Kokoro-82M v1.0 int8 through `sherpa-onnx` (prebuilt static libs, Apache-2.0), with
   phonemes from `misaki-rs` (MIT, Kokoro's own G2P), voice `bm_george` (id 26).** Measured: misaki
   phonemes round-tripped through Moonshine almost word for word, while sherpa's lexicon mode silently
   dropped "opened" as out-of-vocabulary. Speech is synthesized sentence by sentence so the first words
   play while the rest render (Kokoro ran at about real time on an M1 Max). macOS `say` is the fallback
   while the models are missing. *Rejected:* `piper-phonemize` and espeak-ng (GPL-3.0, would make the
   binaries GPL), a Python kokoro-onnx sidecar (a second runtime), Kokoro-FastAPI (Docker).
9. **Voice in:** (superseded 2026-11-24: one microphone stream with a wake word, Silero VAD and a
   denoiser; see `2026-11-24-listening-thinking-orb-design.md`, decisions 1 to 6) **Rune's `Recorder` (16 kHz WAV) plus Moonshine tiny EN
   quantized (28 MB) through `sherpa-onnx`.** Click the orb to talk. Recording stops on a click, after 1.4 s of silence, or after
   20 s. Measured: 4.4 s of speech transcribed in 55 ms. *Rejected:* whisper.cpp (a bigger model and
   build), Apple SpeechAnalyzer (Objective-C bridging, macOS only).
   *Live text (added 2026-11-24, at the owner's request):* while you speak, the words appear in the field.
   Every 0.6 s the voice thread transcribes the recording so far, read from the WAV still being written
   (its sizes are only filled in when it stops, so the reader takes everything after the `data` chunk). At
   55 ms for 4 s of speech, re-transcribing the whole recording costs less than a streaming model would to
   add. A file that can't be read yet shows nothing, and the final transcription is unchanged. *Rejected:*
   a streaming recognizer (another model to download, English only).
10. **Models and config live in `~/Library/Application Support/Alfred/`** (`config.toml`, `models/`).
    `alfred setup` downloads Kokoro and Moonshine from the official k2-fsa releases and pulls the Ollama
    model. `alfred doctor` reports what is ready and what is missing.
11. **One binary, `alfred`.** No arguments opens the app. Subcommands `ask`, `say`, `listen`, `setup`,
    `doctor` and `eval` run headless. Rune's own flags (`--snap`, `--tree`, `--scene`) still pass through
    for snapshots.
12. **Crates:** `alfred-core` (router, agent, tools, repair, config, eval; no Rune, no native code),
    `alfred-mac` (macOS executors), `alfred-voice` (Kokoro, Moonshine, model download), and
    `apps/alfred` (the Rune UI and the CLI).
13. **UI:** (superseded 2026-11-24: a transparent, always-on-top window holding a green liquid orb drawn
    on the GPU, on every Space; see `2026-11-24-listening-thinking-orb-design.md`, decision 12) A 560-point-wide dark card. A brass "Alfred orb" (concentric dotted rings) breathes when
    idle, ripples with the input level while listening, spins while thinking and pulses while speaking.
    Below it: a state caption ("At your service."), the input field, the last turns with tool chips
    ("Opened Notes", "Saved note 'Milk'"), and Yes/No chips for Confirm tools. Headless scenes (`idle`,
    `listening`, `thinking`, `speaking`, `conversation`, `confirm`) drive the tests and the README images.
14. **Tests:** unit tests across `alfred-core` (router table, repair, agent loop against a fake model,
    policy, script building), voice tests that skip themselves when models are missing, a Kokoro to
    Moonshine round trip, headless UI snapshots, and `alfred eval` over a fixture of utterances. The eval
    reports the router's share of fixtures, routing accuracy, and LLM accuracy in dry run, so the
    router's claimed coverage is measured.
15. **Grimoire:** the repo carries the harness (`AGENTS.md` roster, team agent, memory, `grimoire.config.json`)
    and keeps its specs in `docs/specs/`.
16. **Structured decisions for small models (added while testing).** In `tool_mode = "structured"` (the
    default), the model answers with JSON decoded under a schema whose `action` is an enum of the tools plus
    `chat`, with flat fields (`app`, `text`, `query`, `url`, `minutes`, `reply`). Measured on the 17
    model-shaped requests in `eval/utterances.toml` with `qwen3:0.6b`: native tool calls 8 of 17 (47%),
    structured decisions 14 of 17 (82%); overall with the router, 92%. Native tool calling stays available
    for larger models. Fields the user never said are dropped (a reminder delay without "minutes" or
    "hours"), and a `chat` decision with an empty reply gets one free-form chat call.
17. **Every model call is capped** at 256 output tokens. Found while testing: under a JSON grammar the 0.6B
    model once generated 27,000 tokens of padding; without the cap a request could hang until the timeout.

## Vertical slices (smallest first)

1. `alfred ask "open notes"`: router, then `open_app`, then Notes opens. CLI only.
2. `alfred ask "save a note: buy milk"` and `"search google for rust rune"`: Notes through argv-safe
   AppleScript, and a Google search in the default browser.
3. LLM fallback: `alfred ask "could you jot down that the plumber comes at 3"`, through Ollama, tool
   calling and argument repair.
4. `alfred say "..."`: Kokoro speech with `bm_george`.
5. The Rune panel: shortcut, orb, input, transcript, tool chips, confirm chips.
6. Voice in the panel: click the orb, speak, and Alfred acts and answers out loud.
7. `alfred setup`, `alfred doctor`, `alfred eval`, the README, CI, and the public repo.

## Edge cases and failure modes

- **Ollama not running or the model not pulled:** the router still works; LLM requests answer with an
  actionable line ("Start Ollama: `ollama serve`, then `ollama pull qwen3:0.6b`"). Requests time out at 60 s.
- **Model loops on tools or runs on:** at most 2 model rounds per request and 256 output tokens per call.
- **Duplicate tool calls in one turn** (small models repeat themselves): identical calls run once.
- **Automation permission denied** (first `osascript` to Notes): the error is surfaced in plain words with
  how to grant it in System Settings, Privacy & Security, Automation.
- **App not found:** `open -a` fails and the result says so. Nothing guesses at bundle paths.
- **Requests overlap** (a new request while one runs): the newer one wins and stale answers are dropped
  (`Latest` semantics, attempt-fenced).
- **Speech while a new request starts:** playback stops before the next answer speaks.
- **Microphone permission missing:** the orb explains it, and `doctor` reports it. The Info.plist key
  is set by the bundling script.
- **Voice models missing:** TTS falls back to `say`; the mic is disabled with a hint to run `alfred setup`.
- **Partial download:** files download to `*.part` and are renamed only after a full read, so a re-run
  resumes cleanly (idempotent setup).
- **Long text to speak:** split at sentence boundaries; a sentence over 400 characters is split at commas.

## Questions still owned by the user

- The name: "Alfred" is also a well-known macOS launcher (Running with Crayons). Fine for an open-source
  project, but a rename before a public release would avoid confusion.
- Whether v2 should add an on-screen companion overlay or a menu-bar icon first.

## Sources consulted

- Rune `dev` README, `AGENTS.md`, `examples/launcher`, `examples/background`, and the docs on `Recorder`,
  `Player`, `Latest`, `machine` and Depending on Rune.
- sherpa-onnx Rust API examples (`kokoro_tts_en_*`, `moonshine_v2`), `sherpa-onnx-sys` `build.rs`
  (prebuilt static libraries), and the release assets (model sizes).
- misaki-rs README and API; the license metadata of `piper-phonemize` (GPL-3.0).
- Spikes run on this machine: Rune headless snapshot, Ollama tool call with `qwen3:0.6b`, and the Kokoro
  to Moonshine round trip.

## Self-answered questions

| Question | Answer | Source |
| --- | --- | --- |
| What is "rune for macs"? | `anomalyco/rune`, the Rust GPU UI framework | the owner's link |
| Can Rune summon a panel globally? | Yes: `App::global_shortcut` with `WindowOptions::launcher_panel()` | `examples/launcher` |
| Menu-bar icon in Rune? | Not on macOS yet (`Tray::new` returns Unsupported) | Rune README, tray section |
| Microphone in Rune? | `Recorder` writes WAV and needs `NSMicrophoneUsageDescription` | Rune README, Recorder |
| Which tiny model? | `qwen3:0.6b` to test with; `qwen3.5:0.8b` and up for daily use | Ollama library, spike |
| Can a 0.6B model call tools? | It picks the tool, but the arguments come back malformed | spike, decision 3 |
| Kokoro without GPL? | `misaki-rs` (MIT) for phonemes and sherpa-onnx for inference | crates.io metadata, spike |
| Which voice for a butler? | `bm_george` (British male), id 26 | model metadata |
| STT model? | Moonshine tiny EN quantized, 28 MB, 55 ms for 4.4 s | spike |
| Notes without injection? | `osascript` with `on run argv` | AppleScript semantics |
| Bundle id? | `com.laurentlouk.alfred` | public GitHub handle |

## Docs drift fixed

None. This is a new repository.

## Reference projects

| Repository | Path | Pattern adopted | Not adopted |
| --- | --- | --- | --- |
| anomalyco/rune | `examples/launcher/src/main.rs` | global shortcut and launcher panel, close on blur | the notes window |
| k2-fsa/sherpa-onnx | `rust-api-examples/examples/kokoro_tts_en_phonemize.rs` | phoneme codepoints into Kokoro | piper-phonemize (GPL) |
