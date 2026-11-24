//! Recordings: what Alfred was asked, what its models saw and answered, and what each action did, kept as
//! JSON lines on this Mac only. Off unless the user turns it on (`[recording] enabled`). They are the raw
//! material for teaching Alfred from its own runs: recipes, examples in the prompt, and fine-tuning.
//!
//! One file per day (UTC), `YYYY-MM-DD.jsonl`, in a folder only the user can read. Every line is one
//! record:
//!
//! ```text
//! {"v":1,"at":1795478400,"session":1795470000123,"turn":3,"kind":"request","text":"…","source":"model",…}
//! {"v":1,…,"turn":3,"act":7,"kind":"model","role":"app","call":"json","messages":[…],"schema":{…},"reply":{…}}
//! {"v":1,…,"turn":3,"act":7,"kind":"action","action":{"tool":"use_app",…},"ok":true,"said":"…"}
//! ```
//!
//! `turn` ties a request to its model calls and actions; `act` ties the model calls made while an action
//! ran (the step loops in an app or on a page) to that action and its outcome. A `model` record holds
//! exactly what the model was sent and what it answered, which is the shape fine-tuning takes. Nothing here
//! is sent anywhere, and recording stops when the folder reaches its size limit.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::llm::{ChatMessage, ChatModel, LlmError, ModelReply};

/// The record format's version, in every line as `v`.
pub const VERSION: u32 = 1;

enum Sink {
    Dir { dir: PathBuf, bytes: u64, max_bytes: u64 },
    Memory(Vec<Value>),
}

/// Where records go. Shared by the agent, its models and the step loops' models.
pub struct Recorder {
    sink: Mutex<Sink>,
    /// When this recorder started, in unix milliseconds: with `turn`, it names a request for good (two
    /// `alfred ask` runs in the same second are still told apart).
    session: u64,
    turn: AtomicU64,
    /// The action running now (0 when none), and how many have run.
    act: AtomicU64,
    acts: AtomicU64,
}

impl Recorder {
    /// Record into `dir`, until its recordings reach `max_mb`.
    pub fn to_dir(dir: &Path, max_mb: u64) -> Arc<Recorder> {
        let sink = Sink::Dir { dir: dir.to_path_buf(), bytes: folder_bytes(dir), max_bytes: max_mb.max(1) * 1_000_000 };
        Arc::new(Self::with(sink))
    }

    /// Record in memory, for tests.
    pub fn in_memory() -> Arc<Recorder> {
        Arc::new(Self::with(Sink::Memory(Vec::new())))
    }

    fn with(sink: Sink) -> Recorder {
        let session = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        Recorder { sink: Mutex::new(sink), session, turn: AtomicU64::new(0), act: AtomicU64::new(0), acts: AtomicU64::new(0) }
    }

    /// A new request begins: what is recorded from now on belongs to it.
    pub fn next_turn(&self) -> u64 {
        self.act.store(0, Ordering::SeqCst);
        self.turn.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// An action starts running: model calls recorded until [`Recorder::end_action`] belong to it.
    pub fn start_action(&self) -> u64 {
        let id = self.acts.fetch_add(1, Ordering::SeqCst) + 1;
        self.act.store(id, Ordering::SeqCst);
        id
    }

    pub fn end_action(&self) {
        self.act.store(0, Ordering::SeqCst);
    }

    /// Write one record of `kind` with `fields` (a JSON object), stamped with the time, the request and
    /// the action it belongs to. A failed write is dropped: recording never gets in a request's way.
    pub fn record(&self, kind: &str, fields: Value) {
        let mut line = serde_json::Map::new();
        line.insert("v".into(), json!(VERSION));
        line.insert("at".into(), json!(now()));
        line.insert("session".into(), json!(self.session));
        line.insert("turn".into(), json!(self.turn.load(Ordering::SeqCst)));
        let act = self.act.load(Ordering::SeqCst);
        if act != 0 {
            line.insert("act".into(), json!(act));
        }
        line.insert("kind".into(), json!(kind));
        if let Value::Object(extra) = fields {
            line.extend(extra);
        }
        let line = Value::Object(line);
        let Ok(mut sink) = self.sink.lock() else { return };
        match &mut *sink {
            Sink::Memory(all) => all.push(line),
            Sink::Dir { dir, bytes, max_bytes } => {
                let text = format!("{line}\n");
                if *bytes + text.len() as u64 <= *max_bytes && append(dir, &text).is_ok() {
                    *bytes += text.len() as u64;
                }
            }
        }
    }

    /// Everything recorded in memory, for tests.
    pub fn entries(&self) -> Vec<Value> {
        match self.sink.lock().as_deref() {
            Ok(Sink::Memory(all)) => all.clone(),
            _ => Vec::new(),
        }
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The UTC day of a unix time, as `YYYY-MM-DD`.
pub fn utc_day(secs: u64) -> String {
    // Days since 1970-01-01 to a civil date (the proleptic Gregorian calendar).
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Add `text` to today's file in `dir`, creating both for the user's eyes only.
fn append(dir: &Path, text: &str) -> std::io::Result<()> {
    let mut make = std::fs::DirBuilder::new();
    make.recursive(true);
    let mut open = std::fs::OpenOptions::new();
    open.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        make.mode(0o700);
        open.mode(0o600);
    }
    make.create(dir)?;
    open.open(dir.join(format!("{}.jsonl", utc_day(now()))))?.write_all(text.as_bytes())
}

fn recording_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "jsonl")).collect())
        .unwrap_or_default();
    files.sort();
    files
}

