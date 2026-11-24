<!-- HARNESS MEMORY: declarative facts for every root session. Cap 2200 chars. Entries separated by § . Written only by `crystallize`; see memory/README.md -->
Alfred is presented as its own project: code, docs and the README never name other assistant projects as inspiration; only the dependencies and models it ships with are credited (2026-11-24).
§
Rune is pinned to a dev revision (b3503d9) on toolchain 1.99.1; every [patch.crates-io] entry in the root Cargo.toml uses that same rev, and a bump moves all of them together (2026-11-24).
§
No GPL code is linked: Kokoro phonemes come from misaki-rs, never espeak-ng or piper-phonemize (2026-11-24).
§
AppleScript actions are constant scripts that read user text from `on run argv`; user text is never interpolated into script source (2026-11-24).
§
The default model qwen3:0.6b runs in tool_mode = structured: schema-constrained decisions scored 82% on the model-shaped cases of eval/utterances.toml against 47% for native tool calls (2026-11-24).
§
Every model call is capped at 256 output tokens; a 0.6B model under a JSON grammar once generated 27,000 tokens without it (2026-11-24).
