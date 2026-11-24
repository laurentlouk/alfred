# Recordings: design

Status: built and tried live (2026-11-24). First step of teaching Alfred from its own runs.

## Problem

The owner asked how to train the local model to do exactly what they want in apps and the browser. Three
ways exist, cheapest first: fixed recipes (as for messages), examples from the user's own requests put in
the prompt, and fine-tuning a small model. All three need the same raw material, which Alfred threw away:
what it was asked, what each model saw and answered, what each action did, and whether it worked.

## Goals

- Keep that material when the user asks for it, in a form fine-tuning takes as is.
- Nothing recorded unless the user turns it on; nothing ever leaves the Mac.
- Recording never slows or breaks a request.

## Non-goals

- Using the recordings (recipes, examples, training): later steps, once there is data to look at.
- Judging whether an answer was right. A record says what happened and whether the action reported
  success; the user's yes or no is kept, a silent wrong answer is not detected.

## Decisions

1. **Off by default** (`[recording] enabled = false`). On, the app's footer shows `● rec`. *Rejected:*
   on by default (what is on screen in WhatsApp or a web page is the user's to keep or not).
2. **JSON lines, one file a day** (UTC), in `recordings/` under Alfred's folder, the folder `0700` and the
   files `0600`. Every line carries `v`, `at`, `session` (when the recorder started, in milliseconds),
   `turn` and `kind`.
3. **Four kinds.** `request` (the text, where the answer came from, the reply, a pending question),
   `model` (the role `decide`, `think`, `app` or `web`; the messages sent, the tools or schema offered, the
   reply or the error, the time), `action` (the action, whether it worked, what it said, a follow-up it
   asks for, whether its words came from outside), `confirmation` (the user's yes or no and the action it
   was about).
4. **`act` ties steps to their outcome.** While an action runs, its number is on every record, so the
   model calls of a step loop (plan, look, each step) can be kept or dropped by whether the action worked.
5. **Models are recorded by wrapping them** (`record::recorded`), so the step loops needed no change and a
   `model` record is exactly a prompt and its completion. Warm-up calls are not recorded.
6. **The agent records requests, actions and confirmations** through an executor it wraps for the turn.
7. **A dry run records nothing** (`wiring::build_for(cfg, false)`): it does nothing, so its "it worked"
   would teach nothing true. `alfred eval` builds its own agent and records nothing either.
8. **A size limit, never a silent delete.** At `max_mb` (500) recording stops; `alfred recordings` says so,
   and `alfred recordings clear` deletes the files (asking first). *Rejected:* dropping old days
   automatically (the user's data goes only when they say).
9. **A failed write is dropped**: recording never gets in a request's way.

## What the first recording showed

Tried in a test folder with recording on: a routed request, an answer in words, and "on en.wikipedia.org,
find out who created the Simula language". The third failed after 145 s, and the recording said why, which
led to three fixes in the same change:

- The deciding model left the address out of `url` and wrote it in the task, so the browser started
  blank. An address the user said outright is now used when the model gives none.
- The step loop's `search` used Google only, got its robot check, and searched again and again. It now
  searches another engine when Google answers with its robot check, as spoken answers already did.
- A model call that passed 60 s was reported as "I can't reach the language model". It is now "took too
  long to answer" (`LlmError::TimedOut`).

After them the same request answered "created by Kristen Nygaard and Ole-Johan Dahl" in 87 s, in 6 model
calls of 7 to 18 s each: the step model's speed is the next thing the recordings point at.

## Edge cases

- **The folder can't be written:** the record is dropped and the request goes on.
- **Two processes at once** (the app and `alfred ask`): each appends whole lines to the day's file; their
  `session` differs.
- **Outside text:** a record may hold an email's, a window's or a page's text. It stays in the folder; an
  `action` record marks `outside` so a later use can leave such text out of prompts.

## Files

`crates/alfred-core/src/record.rs` (recorder, model wrapper, summary, clear), `agent.rs` (requests,
actions, confirmations), `apps/alfred/src/wiring.rs` (who records), `cli.rs` (`alfred recordings`).