fn folder_bytes(dir: &Path) -> u64 {
    recording_files(dir).iter().filter_map(|p| p.metadata().ok()).map(|m| m.len()).sum()
}

/// What a recordings folder holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub files: usize,
    pub bytes: u64,
    /// Records by kind: `request`, `model`, `action`, `confirmation`.
    pub kinds: BTreeMap<String, usize>,
    /// Actions that worked, and that failed.
    pub worked: usize,
    pub failed: usize,
    pub first_day: Option<String>,
    pub last_day: Option<String>,
}

/// Count what is in `dir`.
pub fn summary(dir: &Path) -> Summary {
    let files = recording_files(dir);
    let day = |p: &PathBuf| p.file_stem().map(|s| s.to_string_lossy().into_owned());
    let mut s = Summary {
        files: files.len(),
        bytes: folder_bytes(dir),
        first_day: files.first().and_then(day),
        last_day: files.last().and_then(day),
        ..Summary::default()
    };
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        for record in text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
            let kind = record["kind"].as_str().unwrap_or("unknown").to_string();
            if kind == "action" {
                match record["ok"].as_bool() {
                    Some(true) => s.worked += 1,
                    Some(false) => s.failed += 1,
                    None => {}
                }
            }
            *s.kinds.entry(kind).or_default() += 1;
        }
    }
    s
}

/// Delete every recording in `dir` (its `.jsonl` files, nothing else); returns how many files went.
pub fn clear(dir: &Path) -> std::io::Result<usize> {
    let files = recording_files(dir);
    for f in &files {
        std::fs::remove_file(f)?;
    }
    Ok(files.len())
}

/// A model whose every call is recorded: what it was sent, what it answered, how long it took.
struct Recorded {
    model: Box<dyn ChatModel>,
    /// What the model is used for: `decide`, `think`, `app` (steps in an app), `web` (steps on a page).
    role: &'static str,
    recorder: Arc<Recorder>,
}

/// `model`, with every call recorded under `role`.
pub fn recorded(model: Box<dyn ChatModel>, role: &'static str, recorder: &Arc<Recorder>) -> Box<dyn ChatModel> {
    Box::new(Recorded { model, role, recorder: recorder.clone() })
}

fn message_json(m: &ChatMessage) -> Value {
    let mut v = json!({ "role": m.role.as_str(), "content": m.content });
    if !m.tool_calls.is_empty() {
        v["tool_calls"] = m.tool_calls.iter().map(|c| json!({ "name": c.name, "arguments": c.arguments })).collect();
    }
    if let Some(name) = &m.tool_name {
        v["tool_name"] = json!(name);
    }
    v
}

fn reply_json(r: &ModelReply) -> Value {
    json!({
        "content": r.content,
        "tool_calls": r.tool_calls.iter().map(|c| json!({ "name": c.name, "arguments": c.arguments })).collect::<Vec<_>>(),
    })
}

impl Recorded {
    fn note(&self, call: &str, messages: &[ChatMessage], offered: (&str, &Value), answer: Result<Value, &LlmError>, started: Instant) {
        let mut fields = json!({
            "role": self.role,
            "model": self.model.describe(),
            "call": call,
            "messages": messages.iter().map(message_json).collect::<Vec<_>>(),
            "millis": started.elapsed().as_millis() as u64,
        });
        if !offered.1.is_null() {
            fields[offered.0] = offered.1.clone();
        }
        match answer {
            Ok(reply) => fields["reply"] = reply,
            Err(e) => fields["error"] = json!(e.to_string()),
        }
        self.recorder.record("model", fields);
    }
}

impl ChatModel for Recorded {
    fn chat(&self, messages: &[ChatMessage], tools: &Value) -> Result<ModelReply, LlmError> {
        let started = Instant::now();
        let out = self.model.chat(messages, tools);
        self.note("chat", messages, ("tools", tools), out.as_ref().map(reply_json), started);
        out
    }

    fn chat_json(&self, messages: &[ChatMessage], schema: &Value) -> Result<Value, LlmError> {
        let started = Instant::now();
        let out = self.model.chat_json(messages, schema);
        self.note("json", messages, ("schema", schema), out.as_ref().cloned(), started);
        out
    }

    fn reason(&self, messages: &[ChatMessage]) -> Result<ModelReply, LlmError> {
        let started = Instant::now();
        let out = self.model.reason(messages);
        self.note("reason", messages, ("tools", &Value::Null), out.as_ref().map(reply_json), started);
        out
    }

    fn describe(&self) -> String {
        self.model.describe()
    }

