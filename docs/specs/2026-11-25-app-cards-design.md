# App cards: each app's fastest way in

Status: approved by the owner on 2026-11-25 ("do the rest, make the plan and execute it in auto"); the
daily WeWork booking is left for later.

## Problem

Alfred reaches most apps through the on-screen step loop: look at the window, ask the model for a step,
click, look again. It is general but slow (7 to 18 s a step) and it fails where a small model loses its
way. The owner's requests that went wrong or that they asked for next are everyday ones: play music and
playlists, text someone through Messages, search Mail, find a video that explains something on YouTube,
and start a Claude Code session with a prompt.

Most of these apps already offer a faster way in than their window: Music, Messages, Mail and Contacts
have scripting dictionaries; YouTube has a results address; the Claude app and Claude Code have official
links that open a new session with a prompt filled in. Assistants that do this well use an app's own
commands first, saved recipes second, and on-screen steps last.

## Goals

- Each of these requests done without the step loop, in one to three seconds where the app allows.
- The same safety as today: nothing sent without the user's yes, no user text in script source.
- The step loop stays the fallback for whatever a card doesn't cover.

## Non-goals

- Recipes learned from recordings, routines (a task every day), and the WeWork booking: later.
- Apple Music's streaming catalogue played by script (Music can't): a song not in the library opens
  Music's search for it instead.
- Sending email (Alfred still only drafts).

## Decisions

1. **An app card is a direct action, the step loop its fallback.** `Action::Music`, `Action::FindVideo`,
   `Action::SearchMail` and `Action::ClaudeCode` are new; Messages and Claude reuse `send_message`, which
   the executor now sends down the fast path for those two apps. *Rejected:* a generic card file the model
   reads before each step (it would still cost a model call per step, which is what makes the loop slow).
2. **Music through its dictionary.** play, pause, next, previous, what's playing, and the user's
   playlists. "Play X" tries a playlist of that name, then a song, an artist, then an album in the library,
   and plays the best match. Not in the library: Music opens its catalogue search for X
   (`music://music.apple.com/search?term=`), and Alfred says so. Pausing never starts Music.
3. **Messages through Contacts and its dictionary.** The person is looked up in Contacts (a mobile number
   first, then any number, then an email); the message goes to the existing conversation with that number
   (so an SMS conversation stays SMS), else to iMessage. Alfred asks first, naming the person and the
   number's label: "Shall I text Ada Lovelace's mobile: “See you at 7”?". The send is a new
   confirm-level action, `Action::SendText`, which only an outcome's follow-up can produce, never the
   model or the router. Several people match: Alfred names them and asks which. Nobody matches: the
   on-screen recipe looks in Messages' own list, as before. A number or an address the user says is used
   as is.
4. **Mail search by subject and sender, inbox only.** Up to five newest matches across accounts, three
   said aloud: who, when, and the subject. The words searched for lose a plural "s", so "invoices" finds
   "Invoice". *Rejected:* searching message bodies (Mail loads every body: tens of seconds on a large inbox)
   and every mailbox (Gmail's All Mail).
5. **YouTube without the model.** Alfred's browser opens YouTube's results for the request; the first
   video (never a Short or an ad) is read off the snapshot with its channel and opens in the user's
   browser, where they are signed in; Alfred says its title and channel. Without agent-browser, the
   results open in the user's browser instead.
6. **Claude and Claude Code through their official links.** `claude://code/new?q=…&folder=…` opens Claude
   Code in the Claude app with the prompt in the composer; `claude-cli://open?q=…&cwd=…` opens it in a
   terminal (`[claude] opens_in = "terminal"`); `claude://claude.ai/new?q=…` opens a new chat. The links
   only fill the prompt in: the user reads it and presses Return, and the Claude app asks before it uses a
   folder from a link. A folder the user names ("in the alfred project") is looked for, two levels deep,
   in `[claude] folders` and the usual code folders (`~/Developer`, `~/Projects`, `~/code`…). *Rejected:*
   typing into Claude's window (the app's accessibility tree takes over 20 s to read) and pressing Return
   for the user (the Claude app may be showing its folder question).
