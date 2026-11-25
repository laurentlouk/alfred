# Alfred

@memory/harness.md

A local, voice-first assistant for the Mac in Rust, with a Rune UI. Start with `README.md`; design decisions are in `docs/specs/`. The agents and their roles are in `AGENTS.md`.

## Commands

- `cargo test --workspace`: the whole suite. The Kokoro → Moonshine round trip runs only when `alfred setup` has downloaded the voice models.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check`: CI runs both.
- `cargo run -p alfred -- eval [--model] [eval/apps.toml | eval/thinking.toml | eval/page.toml | eval/cards.toml]`: score the router (and the configured model) on `eval/utterances.toml`, the shortcuts and on-screen fixture, the hard questions, requests about the page open in Chrome (a case can say where the user is: `app`, `site`, `alfred_site`), or the app cards (Music, texts, Mail search, YouTube, Claude Code).
- `ALFRED_LIVE_BROWSER=1 cargo test -p alfred-hands --test live_browser live_youtube_videos -- --ignored --nocapture`: YouTube's first videos for real queries, headless, nothing played.
- `cargo run -p alfred -- shortcuts`: list the user's Shortcuts.
- `cargo run -p alfred -- --snap out.png --scene conversation --scale 2`: render the panel headlessly (scenes: idle, listening, thinking, speaking, conversation, confirm).
- `cargo run -p alfred -- ask --dry-run "…"`: run the brain without touching the Mac (it still reads which app the user is in).
- `printf 'go to the pokemon website\nwhat pokemon do you see?\n' | cargo run -p alfred -- chat`: a multi-turn conversation on the terminal.
- `ALFRED_LLM_TRACE=1 cargo run -p alfred -- ask --dry-run "…"`: each model call's time, split into loading, reading the prompt and replying (counts only, never text).
- `scripts/bundle-macos.sh [--install]`: build `dist/Alfred.app`.

## Rules

- `alfred ask` without `--dry-run` acts on the real Mac (opens apps, writes notes and reminders, reads Mail and opens reply drafts, runs Shortcuts that drive real devices, operates apps on screen): only when asked to.
- Mail is drafted, never sent: `crates/alfred-mac/src/mail.rs` has no send command, and a test fails if one appears.
- A text goes through Messages only as `Action::SendText`, which only a found contact's confirmation produces (`messages::find`); the model and the router can't produce it, and it is `Risk::Confirm`.
- Claude's links (`crates/alfred-mac/src/claude.rs`) only fill a prompt in; Alfred never presses Return in Claude.
- Text written by someone else (an email, an app's window) is spoken but kept out of the model's history: `Action::reads_outside_text` marks the actions that return it.
- What the user is looking at reaches the model as an app's name and a site's host only (`alfred_core::context`), never a window title or page text.
- Recordings (`alfred_core::record`) are off by default, stay on the Mac in files only the user can read, and are never sent anywhere; a dry run records nothing. The app shows `● rec` while recording.
- On-screen actions (`crates/alfred-hands`) never click what `guard::is_risky` names; such clicks return as a follow-up the user confirms.
- Never interpolate user or model text into AppleScript; pass it through `on run argv` (see `crates/alfred-mac`).
- Keep `alfred-core` free of UI and native code.
