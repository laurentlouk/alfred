<p align="center">
  <img src="assets/orb.png" width="160" alt="The Alfred orb: a sphere of green liquid swirling behind glass">
</p>

<h1 align="center">Alfred</h1>

<p align="center"><strong>A local, voice-first assistant for your Mac.</strong><br>
Summon it, say or type what you need, and it opens your apps, saves notes, adds reminders and searches the web: on your Mac, with a local model, in a calm British voice.</p>

<p align="center">
  <img src="docs/images/conversation.png" width="300" alt="Alfred floating over the desktop: the orb, the request field, a request, its answer, and the action as a chip">
</p>

## What it does

- **Always at hand.** Alfred is an orb that floats over your windows, on your main display and on every desktop, with no window around it. <kbd>⌃⇧Space</kbd> brings it forward, it's in the Dock and <kbd>⌘Tab</kbd> like any app, and <kbd>esc</kbd> puts it away.
- **Just say "Alfred".** "Alfred, what time is it?" works from across the room, like "Hey Siri" (turn on `wake_word`). Or click the orb (<kbd>⌘L</kbd>) and speak. Your words appear as you say them, background noise is cleaned out, and Alfred answers about half a second after you stop. Say "Alfred" while it talks to cut it short.
- **It does things.** Opens and quits apps, saves notes in Notes, adds reminders, searches the web, opens links, tells the time.
- **It answers from the web.** "What's the weather in Paris?" opens the results in Alfred's own browser window, reads them, and tells you.
- **It thinks when it has to.** "Good evening" is answered at once; "I save 150 euros a month, when do I reach 2,000?" is thought through first.
- **It reads your mail and drafts replies.** "Read my last email", "reply to it saying I'll be there at 3": the reply opens in Mail for you to check and send. Alfred never sends mail.
- **It runs your Shortcuts.** "Run Good Morning", or just "start the vacuum" when you have a shortcut for it.
- **It can use other apps on screen.** With [agent-desktop](https://github.com/vercel-labs/agent-desktop) installed, Alfred can operate apps that have no shortcut, such as Slack or WhatsApp.
- **It talks back.** Every answer is spoken with Kokoro, sentence by sentence, at your Mac's volume, while the orb pulses with the voice.
- **Everything stays on your Mac.** The language model runs in Ollama, speech and dictation run in-process. No account, no cloud.
- **Safe by default.** No shell, no arbitrary scripts. Quitting an app, a shortcut Alfred picked for you, and any Send, Delete or Pay click wait for your yes.

Try:

```text
open Notes and save pick up the dry cleaning
search Google for the Rune UI framework
remind me in 20 minutes to stretch
open Safari and search for train times to Bath
read my last email
reply to my last email from Ada saying see you Friday
quit Music
```

## Get started

You need an Apple-silicon Mac, [Rust](https://rustup.rs), and [Ollama](https://ollama.com).

```bash
brew install ollama
```

```bash
ollama serve
```

In another terminal:

```bash
git clone https://github.com/laurentlouk/alfred.git && cd alfred
```

```bash
cargo run --release -- setup
```

`setup` writes the config, downloads the voice (Kokoro-82M, 350 MB), dictation (Moonshine tiny, 28 MB) and listening models (the "Alfred" keyword spotter, voice detection and noise cleaning, about 20 MB), and pulls the language model (`qwen3:0.6b`, about 520 MB, which runs on any Apple-silicon Mac; see [Choosing a model](#choosing-a-model) for a better one). Then build and install the app:

```bash
scripts/bundle-macos.sh --install
```

That puts `Alfred.app` in `~/Applications` and opens it. The first time Alfred saves a note, adds a reminder or reads your mail, macOS asks whether Alfred may control Notes, Reminders or Mail; the first time you talk, it asks for the microphone.

## The command line

The same brain runs headless, which is handy for scripts and for trying things out. Put `alfred` on your `PATH` first:

```bash
cargo install --path apps/alfred
```

```bash
alfred ask "open Notes and save buy oat milk"
```

| Command | What it does |
| --- | --- |
| `alfred` | Open Alfred |
| `alfred ask TEXT` | Do what TEXT asks. `--dry-run` shows the action instead, `--yes` confirms, `--speak` says the answer |
| `alfred say TEXT` | Speak with Alfred's voice |
| `alfred listen FILE.wav` | Transcribe a recording |
| `alfred setup` | Write the config, download the models, pull the language model |
| `alfred doctor` | Check what is ready and what is missing |
| `alfred shortcuts` | List the Shortcuts Alfred can run |
| `alfred eval [--model]` | Score Alfred on the request fixture in `eval/utterances.toml` |

## How it works

```text
 microphone ─► "Alfred"? ─► noise cleaning ─► voice detection ─► Moonshine ─┐
                                                                            ▼
 you ──► orb and field ──► router ──── recognized ───► action ──► Notes · Reminders · Mail · browser · apps
  ▲                          │                           ▲
  │                          └─ not recognized ─► model ─┘   (hard question? think it through first)
  │
  └──── Kokoro voice ◄── reply
```

1. **Listening** runs on one microphone stream. A keyword spotter waits for "Alfred"; then the audio is cleaned of steady noise, a voice detector (Silero) decides where speech ends, and Moonshine writes it down as you speak. The detector learns the room's noise between words, and a voice much quieter than yours (someone across the room) neither starts nor prolongs a command. A finished sentence ends after 0.6 s of quiet, one that trails off ("open Notes and…") after 1.2 s.
2. **The router** turns everyday phrasings into actions instantly, with no model at all: "open …", "save a note …", "remind me in … to …", "search Google for …", "what time is it". It errs on the side of passing a request on: a wrong action is worse than a slower right one.
3. **The model** handles everything else. A 0.6B model picks the right tool but mangles its arguments, so Alfred asks it for a JSON decision under a schema whose `action` is an enum of the tools (Ollama's structured outputs). It cannot answer with prose where a tool belongs or invent a tool. Arguments are then repaired (a string sent as an object, a renamed field) and fields the user never said (a reminder delay) are dropped.
4. **Actions** run through `open` and `osascript`. Every AppleScript is a constant that reads its inputs from `on run argv`, so whatever you dictate is text, never code. Links must be `http(s)`. Quitting an app needs a yes.
5. **Answers** come from what the actions did ("I've saved a note: “Buy oat milk”."), not from the model's summary of it, which saves a second model call and keeps a small model honest.
6. **Hard questions** are thought through. The model flags a question that needs working out (or the words plainly ask for it: sums, percentages, "step by step"), and Alfred asks again with thinking on, optionally of a second model (`think_model`). Actions never think.

### Measured

`alfred eval --model` on the 46 requests in [`eval/utterances.toml`](eval/utterances.toml), with `qwen3:0.6b` on an M1 Max:

| Path | Share of requests | Correct | Time |
| --- | --- | --- | --- |
| Router | 59% (27 of 46) | 100% | under 1 ms |
| Model, structured decisions | 41% (19 of 46) | 79% | 0.1 to 0.3 s |
| **Overall** | | **91%** | |

Native tool calls, measured on the earlier 40 requests for comparison, got 47% of the model's share right against 82% for structured decisions.

A bigger model follows looser requests better. With `qwen3:4b` the same fixture scores 96%: it also gets the loose mail requests ("has anyone emailed me this morning?", "tell whoever wrote to me last that I got it") that `qwen3:0.6b` answers with chat.

### Choosing a model

The default runs on any Apple-silicon Mac. With more memory, Gemma 4 does much better. Measured on an M1 Max with 64 GB:

| | `gemma4:e4b` (9.5 GB) | `gemma4:26b` (16 GB) |
| --- | --- | --- |
| Requests ([`eval/utterances.toml`](eval/utterances.toml), 51) | 92% | 96% |
| Apps and shortcuts ([`eval/apps.toml`](eval/apps.toml), 21) | 76% | 95% |
| Hard questions ([`eval/thinking.toml`](eval/thinking.toml), 11) | 100%, 8 s each | 91%, 51 s each |
| Quick answer | 0.3 to 0.5 s | 0.5 to 0.8 s |

`26b` picks the right action more often but thinks slowly; `e4b` thinks fast. With 32 GB or more, use both: `26b` decides and `e4b` thinks hard questions through. Together they got all 11 hard questions right at 6.4 s each, with quick answers at 0.5 s.

```bash
ollama pull gemma4:26b
```

```bash
ollama pull gemma4:e4b
```

```toml
[llm]
model = "gemma4:26b"
tool_mode = "native"
think_model = "gemma4:e4b"
```

Ollama's Apple-silicon builds of Gemma can't do structured output; Alfred notices and uses native tool calls anyway, and `tool_mode = "native"` skips that first try. On a Mac with less memory, `gemma4:e4b` alone (no `think_model`) needs about 10 GB, and `qwen3:4b` (2.5 GB) is a smaller step up from the default.

## Reaching other apps

**Mail.** "Read my last email", "who emailed me last", "anything new in my inbox?" and "lis mon dernier mail" read the newest email in your inbox: who sent it, when, the subject and how it begins. Name a sender ("the last email from Ada") to read theirs. "Reply to my last email saying …" (or "réponds au dernier mail de Marie en disant …") opens a reply in Mail with your words in it and the email quoted below, brought forward for you to check and send. Without words, Alfred asks what to say. Alfred has no way to send mail: the script it runs contains no send command, and a test keeps it that way. What an email says is spoken to you but never kept in the conversation the model sees, so an email can't give Alfred orders.

**Shortcuts.** Every shortcut in your Shortcuts app is available. Name one ("run Good Morning", "run the Lights Off shortcut") and it runs at once. Say it more loosely ("I'm cold in the living room") and the model picks the shortcut, then asks before running it, because shortcuts drive real devices. Names you trust can skip the question: list them in `[shortcuts] trusted`. `alfred shortcuts` shows them all.

**On screen.** For apps with no shortcut, Alfred can operate the app itself through its accessibility tree:

```bash
brew install agent-desktop
```

Then allow Alfred in System Settings → Privacy & Security → Accessibility (or your terminal, for the command line). Alfred looks at the app's window, picks one step, acts, and looks again, at most 8 steps. It works only inside the app you named, it never moves your pointer, and it stops before anything irreversible: a Send, Delete, Pay or Publish click comes back to you as a Yes/No question. Text inside the app, like an email, is treated as content, never as instructions. Reading an app's interface needs a bigger model than the default, set in `[desktop] model` (`qwen3:8b` by default):

```bash
ollama pull qwen3:8b
```

Tested live in Mail: `qwen3:4b` answers questions about what's on screen (who sent the latest email) but loses its place in multi-step tasks; `qwen3:8b` plans and follows menu commands (select a message, Reply). Mail's message body is a web view that can't be filled without physical keystrokes, which Alfred never sends, so Mail has its own tools (above).

Measured with `alfred eval --model eval/apps.toml` (15 requests: named and loose shortcuts, on-screen tasks):

| Main model | Loose requests right | Overall | Time per decision |
| --- | --- | --- | --- |
| `qwen3:0.6b` | 1 of 11 | 33% | about 0.2 s |
| `qwen3:4b` | 10 of 11 | 93% | 0.5 to 1 s |

Named shortcuts go through the router and work with any model. For loose requests to your apps, set `[llm] model = "qwen3:4b"`.

**On the web.** With [agent-browser](https://github.com/vercel-labs/agent-browser) installed, Alfred has its own Chrome window, with its own profile (`~/Library/Application Support/Alfred/browser`, never your Chrome profiles):

```bash
brew install agent-browser
```

A question that needs the web ("what's the weather in Paris", "who won the last Champions League final") opens the search results in that window, and Alfred reads them and answers in a sentence, saying where the answer came from. A task that takes a few steps on a site ("on amazon.fr, find the price of the Kindle Paperwhite", "compare the AirPods Pro on apple.com and fnac.com") runs the same look-decide-act loop as on screen, at most 10 steps, with the model from `[desktop] model`. Alfred only opens an address you said; it follows links on the page, goes back or searches, but never opens an address it read on a page. It never types passwords, card numbers or sign-in codes, and a Place order, Book, Subscribe or Sign up click comes back to you as a Yes/No question. Cookie walls are declined ("Reject all"); a robot check is left for you to answer in the window. The window stays open on the last page so you can see it and take over.

## Configuration

`~/Library/Application Support/Alfred/config.toml` (`ALFRED_HOME` moves the folder). Every field has a default:

```toml
[llm]
provider = "ollama"          # or "openai" for llama-server, LM Studio, mlx-lm…
tool_mode = "structured"     # or "native" for larger models
base_url = "http://127.0.0.1:11434"
model = "qwen3:0.6b"
thinking = "auto"            # think hard questions through; "never" or "always"
think_tokens = 2048          # room for a thought-through answer
# think_model = "gemma4:e4b" # another model for the thinking (unset: `model` thinks)

[voice]
speak = true
voice = "bm_george"          # British male; try bf_emma, af_heart, am_michael
speed = 1.05
wake_word = false            # true: keep the microphone open and listen for "Alfred"
silence_secs = 1.2           # quiet that ends a command that trails off
max_listen_secs = 20.0

[ui]
shortcut = "ctrl+shift+space"   # macOS keeps ⌃Space and ⌃⌥Space for switching keyboards
hide_on_blur = false        # true: put the panel away when another app comes forward

[search]
engine = "google"            # duckduckgo, bing
browser = ""                 # empty for your default browser, or "Safari"

[shortcuts]
enabled = true
trusted = []                 # shortcut names that run without asking when Alfred picks them

[desktop]
enabled = true               # on-screen actions, when agent-desktop is installed
model = "qwen3:8b"           # the model that reads app windows and web pages
max_steps = 8

[browser]
enabled = true               # website tasks and spoken search answers, when agent-browser is installed
command = "agent-browser"
profile = "~/Library/Application Support/Alfred/browser"   # Alfred's own Chrome profile
headed = true                # show the window, so you can watch and step in
max_steps = 10
```

## Develop

```bash
cargo test --workspace
```

The suite covers the router, argument repair, the agent loop (against a scripted model), the macOS actions (including an injection attempt that must come back as plain text), the UI (in Rune's headless runtime), and a Kokoro → Moonshine round trip when the models are downloaded.

The panel renders headlessly, which is how the images in this README were made:

```bash
cargo run -p alfred -- --snap docs/images/conversation.png --scene conversation --scale 2
```

Scenes: `idle`, `listening`, `thinking`, `speaking`, `conversation`, `confirm`.

| Crate | What it holds |
| --- | --- |
| [`crates/alfred-core`](crates/alfred-core) | Router, tools and argument repair, the agent loop, the LLM clients, config, eval. No UI, no native code |
| [`crates/alfred-mac`](crates/alfred-mac) | The macOS executor: apps, Notes, Reminders, browser |
| [`crates/alfred-voice`](crates/alfred-voice) | Listening (wake word, voice detection, noise cleaning, dictation), Kokoro speech, model downloads |
| [`crates/alfred-hands`](crates/alfred-hands) | Operating apps on screen with agent-desktop and web pages with agent-browser: the observe-decide-act loops and their guards |
| [`apps/alfred`](apps/alfred) | The Rune app and the `alfred` command line |

Design notes live in [`docs/specs`](docs/specs).

## Built with

- [Rune](https://github.com/anomalyco/rune), a GPU-rendered Rust UI framework (MIT)
- [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) for on-device speech, and its zipformer keyword spotter for "Alfred" (Apache-2.0)
- [cpal](https://github.com/RustAudio/cpal) for the microphone (Apache-2.0)
- [Silero VAD](https://github.com/snakers4/silero-vad) for voice detection (MIT) and [GTCRN](https://github.com/Xiaobin-Rong/gtcrn) for noise cleaning (MIT)
- [Kokoro-82M](https://huggingface.co/hexgrad/Kokoro-82M) voice (Apache-2.0), phonemized by [misaki-rs](https://github.com/MicheleYin/misaki-rs) (MIT)
- [Moonshine](https://github.com/moonshine-ai/moonshine) tiny for dictation (MIT)
- [Ollama](https://ollama.com), [Qwen3](https://huggingface.co/Qwen) and [Gemma 4](https://ai.google.dev/gemma) (Apache-2.0)
- [agent-desktop](https://github.com/vercel-labs/agent-desktop) for on-screen actions, optional (Apache-2.0)
- [agent-browser](https://github.com/vercel-labs/agent-browser) for web pages, optional (Apache-2.0)

No GPL code is linked: Kokoro's phonemes come from misaki-rs rather than espeak-ng.

## License

[MIT](LICENSE)