7. **The router knows the everyday phrasings** ("play my running playlist", "pause the music", "next
   song", "text Ada that I'm late", "find emails about the invoice", "find a video explaining
   transformers", "ask Claude Code to fix the tests in the alfred project"), so these run with no model
   call. The model gets the same four tools for the rest.
8. **Song titles, playlist names and contact names are not outside text**: they are the user's own
   library and address book, so they stay in the history ("what was that song?" works). Email subjects
   and video titles are written by others, so `reads_outside_text` marks those two.

## Edge cases

- **Music isn't running:** play starts it; pause, next and "what's playing" say nothing is playing.
- **An empty library or no playlists:** said in words; a play request falls back to the catalogue search.
- **Messages without an iMessage account** (an SMS-only Mac relay): the SMS account is used.
- **A contact with no number or address:** said, nothing is sent.
- **A wrong number in Contacts:** the confirmation names the number's label, and nothing goes before yes.
- **A folder that doesn't exist:** Claude Code opens without one, and Alfred says it didn't find it.
- **A prompt longer than a link allows** (5,000 characters for the terminal, about 14,000 for the app):
  clipped at a word, and Alfred says so.
- **YouTube's cookie wall:** turned down, as for searches. A robot check: the results open in the user's
  browser.
- **The first permission prompt** (Alfred controlling Music, Messages, Contacts or Mail): macOS asks
  once per app; the script waits for the answer within its timeout.

## Measured

On the owner's M1 Max, with `qwen3.5:35b-a3b-q4_K_M` deciding (native tool calls):

| Fixture | `main` | With the cards |
| --- | --- | --- |
| `eval/cards.toml` (24 requests) | (no cards) | 100%, 14 by the router at 100% |
| `eval/utterances.toml` (51) | 92% | 94% |
| `eval/apps.toml` (21) | 95% | 95% |
| `eval/page.toml` (13) | 92% | 92% (twice) |

Two prompt lessons on the way there. Adding four tools made the model call "go through the first three
results for rust gui frameworks" a plain search, until the browser line named "going through several
results". And "play the first video" on a YouTube page went to `find_video`, then (with a vague "a step on
that page") was answered in words, until the browser line named it as a page step; a `find_video` call
whose request points at the page is now refused with that advice, so the model retries.

YouTube live (headless, a throwaway profile): 1.4 to 3.4 s a query, no model call; titles and channels
read correctly after the avatar link's "Go to channel" label was taken off. Music, Messages, Contacts
and Mail scripts are compiled against the apps' dictionaries in the tests but were not run on the
owner's Mac: running them from a development shell would raise macOS permission prompts for the shell,
and texting or playing would act for real. The Claude links follow Anthropic's documentation and were
not opened, so as not to take over the Claude window during the build.

## Self-answered questions

| Question | Answer | Source |
| --- | --- | --- |
| Can Claude Code be opened with a prompt from outside? | Yes: `claude://code/new` (desktop app) and `claude-cli://open` (terminal), prompt filled in but never sent | support.claude.com article 14729294; code.claude.com/docs/en/deep-links |
| Does Music handle `music://` links? | Yes, `music` and `itmss` are its URL schemes | Music.app Info.plist |
| Can Messages send to an existing SMS conversation by script? | Yes: `send … to chat id …`; a chat's account gives its service | Messages.app scripting dictionary |
| Where are YouTube video links in a snapshot? | `snapshot -i -c -u` lines: `link "Title 27 minutes" [ref=…, url=…watch?v=…]`, then a "Go to channel" button | a live headless run on 2026-11-25 |
| Which apps have dictionaries on this Mac? | Music 31 commands, Mail 19, Messages 3 (send), Chrome 23, Safari 10; WhatsApp and Slack none | `sdef` |

## Files

`crates/alfred-core`: `action.rs` (the new actions), `tools.rs` (tools, schema, guide), `router.rs` (the
phrasings), `catalog.rs` (`claude`), `config.rs` (`[claude]`). `crates/alfred-mac`: `music.rs`,
`messages.rs`, `mail.rs` (search), `claude.rs`. `crates/alfred-hands`: `results.rs` (YouTube videos),
`web.rs` (`video`). `eval/cards.toml`.