    /// Warming the model is no decision: nothing to record.
    fn warm(&self, messages: &[ChatMessage], tools: &Value, schema: Option<&Value>) {
        self.model.warm(messages, tools, schema);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{Role, Scripted};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("alfred-recordings-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        dir
    }

    #[test]
    fn days_are_utc_calendar_days() {
        assert_eq!(utc_day(0), "1970-01-01");
        assert_eq!(utc_day(951_782_400), "2000-02-29", "a leap day");
        assert_eq!(utc_day(1_795_478_400 + 86_399), "2026-11-24", "the last second of the day");
        assert_eq!(utc_day(1_798_761_600), "2027-01-01");
    }

    #[test]
    fn records_carry_their_request_and_the_action_they_ran_in() {
        let r = Recorder::in_memory();
        assert_eq!(r.next_turn(), 1);
        r.record("model", json!({"role": "decide"}));
        let act = r.start_action();
        r.record("model", json!({"role": "app"}));
        r.record("action", json!({"ok": true}));
        r.end_action();
        r.next_turn();
        r.record("request", json!({"text": "hello"}));
        let all = r.entries();
        assert_eq!((all[0]["turn"].as_u64(), all[0].get("act")), (Some(1), None), "deciding happens before any action");
        assert_eq!(all[1]["act"].as_u64(), Some(act), "a step's model call belongs to its action");
        assert_eq!(all[2]["act"].as_u64(), Some(act));
        assert_eq!((all[3]["turn"].as_u64(), all[3].get("act")), (Some(2), None));
        assert!(all.iter().all(|e| e["v"] == VERSION && e["at"].as_u64().is_some() && e["session"].as_u64().is_some()));
        assert_eq!(all[3]["text"], "hello");
    }

    #[test]
    fn a_folder_gets_one_private_file_a_day_and_stops_at_its_limit() {
        let dir = temp_dir("files");
        let r = Recorder::to_dir(&dir, 1);
        r.next_turn();
        r.record("request", json!({"text": "open notes"}));
        r.record("action", json!({"ok": true}));
        r.record("action", json!({"ok": false}));
        let file = dir.join(format!("{}.jsonl", utc_day(now())));
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text.lines().count(), 3);
        assert!(text.lines().all(|l| serde_json::from_str::<Value>(l).is_ok()), "every line is a JSON record");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600, "only the user can read it");
            assert_eq!(dir.metadata().unwrap().permissions().mode() & 0o777, 0o700);
        }
        let s = summary(&dir);
        assert_eq!((s.files, s.worked, s.failed), (1, 1, 1));
        assert_eq!(s.kinds.get("request"), Some(&1));
        assert_eq!(s.first_day, s.last_day);

        // The limit: 1 MB. A record that would pass it is dropped, and so is everything after.
        let big = "x".repeat(600_000);
        r.record("model", json!({"view": big}));
        r.record("model", json!({"view": big}));
        assert_eq!(std::fs::read_to_string(&file).unwrap().lines().count(), 4, "the second big record didn't fit");

        // Another recorder on the same folder starts from what is already there.
        Recorder::to_dir(&dir, 1).record("model", json!({"view": big}));
        assert_eq!(std::fs::read_to_string(&file).unwrap().lines().count(), 4);

        std::fs::write(dir.join("notes.txt"), "mine").unwrap();
        assert_eq!(clear(&dir).unwrap(), 1);
        assert!(dir.join("notes.txt").exists(), "only recordings are deleted");
        assert_eq!(summary(&dir), Summary::default());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_recorded_model_keeps_what_it_was_sent_and_what_it_answered() {
        let r = Recorder::in_memory();
        let script = Scripted::new([Scripted::calls("open_app", json!({"name": "Notes"})), Scripted::calls("click", json!({"ref": "e3"}))]);
        let model = recorded(Box::new(script), "decide", &r);
        let messages = [ChatMessage::new(Role::System, "You are Alfred."), ChatMessage::new(Role::User, "open notes")];
        model.warm(&messages, &Value::Null, None);
        model.chat(&messages, &json!([{"function": {"name": "open_app"}}])).unwrap();
        model.chat_json(&messages, &json!({"type": "object"})).unwrap();
        let all = r.entries();
        assert_eq!(all.len(), 2, "warming isn't recorded");
        assert_eq!(
            (all[0]["kind"].as_str(), all[0]["role"].as_str(), all[0]["call"].as_str()),
            (Some("model"), Some("decide"), Some("chat"))
        );
        assert_eq!(all[0]["messages"][1], json!({"role": "user", "content": "open notes"}));
        assert_eq!(all[0]["tools"][0]["function"]["name"], "open_app");
        assert_eq!(all[0]["reply"]["tool_calls"][0], json!({"name": "open_app", "arguments": {"name": "Notes"}}));
        assert_eq!(all[1]["call"], "json");
        assert_eq!(all[1]["schema"], json!({"type": "object"}));
        assert_eq!(all[1]["reply"], json!({"action": "click", "ref": "e3"}));
        assert!(all[1]["millis"].as_u64().is_some());
    }
}
