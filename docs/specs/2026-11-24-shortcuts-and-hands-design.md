# Shortcuts and on-screen hands: design

Status: approved (roasted 2026-11-24; questions self-answered at the owner's request).

## Problem

Alfred v0.1 acts through seven hand-written tools. The owner wants it to reach every app: Messages, Mail,
home devices, Slack. Two complementary bridges do that:

- **Shortcuts**: run the user's own shortcuts. Fast, one call, no window, works with the 0.6B model.
- **Hands**: operate any app through its accessibility tree with `agent-desktop`, for apps that publish no
  actions at all.

## Goals

- "Start the vacuum" or "run Living Room Heat" runs the user's shortcut.
- "Reply to my last email saying I'll be there at 3, as a draft" works by operating Mail on screen.
- Nothing irreversible happens without a yes: no Send, Delete, Pay or Quit click without confirmation.
- Text read from an app (an email body) can never make Alfred act beyond the task it was given.
- Both bridges are optional: Alfred works the same without the `shortcuts` command or `agent-desktop`.

## Non-goals

- Building our own accessibility engine.
- Physical input (`--headed`: moving the pointer, synthetic keystrokes, stealing focus).
- Recipes (fixed step sequences per app). The loop comes first; recipes follow once real traces show
  which flows repeat.

## Decisions

1. **Shortcuts through the `shortcuts` command** (`list`, `run NAME -i FILE -o FILE --output-type
   public.plain-text`). Input text goes through a temporary file, never the command line. A shortcut's text
   output becomes Alfred's answer. *Rejected:* opening `shortcuts://run-shortcut` links (no output, no
   error, and it brings the Shortcuts app forward).
2. **Shortcut names are discovered at start and offered to the model as an enum**, so the 0.6B model picks
   an existing name and cannot invent one. Names are matched accent- and case-insensitively, since the
   owner's are French. A name that isn't found triggers one fresh listing before failing.
3. **Who confirms a shortcut.** A request that names a shortcut ("run Bonjour", "run the Réveil shortcut")
   runs at once: the user asked for that exact shortcut. A shortcut the model chose from looser words asks
   first ("Shall I run “démarre la musique”?"), unless its name is in `[shortcuts] trusted`. Shortcuts drive
   real devices, and a tiny model can pick the wrong one. A single-word name never runs on its own (a
   shortcut called "Bonjour" must not fire when the user says "bonjour").
4. **Hands through the `agent-desktop` command**, called with JSON output and never `--headed`. Every call
   has a timeout. *Rejected:* the C library (unsafe foreign calls in a crate that forbids `unsafe`), its
   Rust crates (internal APIs, no stability promise).
5. **One tool, `use_app { app, text }`, runs a loop:** observe (snapshot of that app's window), decide (one
   model call), act (click, type, set a value, scroll), verify, at most 8 steps. Only refs from the latest
   snapshot of the named app are accepted, so the loop cannot wander into another app.
