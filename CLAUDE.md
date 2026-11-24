# Alfred

@memory/harness.md

A local, voice-first assistant for the Mac in Rust, with a Rune UI. Start with `README.md`; design decisions are in `docs/specs/`. The agents and their roles are in `AGENTS.md`.

## Commands

- `cargo test --workspace`: the whole suite. The Kokoro → Moonshine round trip runs only when `alfred setup` has downloaded the voice models.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check`: CI runs both.
- `cargo run -p alfred -- eval [--model] [eval/apps.toml | eval/thinking.toml]`: score the router (and the configured model) on `eval/utterances.toml`, the shortcuts and on-screen fixture, or the hard questions.
- `cargo run -p alfred -- shortcuts`: list the user's Shortcuts.
- `cargo run -p alfred -- --snap out.png --scene conversation --scale 2`: render the panel headlessly (scenes: idle, listening, thinking, speaking, conversation, confirm).
- `cargo run -p alfred -- ask --dry-run "…"`: run the brain without touching the Mac.
- `scripts/bundle-macos.sh [--install]`: build `dist/Alfred.app`.

## Rules

- `alfred ask` without `--dry-run` acts on the real Mac (opens apps, writes notes and reminders, reads Mail and opens reply drafts, runs Shortcuts that drive real devices, operates apps on screen): only when asked to.
- Mail is drafted, never sent: `crates/alfred-mac/src/mail.rs` has no send command, and a test fails if one appears.
- Text written by someone else (an email, an app's window) is spoken but kept out of the model's history: `Action::reads_outside_text` marks the actions that return it.
- On-screen actions (`crates/alfred-hands`) never click what `guard::is_risky` names; such clicks return as a follow-up the user confirms.
- Never interpolate user or model text into AppleScript; pass it through `on run argv` (see `crates/alfred-mac`).
- Keep `alfred-core` free of UI and native code.
