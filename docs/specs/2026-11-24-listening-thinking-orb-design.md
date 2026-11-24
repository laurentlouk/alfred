# Listening, thinking and the floating orb: design

Status: built and tested (2026-11-24). It supersedes decisions 8, 9 and 13 of
`2026-11-24-alfred-v1-design.md`, and adds to its decision 5.

## Problem

Alfred v1 worked, but daily use showed five gaps the owner asked to close:

- **Listening needed a click,** and it ended after 1.4 s of quiet. With a fan, a street or a television
  behind the speaker, the quiet never came, so a command never ended.
- **One small model did everything.** It answered "what's 17 by 23?" as fast as "good evening", and
  guessed. The owner wanted Gemma, and thinking only when a question needs it.
- **The voice was quiet and slow to start.** Speech was normalized to a fixed peak, so it played well under
  other apps, and Kokoro int8 took over a second before the first word.
- **The panel was a dark card** that had to be summoned. The owner wanted a transparent, always-on-top
  orb, like a plugin, that follows across desktops and moves with what Alfred is doing.
- **Text appeared only at the end** of dictation.

## Goals

- Say "Alfred, what time is it?" from across the room, like "Hey Siri".
- A command ends 0.3 to 0.5 s after a finished sentence, even with steady noise behind the speaker.
- Hard questions are thought through; easy ones and actions stay instant.
- Alfred's voice plays as loud as the Mac's other sounds, and the first word comes in about half a second.
- The orb floats over every window, on the main display, on every Space, with no background.

## Non-goals

- Echo cancellation. Alfred's voice from the speakers reaches the microphone; only "Alfred" interrupts it.
- Speaker identification. A quieter, more distant voice is ignored, but no voice is recognized.
- A custom wake word. "Alfred" is the only keyword.

## Decisions

1. **One microphone stream, owned by `alfred-voice`'s `Ears`.** A thread reads the microphone with cpal
   (Apache-2.0), runs the wake word, the denoiser, the voice activity detector and Moonshine, and reports
   `Heard` events: `Wake`, `Level`, `Partial`, `Final`, `Nothing`, `Error`. The app forwards them to the UI
   thread. Every duration is counted in samples, so tests feed recorded audio faster than real time.
   *Rejected:* Rune's `Recorder`, which writes a file and can't hand over samples as they arrive, so it
   can't spot a wake word or detect speech.
2. **The wake word is sherpa-onnx's keyword spotter** (zipformer, GigaSpeech, 3.3M parameters, int8,
   Apache-2.0), with the keyword line `▁A L F RE D @ALFRED` built from the model's own tokens. It is off by
   default (`[voice] wake_word`): when on, the microphone stays open and the indicator stays lit. Measured
   on an M1: 15 ms of CPU per second of audio, 200 of 200 isolated "Alfred"s caught across 5 voices, fired
   a median 0.32 s after the word. "Alfredo" still fires 14 times in 24. *Rejected:* a hosted or licensed
   wake-word engine (an access key, a non-open license), and training a custom model (no data to train on).
3. **Silero VAD alone decides where speech ends** (MIT). The room's noise floor is learned between words
   and raises the threshold from 0.6 up to 0.75; speech must be 12 dB above that floor and last 0.25 s to
   count, so a click or a cough doesn't start a command. Once the speaker's level is known (from "Alfred",
   or the first words), a voice 15 dB quieter neither starts nor extends the command. A sentence that
   looks finished ends after 0.6 s of quiet, one that doesn't ("open Notes and…") after
   `silence_secs` (1.2 s). Measured: steady noise 10 dB under the speech still ends a command 0.4 to
   0.47 s after the words. *Rejected:* an energy threshold (the v1 bug: noise is energy).
4. **A GTCRN denoiser cleans the audio** (MIT, 0.5 MB) for the VAD and the orb's level. Moonshine hears 70%
   cleaned and 30% original: on 10 sentences in noise that got 55, 59 and 53 of 67 words right against 44,
   56 and 38 raw. Fully cleaned audio scored worse than raw. The wake word hears the original audio, which
   keeps it cheap.
5. **Live text comes from the same thread:** at each pause Moonshine transcribes what has been said so far
   (41 ms for 2 s of audio), and `Final` reuses the last transcription. This replaced re-reading the
   recorder's growing WAV file.