6. **The loop runs on a bigger model** (`[desktop] model`, default `qwen3:4b`): reading an interface tree is
   beyond 0.6B. Each decision is schema-constrained like the main agent's: `{action: click | type | scroll |
   done | fail, ref, text, note}`.
7. **The guard stops before irreversible clicks.** A click on an element whose name contains a risky word
   (send, delete, remove, pay, buy, submit, post, publish, transfer, erase, unsubscribe, quit, and their
   French forms) is not performed; the loop ends with that click as a pending action, and Alfred asks
   ("Shall I click “Send” in Mail?"). On yes, the element is found again by role and name and clicked.
   *Hardened after a security review:* text typed as keystrokes (when a field can't take a value) has its
   line breaks turned into spaces, because a typed Enter sends in chat apps; setting a value sends no keys
   and keeps them. Unnamed buttons, links and menu items are never clicked, because an icon-only Send has
   no name for the guard to read. *Not adopted:* an allowlist where every other click asks first, which
   would stop each step of a task (Reply, a message row). *Residual risk:* a control labelled in another
   language, or misleadingly, gets past the word list; the one-app scope and the step cap still hold.
8. **Snapshots are compacted for the model:** one line per element, `e12 button "Reply"`, static text kept
   (that is where an email's content is), values clipped to 120 characters, the whole view capped at 9,000
   characters. Refs are shown without their snapshot prefix and mapped back.
9. **The loop's model sees app text as data.** Its prompt says so, but the real protection is structural:
   the loop's model can only act inside the one app, cannot call Alfred's other tools, stops at 8 steps, and
   the guard holds the irreversible clicks.
10. **Permissions.** agent-desktop needs Accessibility, granted to the app that launches it (Alfred.app, or
    the terminal for the CLI). `alfred doctor` reports it; a missing grant returns the exact System Settings
    path instead of failing silently.
    *Asking, added 2026-11-24 at the owner's request ("it should open the system panel like other apps"):*
    when agent-desktop reports Accessibility missing, at launch and before an on-screen action, Alfred
    calls `AXIsProcessTrustedWithOptions` with the prompt option (through `osascript -l JavaScript`, which
    macOS attributes to Alfred, since the workspace forbids `unsafe`), which shows macOS's own request and
    puts Alfred in the list, and opens System Settings at Privacy & Security → Accessibility. Nothing is
    switched on for the user. `scripts/bundle-macos.sh` signs with the Mac's Apple Development identity
    (or `ALFRED_SIGN`): an ad hoc signature changes with every build, and macOS forgot the grant at each
    reinstall, which is what the owner ran into.
10a. **Read first, and name the common places (added 2026-11-24, at the owner's request: reading the
    latest WhatsApp message took minutes).** A task that asks to be told something (read, who, what, how
    many, latest, quel, dernier…, and no action word such as reply, send, search, open or check) first gets
    one look: the window as it is, the question, and a `{found, answer}` schema. When the window answers
    it, that is the reply, with no plan and no click; otherwise the loop runs as before. The same first look
    applies to a question about an open web page. A look whose answer only echoes a field ("found", "true")
    counts as not found: `qwen3:8b` once answered "found". The compacted view also starts with `KEY PLACES`:
    the message box (a field named like "Type a message", "Saisissez un message", or the one field besides
    search), the search field and the send button, found by role and name in several languages; the step
    and plan prompts point at them. Names lose the invisible marks some apps add (WhatsApp writes U+200E
    before its own name and its fields'). Measured live: "read my latest message on WhatsApp" answered in
    one look, 5.5 s from the command line; "how many conversations are listed?" answered "14".
10b. **Messages follow a recipe, not a model (added 2026-11-24, at the owner's request: replying took
    minutes).** `send_message { app, to, text }` is its own tool. `DesktopHands::send_message` opens the
    conversation (the row whose name starts with `to`, else through the search field; the newest row,
    named like "Name, last message, time", when nobody is named; nothing when the app has one message
    box, like an assistant app), types into the message box (keystrokes, which chat apps react to, with
    line breaks gone so no Enter), and returns the send button as the follow-up the user confirms. No
    model call; each step is one agent-desktop call. A step it can't find hands the task to the loop. `to`
    is kept only when the user said it, so a guessed name never opens someone else's chat. Checked
    read-only on the owner's Mac: WhatsApp (message box "Saisissez un message", search, 25 conversations)
    and Messages (message box "Message", search, 29 conversations; no send button, Return sends, so
    Alfred leaves it written). Claude's window, holding a very long conversation, couldn't be read within
    20 s (nor through `find`): the recipe then reports that it couldn't see the window.
11. **Pending actions come from executors too.** `Outcome` gains an optional follow-up action. The agent
    turns it into the pending confirmation exactly like a risky router action, so the UI's Yes/No card and
    the CLI's prompt need no new code path.

12. **Shortcut input only when the user gave some (added while testing).** Asked to pick a shortcut,
    `qwen3:4b` paraphrased the request into its input ("Start vacuuming"). A shortcut that takes input would
    act on that, so model-chosen input is kept only when the request carries text after a colon or in quotes.
13. **"start X" opens only apps Alfred knows (added while testing).** "start the vacuum" had routed to an
    app called "Vacuum"; `open` and `launch` still take any name.

## Measured (2026-11-24, M1 Max, `eval/apps.toml`, 15 requests)

| Main model | Loose requests (11) | Overall | Time per decision |
| --- | --- | --- | --- |
| `qwen3:0.6b` | 1 right (9%) | 33% | about 0.2 s |
| `qwen3:4b` | 10 right (91%) | 93% | 0.5 to 1 s |

### Live runs in Mail (2026-11-24, French-language UI, owner's real inbox)

| Task | Model driving the steps | Result |
| --- | --- | --- |
| "who sent my most recent email and what is it about" | `qwen3:4b` | Right answer in 34 s, read from the message list |
| reply to the newest email as a draft | `qwen3:4b` | Never reached Reply; stopped honestly after 8 steps |
| same | `qwen3:8b` | Planned in French ("Répondre"), selected the message, opened the reply window; typed into the Reply-To field because Mail's body is a web view with no actionable element |

What the live runs fixed in the code: a pipe deadlock on large snapshots (output is now read while the
program runs), views that hid message subjects (budget shared between regions, rows kept whole), toolbar
buttons with no names (the menu bar is now part of the view), invented refs (the `ref` field is an enum of
the refs on screen), runaway output under a grammar (every string has a `maxLength`), repeated no-ops
(clicking what is already selected is skipped; a ref clicked twice for nothing is taken out of the
grammar), false success claims (a run that circles is reported as unfinished with only what was really
done), and losing the thread (a short plan is written first and shown at every step).

What they established: reading works on 4B; menu navigation needs 8B (now the default for
`[desktop] model`); and Mail's compose body can't be filled through the accessibility tree without
physical keystrokes, which Alfred doesn't use. Mail drafts belong to a typed tool on Mail's own scripting
(`reply`, then set the content); on-screen control is for apps without scripting or Shortcuts. That tool
now exists: see `2026-11-24-mail-design.md`.

The router handled the 4 requests that named a shortcut, at 100%. `qwen3:4b`'s miss: "switch on the light"
picked "Lights Off", which the confirmation for model-picked shortcuts catches. On the owner's own shortcuts
(French names), `qwen3:4b` mapped "start the vacuum", "stop the vacuum cleaner" and "make the living room
radiator comfortable" to the right ones. The live hands path was exercised up to the Accessibility check;
a full on-screen run waits for the owner to grant Accessibility.

## Vertical slices

1. `alfred shortcuts` lists them; `alfred ask "run Bonjour"` runs one (dry run first).
2. The model picks a shortcut from loose words, with confirmation.
3. `agent-desktop` client: snapshot, find, click, type, with the JSON envelope parsed and errors mapped.
4. The hands loop against a scripted model and a recorded app.
5. The guard and the pending click.
6. Live: a draft reply in Mail, once Accessibility is granted.

## Edge cases

- **No `shortcuts` command or none defined:** the tool isn't offered; the router doesn't match.
- **Shortcut asks a question mid-run** (Ask for Input): it waits until the 120 s timeout, which is reported.
- **agent-desktop missing or Accessibility denied:** `use_app` answers with the install command, or asks
  for Accessibility and opens System Settings at the list.
- **The app isn't running:** `launch` first, then snapshot.
- **`STALE_REF` or `AMBIGUOUS_TARGET`:** take a new snapshot and let the model decide again (counts as a
  step).
- **An action whose delivery is uncertain** (`retry` not `safe`): never repeated blindly; the next snapshot
  decides.
- **The model loops** (same action twice on an unchanged view): the loop stops and reports what it did.

## Self-answered questions

| Question | Answer | Source |
| --- | --- | --- |
| Does the owner have shortcuts? | yes, named in French; several drive home devices | `shortcuts list` |
| Can text be passed to a shortcut? | Yes, as a file with `-i` | `shortcuts help run` |
| Does agent-desktop need the window in front? | No: ref actions are headless | its desktop skill |
| What does a ref look like? | `@<snapshot>:e<N>`, valid for that snapshot only | its observation reference |
| When is retrying safe? | Only when `disposition.retry` is `safe` | its interaction reference |
| Which permission? | Accessibility, for the launching app | `agent-desktop status` |
