# AGENTS.md: the roster

The agents the grimoire harness dispatches in this repository. The pipeline (`roast` → `to-plan` → `to-issues` → build ⇄ review → PR → `crystallize`), the scouts, the reviewer and the specialists come from the grimoire plugin; this file lists the project's own roles and what each owns. If a definition in `.claude/agents/` and this file disagree, the definition wins: fix the drift.

## Team agents

| Agent | Repo | Default model | Owns |
| --- | --- | --- | --- |
| `alfred-engineer` | `alfred` (the root) | sonnet (opus in unattended runs) | `grimoire:tdd`; the Rust workspace: `crates/alfred-core`, `crates/alfred-mac`, `crates/alfred-voice`, `apps/alfred` |

## Scouts in use

| Scout | Used for |
| --- | --- |
| `codebase-scout` | where and how something works today |
| `reference-scout` | how credible Rust or Rune projects solve a problem; never a stack change |
| `security-scout` | the action surface: what a model or a dictated sentence can make the Mac do (AppleScript, `open`, links) |
| `perf-scout` | voice latency (Kokoro synthesis, Moonshine, model calls) and the UI frame loop |
| `reviewer` | spec and quality verdicts on every diff |
| `tracker-scout` | GitHub Issues on laurentlouk/alfred, once work is tracked there |

`design-scout` and `contract-checker` don't apply: there is no design source and no cross-repository contract.

## Memory

| Store | File |
| --- | --- |
| Shared facts | `memory/harness.md` |
| `alfred-engineer` | `memory/agents/alfred-engineer.md` |
| `reviewer` | `memory/agents/reviewer.md` |

Written only by `crystallize`, after a PR. The rules are in the grimoire plugin's `memory/README.md`.

## Gates

None. Verification is `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`. The real-Mac actions (`alfred ask` without `--dry-run`) are never part of an agent's verification.