6. **Saying "Alfred" interrupts.** While Alfred speaks, a wake-word hit still emits `Wake`; the app stops
   speaking and captures the command after it.
7. **Speech is brought to a speaking loudness** (RMS 0.16, about −16 dBFS, peaks at most 0.92, gain 1 to 4)
   and played at the system volume. *Rejected:* a fixed peak (quiet speech stayed quiet), a volume setting
   of Alfred's own (it should respect the Mac's).
7a. **Alfred plays its own speech** (`alfred_voice::player::Speakers`): one cpal output stream on the
   default device, each sentence queued as soon as Kokoro renders it, resampled from 24 kHz to the
   device's rate with sherpa-onnx's resampler (flushed at the end of a reply, dropped on a stop). Opening
   the device costs 0.16 to 0.6 s with AirPods, on every open, so the stream opens when a request begins
   (listening, a typed request, a confirmation) and closes after 30 s of quiet: warm, the first samples
   play 11 ms after they are queued. Measured on a three-sentence reply of 9.2 s: 10.5 s from text to
   silence, against 13.0 s with one `afplay` per sentence, which cost about 1 s each (a 0.2 s clip took
   1.0 to 1.5 s). *Rejected:* `afplay` (that cost, and a gap between sentences), and a stream open all the
   time (headphones would stay held by an idle app).
8. **Kokoro is the float32 multilingual v1.0 build, on the performance cores**
   (`hw.perflevel0.physicalcpu` threads), with a short first chunk so the first words play while the rest
   renders. Measured: 0.53 s to the first sentence against 1.24 s for int8 on 4 threads. It is a 350 MB
   download against 126 MB; speed won.
9. **Thinking on demand.** The decision schema has a `hard` flag the model sets on a question that needs
   working out, and `needs_reasoning` catches plain signs of one ("step by step", sums, percentages,
   "compare"). A flagged answer in words is asked again with Ollama's `think: true`, a reasoning prompt
   and its own budget (`think_tokens`, 2048); the quick answer stays as the fallback. Actions never think.
   `[llm] thinking` is `auto`, `never` or `always`. The UI shows "Thinking it through…".
10. **The model that thinks can differ from the one that decides** (`[llm] think_model`). Measured on an
    M1 Max with 64 GB:

    | | `gemma4:e4b` | `gemma4:26b` |
    | --- | --- | --- |
    | Requests (`eval/utterances.toml`, 51) | 92% | 96% |
    | Apps and shortcuts (`eval/apps.toml`, 21) | 76% | 95% |
    | Hard questions (`eval/thinking.toml`, 11) | 100%, 8 s each | 91%, 51 s each |
    | Quick answer | 0.34 to 0.52 s | 0.49 to 0.78 s |

    So on a Mac with room for both, `gemma4:26b` decides and `gemma4:e4b` thinks: together they got all
    11 hard questions right at 6.4 s each (median), with quick answers at 0.5 s. The default stays
    `qwen3:0.6b`, which runs on any Apple-silicon Mac. *Rejected:* `26b` for both (a minute per hard
    question), `e4b` for both (it misses a quarter of the loose app requests), and `qwen3.5:35b`
    (35B-A3B, tried later the same day): 94% of requests, 90% of app requests against 95% for `26b` on the
    same fixture, quick answers just as fast (0.6 to 0.8 s), and 11 of 11 hard questions but 34 s each.
    Ollama serves its default tag on Apple silicon as NVFP4/MLX, without structured output, so it can't
    drive apps either; the GGUF tags (`qwen3.5:9b-q4_K_M`, `qwen3.5:35b-a3b-q4_K_M`) can.
11. **Models without structured output fall back to native tool calls.** Ollama's Apple-silicon Gemma
    builds (NVFP4 and MLX) answer a `format` schema with 501. The agent switches to native tool calls for
    the session, parses Gemma's own call syntax when it arrives as text, and nudges once when a reply
    describes an action instead of calling it. The on-screen and browser hands need structured output, so
    `[desktop] model` stays `qwen3:8b`. Gemma's GGUF build `gemma4:e4b-it-q8_0` does follow a schema, but
    on live Wikipedia tasks (headless, agent-browser 0.44.0) it finished 1 of 3 against 3 of 3 for
    `qwen3:8b`: it took Wikipedia's "did you mean: rule programming language" suggestion both times it was
    asked for the first result on Rune, though it was faster on the one it finished (34 s against 56 s).
11a. **A request reads only what is new (added 2026-11-24: requests took 3 to 20 s).** Every request
    carries about 2,200 tokens of instructions and tools. Ollama keeps what it read of a model's last
    prompt and reuses it up to the first difference, so the instructions never change from one request to
    the next: where the user is (C1 of the browser design) moved from the instructions to a bracketed note
    before the request, and never enters the history. Measured with `ALFRED_LLM_TRACE=1` on
    `qwen3.5:35b`: reading the prompt again took 3.2 s, reused 0.1 s. Ollama also unloaded a model after the
    30 minutes Alfred asked for, and loading 21 GB took 11.5 s, so `[llm] keep_alive` (default 4 h) keeps
    it, and the app warms the model at launch with the same instructions and tools, one token of reply.
    Requests then took 0.8 to 2.4 s from the command line. *Rejected:* trimming the tools first (the
    reuse alone took reading from 3 s to 0.1 s; shorter definitions would only shorten the first read).
12a. **Smaller and quieter (added 2026-11-24, at the owner's request).** Answers are spoken, so they are
    no longer written: only a question waiting for a yes shows, with its Yes and No (and, when Alfred has
    no voice, its latest answer). Nothing is painted behind text, not even the soft shadow that kept it
    legible over light windows, because Rune draws a shadow as a box and it showed as one; over a light
    window the pale text is harder to read, the trade-off of a transparent widget. The orb's square no
    longer scales: only its inside swells, at most 1.11 times, and the glow and listening rings are bounded
    (32 points past the orb's edge) so everything stays inside it instead of being cut by the window, which
    a test checks. Dragging the orb moves Alfred (`window_drag`); it no longer starts listening. The field
    grows a line at a time up to four, then scrolls, following the end so the latest words stay in view,
    typed or dictated (`ScrollSpec { stick_bottom: true }`; Rune's `auto_grow` follows the caret only on a
    keystroke). With the wake word on, the mic button mutes: `Ears::set_muted` closes the microphone
    (macOS's indicator goes out) and nothing is heard, not even "Alfred", until it is pressed again.
12. **The panel is a floating orb.** A 300 × 480 point window, borderless, transparent and always on top,
    on every Space (`collection_behavior`), opened on the main display. Alfred is a regular app, in the
    Dock and ⌘Tab. The orb is drawn on the GPU by a Rune compositor in layers: a radial body, screened
    veins that swirl, a ripple, a glass lens, a glow, and radar rings while listening. Energy (by mode, plus
    the voice level) speeds the liquid and brightens the glow; it breathes when idle, swirls while
    thinking, swells with your voice while listening and pulses with Alfred's while it speaks. Text has a
    soft shadow so it reads on any wallpaper. *Rejected:* a dark card (the owner wanted no background), an
    animated video (it ignores alpha).

## Edge cases and failure modes

- **Microphone denied:** `Ears::start` succeeds and reports `Heard::Error`; with the wake word on it retries
  every 5 s, so granting access later works without a restart.
- **Models missing:** `Ears::start` fails, and a click on the orb says to run `alfred setup`, which
  downloads them. `alfred doctor` lists what is missing.
- **AirPods:** with the wake word on, the open microphone keeps them in their low-quality call mode.
- **A distant voice already talking when you click** can still start the command (the speaker's level isn't
  known yet).
- **Thinking runs out of tokens or fails:** the quick answer is said instead.
- **Thinking model not pulled:** the reasoning call fails and the quick answer is said; `alfred doctor`
  names the models.

## Sources consulted

- `crates/alfred-voice/src/{ears,vad,wake,denoise,capture}.rs` and their tests; measurements in this spec
  come from them and from `alfred eval --model`.
- sherpa-onnx's keyword-spotting, VAD and speech-enhancement model releases, and the keyword model's
  `bpe.model` for the token line.
- Ollama's `/api/chat` with `format` and `think` on `gemma4:e4b`, `gemma4:26b` and `qwen3`.

## Self-answered questions

- *Wake word on by default?* No: an always-open microphone is a privacy choice the user makes.
- *Should thinking slow every answer?* No: only flagged or plainly hard questions think.
- *Which display?* The main one, where the menu bar is, the owner's choice with two screens.
