//! Talking to a local model: Ollama's native API by default, any OpenAI-compatible server otherwise
//! (llama-server, LM Studio, mlx-lm). Both speak tool calling; [`ChatModel`] hides the difference.

use std::collections::VecDeque;
use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Value, json};

use crate::config::{LlmConfig, Provider};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: Option<String>,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    /// For `Role::Tool`: which tool produced this result.
    pub tool_name: Option<String>,
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self { role, content: content.into(), tool_calls: Vec::new(), tool_name: None, tool_call_id: None }
    }

    pub fn tool_result(call: &ToolCall, content: impl Into<String>) -> Self {
        Self { tool_name: Some(call.name.clone()), tool_call_id: call.id.clone(), ..Self::new(Role::Tool, content) }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelReply {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LlmError {
    /// Nothing answered at the address.
    Unreachable { base: String, detail: String },
    /// The server answered but doesn't have the model.
    ModelMissing { model: String },
    /// Any other error status.
    Http { status: u16, message: String },
    /// The answer wasn't what we expected.
    BadReply(String),
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmError::Unreachable { base, .. } => {
                write!(f, "I can't reach the language model at {base}. Start it with `ollama serve`.")
            }
            LlmError::ModelMissing { model } => write!(f, "The model {model} isn't installed. Run `ollama pull {model}`."),
            LlmError::Http { status, message } => write!(f, "The model server answered {status}: {message}"),
            LlmError::BadReply(m) => write!(f, "The model's answer didn't make sense: {m}"),
        }
    }
}

impl std::error::Error for LlmError {}

/// A chat model that may call tools.
pub trait ChatModel: Send {
    fn chat(&self, messages: &[ChatMessage], tools: &Value) -> Result<ModelReply, LlmError>;
    /// A JSON object that matches `schema`, decoded under the schema's grammar (structured outputs).
    fn chat_json(&self, _messages: &[ChatMessage], _schema: &Value) -> Result<Value, LlmError> {
        Err(LlmError::BadReply("this model server doesn't support structured output; set tool_mode = \"native\"".into()))
    }
    /// An answer in words with the model's thinking on, for a question that needs reasoning. Only the
    /// answer comes back, never the thoughts. A model that can't think answers directly.
    fn reason(&self, messages: &[ChatMessage]) -> Result<ModelReply, LlmError> {
        self.chat(messages, &Value::Array(Vec::new()))
    }
    /// "gemma4:e4b via Ollama", for logs and the UI footer.
    fn describe(&self) -> String;
}

/// A shared model is still a model (tests keep a handle to inspect what it was asked).
impl<T: ChatModel + Sync> ChatModel for std::sync::Arc<T> {
    fn chat(&self, messages: &[ChatMessage], tools: &Value) -> Result<ModelReply, LlmError> {
        (**self).chat(messages, tools)
    }

    fn chat_json(&self, messages: &[ChatMessage], schema: &Value) -> Result<Value, LlmError> {
        (**self).chat_json(messages, schema)
    }

    fn reason(&self, messages: &[ChatMessage]) -> Result<ModelReply, LlmError> {
        (**self).reason(messages)
    }

    fn describe(&self) -> String {
        (**self).describe()
    }
}

/// The model the config asks for.
pub fn from_config(cfg: &LlmConfig) -> Box<dyn ChatModel> {
    match cfg.provider {
        Provider::Ollama => Box::new(
            Ollama::new(&cfg.base_url, &cfg.model, cfg.timeout_secs)
                .with_context(cfg.context_tokens)
                .with_max_tokens(cfg.max_reply_tokens)
                .with_think_tokens(cfg.think_tokens),
        ),
        Provider::OpenAi => Box::new(OpenAiCompat::new(&cfg.base_url, &cfg.model, cfg.api_key(), cfg.timeout_secs)),
    }
}

/// The model that thinks hard questions through, when `think_model` names one other than `model`.
pub fn thinker_from_config(cfg: &LlmConfig) -> Option<Box<dyn ChatModel>> {
    let name = cfg.think_model.as_deref().map(str::trim).filter(|m| !m.is_empty() && *m != cfg.model)?;
    Some(from_config(&cfg.with_model(name)))
}

fn agent(timeout_secs: u64) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(timeout_secs.max(1)))).http_status_as_error(false).build().into()
}

fn post(agent: &ureq::Agent, url: &str, base: &str, key: Option<&str>, body: &Value) -> Result<(u16, Value), LlmError> {
    let mut req = agent.post(url);
    if let Some(k) = key {
        req = req.header("Authorization", &format!("Bearer {k}"));
    }
    let mut resp = req.send_json(body).map_err(|e| LlmError::Unreachable { base: base.to_string(), detail: e.to_string() })?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().map_err(|e| LlmError::BadReply(e.to_string()))?;
    let value = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
    Ok((status, value))
}

fn error_message(v: &Value) -> String {
    v.get("error")
        .map(|e| {
            e.get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| e.as_str().map(str::to_string).unwrap_or(e.to_string()))
        })
        .unwrap_or_else(|| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))
}

/// The object in a structured answer (tolerating stray text or a code fence around it).
fn json_object(content: &str) -> Result<Value, LlmError> {
    match crate::tools::parse_arguments(&Value::String(strip_think(content))) {
        Value::Object(m) if !m.is_empty() => Ok(Value::Object(m)),
        _ => Err(LlmError::BadReply(format!("expected a JSON object, got {:?}", content.chars().take(80).collect::<String>()))),
    }
}

/// Tokens a reply may use: enough for a spoken sentence or a tool call, never a runaway loop. (A 0.6B model
/// under a JSON grammar once generated 27,000 tokens of padding before this cap existed.)
const MAX_REPLY_TOKENS: u32 = 256;

/// Remove `<think>…</think>` blocks some models still emit.
pub fn strip_think(s: &str) -> String {
    let mut out = s.to_string();
    while let Some(a) = out.find("<think>") {
        match out[a..].find("</think>") {
            Some(b) => out.replace_range(a..a + b + "</think>".len(), ""),
            None => out.truncate(a),
        }
    }
    out.trim().to_string()
}

// ---- Ollama ---------------------------------------------------------------------------------------

pub struct Ollama {
    base: String,
    model: String,
    agent: ureq::Agent,
    /// Ollama refuses `think` for models that can't think; remember that after the first refusal.
    send_think: Mutex<bool>,
    /// The context window to ask for. Too small and a long prompt pushes out the instructions at its start.
    context: u32,
    /// The most tokens one reply may use.
    max_tokens: u32,
    /// The most tokens a thinking answer may use, thoughts included.
    think_tokens: u32,
    timeout_secs: u64,
}

impl Ollama {
    pub fn new(base: &str, model: &str, timeout_secs: u64) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            model: model.to_string(),
            agent: agent(timeout_secs),
            send_think: Mutex::new(true),
            context: 8192,
            max_tokens: MAX_REPLY_TOKENS,
            think_tokens: 2048,
            timeout_secs,
        }
    }

    pub fn with_think_tokens(mut self, tokens: u32) -> Self {
        self.think_tokens = tokens.clamp(256, 16_384);
        self
    }

    pub fn with_max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = tokens.clamp(32, 4096);
        self
    }

    pub fn with_context(mut self, tokens: u32) -> Self {
        self.context = tokens.clamp(2048, 131_072);
        self
    }

    fn message_json(m: &ChatMessage) -> Value {
        let mut v = json!({ "role": m.role.as_str(), "content": m.content });
        if !m.tool_calls.is_empty() {
            v["tool_calls"] = m.tool_calls.iter().map(|c| json!({ "function": { "name": c.name, "arguments": c.arguments } })).collect();
        }
        if let Some(name) = &m.tool_name {
            v["tool_name"] = json!(name);
        }
        v
    }

    /// The installed model names, for `alfred doctor`.
    pub fn installed(&self) -> Result<Vec<String>, LlmError> {
        let url = format!("{}/api/tags", self.base);
        let mut resp = self.agent.get(&url).call().map_err(|e| LlmError::Unreachable { base: self.base.clone(), detail: e.to_string() })?;
        let v: Value = resp.body_mut().read_json().map_err(|e| LlmError::BadReply(e.to_string()))?;
        Ok(v["models"].as_array().map(|a| a.iter().filter_map(|m| m["name"].as_str().map(str::to_string)).collect()).unwrap_or_default())
    }
}

impl ChatModel for Ollama {
    fn chat(&self, messages: &[ChatMessage], tools: &Value) -> Result<ModelReply, LlmError> {
        let mut body = json!({
            "model": self.model,
            "messages": messages.iter().map(Self::message_json).collect::<Vec<_>>(),
            "stream": false,
            "keep_alive": "30m",
            "options": { "temperature": 0.1, "num_ctx": self.context, "num_predict": self.max_tokens },
        });
        if tools.as_array().is_some_and(|a| !a.is_empty()) {
            body["tools"] = tools.clone();
        }
        self.send(body).map(|v| Self::reply_from(&v))
    }

    fn chat_json(&self, messages: &[ChatMessage], schema: &Value) -> Result<Value, LlmError> {
        let body = json!({
            "model": self.model,
            "messages": messages.iter().map(Self::message_json).collect::<Vec<_>>(),
            "stream": false,
            "keep_alive": "30m",
            "format": schema,
            "options": { "temperature": 0.0, "num_ctx": self.context, "num_predict": self.max_tokens },
        });
        let v = self.send(body)?;
        json_object(v["message"]["content"].as_str().unwrap_or_default())
    }

    /// Thinking on, with room for the thoughts: a larger token budget and a longer wait. Ollama returns the
    /// thoughts apart (`message.thinking`), so only the answer is kept.
    fn reason(&self, messages: &[ChatMessage]) -> Result<ModelReply, LlmError> {
        let body = json!({
            "model": self.model,
            "messages": messages.iter().map(Self::message_json).collect::<Vec<_>>(),
            "stream": false,
            "keep_alive": "30m",
            "options": { "temperature": 0.3, "num_ctx": self.context, "num_predict": self.think_tokens },
        });
        let patient = agent(self.timeout_secs.max(180));
        self.send_on(&patient, body, true).map(|v| Self::reply_from(&v))
    }

    fn describe(&self) -> String {
        format!("{} via Ollama", self.model)
    }
}

impl Ollama {
    /// POST to `/api/chat` with thinking off (decisions and quick answers).
    fn send(&self, body: Value) -> Result<Value, LlmError> {
        self.send_on(&self.agent, body, false)
    }

    /// POST to `/api/chat` with `think` set as asked, dropping it for models that refuse it, and map error
    /// statuses.
    fn send_on(&self, agent: &ureq::Agent, mut body: Value, thinking: bool) -> Result<Value, LlmError> {
        let url = format!("{}/api/chat", self.base);
        let think = self.send_think.lock().map(|g| *g).unwrap_or(false);
        if think {
            body["think"] = json!(thinking);
        }
        let (mut status, mut v) = post(agent, &url, &self.base, None, &body)?;
        if status == 400 && think && error_message(&v).contains("think") {
            if let Ok(mut g) = self.send_think.lock() {
                *g = false;
            }
            body.as_object_mut().map(|o| o.remove("think"));
            (status, v) = post(agent, &url, &self.base, None, &body)?;
        }
        if status == 404 {
            return Err(LlmError::ModelMissing { model: self.model.clone() });
        }
        if status >= 400 {
            return Err(LlmError::Http { status, message: error_message(&v) });
        }
        Ok(v)
    }

    fn reply_from(v: &Value) -> ModelReply {
        let msg = &v["message"];
        let tool_calls: Vec<ToolCall> = msg["tool_calls"]
            .as_array()
            .map(|calls| {
                calls
                    .iter()
                    .filter_map(|c| {
                        let f = &c["function"];
                        Some(ToolCall {
                            id: c["id"].as_str().map(str::to_string),
                            name: f["name"].as_str()?.to_string(),
                            arguments: f["arguments"].clone(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let content = strip_think(msg["content"].as_str().unwrap_or_default());
        // Gemma's own call syntax sometimes reaches the content unparsed: read it here.
        if tool_calls.is_empty()
            && let Some(call) = gemma_call(&content)
        {
            return ModelReply { content: String::new(), tool_calls: vec![call] };
        }
        ModelReply { content, tool_calls }
    }
}

/// A tool call written in Gemma's syntax, as Ollama sometimes leaves it in the content:
/// `open_app{name:<|"|>Calendar<|"|>}` (or with `call:` in front). Strings sit between `<|"|>` marks;
/// numbers and booleans are bare.
pub fn gemma_call(content: &str) -> Option<ToolCall> {
    const Q: &str = "<|\"|>";
    let c = content.trim().trim_start_matches("<|tool_call>").trim_end_matches("<tool_call|>").trim();
    let c = c.strip_prefix("call:").unwrap_or(c).trim();
    let open = c.find('{')?;
    let name = c[..open].trim();
    if name.is_empty() || !name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') || !c.ends_with('}') {
        return None;
    }
    let mut rest = &c[open + 1..c.len() - 1];
    let mut args = serde_json::Map::new();
    loop {
        rest = rest.trim_start_matches([',', ' ', '\n']);
        if rest.is_empty() {
            break;
        }
        let colon = rest.find(':')?;
        let key = rest[..colon].trim().to_string();
        rest = &rest[colon + 1..];
        let value = if let Some(after) = rest.strip_prefix(Q) {
            let end = after.find(Q)?;
            let v = Value::String(after[..end].to_string());
            rest = &after[end + Q.len()..];
            v
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            let raw = rest[..end].trim();
            rest = &rest[end..];
            serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
        };
        args.insert(key, value);
    }
    Some(ToolCall { id: None, name: name.to_string(), arguments: Value::Object(args) })
}

// ---- OpenAI-compatible ----------------------------------------------------------------------------

pub struct OpenAiCompat {
    base: String,
    model: String,
    key: Option<String>,
    agent: ureq::Agent,
}

impl OpenAiCompat {
    pub fn new(base: &str, model: &str, key: Option<String>, timeout_secs: u64) -> Self {
        let base = base.trim_end_matches('/');
        let base = if base.ends_with("/v1") { base.to_string() } else { format!("{base}/v1") };
        Self { base, model: model.to_string(), key, agent: agent(timeout_secs) }
    }

    fn message_json(m: &ChatMessage) -> Value {
        let mut v = json!({ "role": m.role.as_str(), "content": m.content });
        if !m.tool_calls.is_empty() {
            v["tool_calls"] = m
                .tool_calls
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    json!({
                        "id": c.id.clone().unwrap_or_else(|| format!("call_{i}")),
                        "type": "function",
                        "function": { "name": c.name, "arguments": c.arguments.to_string() },
                    })
                })
                .collect();
        }
        if m.role == Role::Tool {
            v["tool_call_id"] = json!(m.tool_call_id.clone().unwrap_or_else(|| "call_0".into()));
        }
        v
    }
}

impl ChatModel for OpenAiCompat {
    fn chat(&self, messages: &[ChatMessage], tools: &Value) -> Result<ModelReply, LlmError> {
        let url = format!("{}/chat/completions", self.base);
        let mut body = json!({
            "model": self.model,
            "messages": messages.iter().map(Self::message_json).collect::<Vec<_>>(),
            "temperature": 0.1,
            "max_tokens": MAX_REPLY_TOKENS,
            "stream": false,
        });
        if tools.as_array().is_some_and(|a| !a.is_empty()) {
            body["tools"] = tools.clone();
        }
        let (status, v) = post(&self.agent, &url, &self.base, self.key.as_deref(), &body)?;
        if status == 404 && error_message(&v).to_lowercase().contains("model") {
            return Err(LlmError::ModelMissing { model: self.model.clone() });
        }
        if status >= 400 {
            return Err(LlmError::Http { status, message: error_message(&v) });
        }
        let msg = &v["choices"][0]["message"];
        if msg.is_null() {
            return Err(LlmError::BadReply("no choices in the answer".into()));
        }
        let tool_calls = msg["tool_calls"]
            .as_array()
            .map(|calls| {
                calls
                    .iter()
                    .filter_map(|c| {
                        let f = &c["function"];
                        Some(ToolCall {
                            id: c["id"].as_str().map(str::to_string),
                            name: f["name"].as_str()?.to_string(),
                            arguments: f["arguments"].clone(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(ModelReply { content: strip_think(msg["content"].as_str().unwrap_or_default()), tool_calls })
    }

    fn chat_json(&self, messages: &[ChatMessage], schema: &Value) -> Result<Value, LlmError> {
        let url = format!("{}/chat/completions", self.base);
        let body = json!({
            "model": self.model,
            "messages": messages.iter().map(Self::message_json).collect::<Vec<_>>(),
            "temperature": 0.0,
            "max_tokens": MAX_REPLY_TOKENS,
            "stream": false,
            "response_format": { "type": "json_schema", "json_schema": { "name": "decision", "strict": true, "schema": schema } },
        });
        let (status, v) = post(&self.agent, &url, &self.base, self.key.as_deref(), &body)?;
        if status >= 400 {
            return Err(LlmError::Http { status, message: error_message(&v) });
        }
        json_object(v["choices"][0]["message"]["content"].as_str().unwrap_or_default())
    }

    fn describe(&self) -> String {
        format!("{} via {}", self.model, self.base)
    }
}

#[cfg(test)]
mod gemma_tests {
    use super::*;

    #[test]
    fn gemma_calls_left_in_the_content_are_read() {
        let c = gemma_call("open_app{name:<|\"|>Calendar<|\"|>}").unwrap();
        assert_eq!((c.name.as_str(), &c.arguments), ("open_app", &json!({"name": "Calendar"})));
        let c = gemma_call("call:use_app{app:<|\"|>Slack<|\"|>,text:<|\"|>post hi, in general<|\"|>}").unwrap();
        assert_eq!(c.arguments, json!({"app": "Slack", "text": "post hi, in general"}));
        let c = gemma_call("create_reminder{title:<|\"|>Stretch<|\"|>,minutes:20}").unwrap();
        assert_eq!(c.arguments, json!({"title": "Stretch", "minutes": 20}));
        assert_eq!(gemma_call("I will open the Music application for you."), None);
        assert_eq!(gemma_call("Sure {not a call}"), None);
    }
}

// ---- A scripted model, for tests and offline demos ------------------------------------------------

/// Answers with the replies it was given, in order, and records what it was asked.
#[derive(Default)]
pub struct Scripted {
    replies: Mutex<VecDeque<Result<ModelReply, LlmError>>>,
    pub seen: Mutex<Vec<Vec<ChatMessage>>>,
    /// The tools or schema passed with each call.
    pub schemas: Mutex<Vec<Value>>,
}

impl Scripted {
    pub fn new(replies: impl IntoIterator<Item = Result<ModelReply, LlmError>>) -> Self {
        Self { replies: Mutex::new(replies.into_iter().collect()), seen: Mutex::default(), schemas: Mutex::default() }
    }

    pub fn says(text: &str) -> Result<ModelReply, LlmError> {
        Ok(ModelReply { content: text.into(), tool_calls: vec![] })
    }

    pub fn calls(name: &str, arguments: Value) -> Result<ModelReply, LlmError> {
        Ok(ModelReply { content: String::new(), tool_calls: vec![ToolCall { id: None, name: name.into(), arguments }] })
    }
}

impl ChatModel for Scripted {
    fn chat(&self, messages: &[ChatMessage], tools: &Value) -> Result<ModelReply, LlmError> {
        if let Ok(mut s) = self.seen.lock() {
            s.push(messages.to_vec());
        }
        if let Ok(mut s) = self.schemas.lock() {
            s.push(tools.clone());
        }
        self.replies.lock().ok().and_then(|mut r| r.pop_front()).unwrap_or_else(|| Scripted::says(""))
    }

    /// The same script, shaped as structured decisions: a tool call becomes
    /// `{"action": name, ...arguments}`, words become `{"action": "chat", "reply": …}`.
    fn chat_json(&self, messages: &[ChatMessage], tools: &Value) -> Result<Value, LlmError> {
        let reply = self.chat(messages, tools)?;
        Ok(match reply.tool_calls.first() {
            Some(call) => {
                let mut v = crate::tools::parse_arguments(&call.arguments);
                v["action"] = json!(call.name);
                v
            }
            None => json!({ "action": "chat", "reply": reply.content }),
        })
    }

    /// Records the call as `"reason"` in `schemas`, so tests can tell a thinking answer from a quick one.
    fn reason(&self, messages: &[ChatMessage]) -> Result<ModelReply, LlmError> {
        self.chat(messages, &json!("reason"))
    }

    fn describe(&self) -> String {
        "scripted model".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_think_blocks() {
        assert_eq!(strip_think("<think>hmm</think>Hello"), "Hello");
        assert_eq!(strip_think("Hi <think>a</think>there<think>b</think>"), "Hi there");
        assert_eq!(strip_think("Hello <think>never closed"), "Hello");
    }

    #[test]
    fn ollama_messages_carry_tool_calls_and_names() {
        let call = ToolCall { id: Some("c1".into()), name: "open_app".into(), arguments: json!({"name": "Notes"}) };
        let asst = ChatMessage { tool_calls: vec![call.clone()], ..ChatMessage::new(Role::Assistant, "") };
        let v = Ollama::message_json(&asst);
        assert_eq!(v["tool_calls"][0]["function"]["arguments"]["name"], "Notes");
        let tool = Ollama::message_json(&ChatMessage::tool_result(&call, "Notes is open."));
        assert_eq!(tool["role"], "tool");
        assert_eq!(tool["tool_name"], "open_app");
    }

    #[test]
    fn openai_messages_stringify_arguments_and_link_ids() {
        let call = ToolCall { id: Some("c1".into()), name: "open_app".into(), arguments: json!({"name": "Notes"}) };
        let asst = ChatMessage { tool_calls: vec![call.clone()], ..ChatMessage::new(Role::Assistant, "") };
        let v = OpenAiCompat::message_json(&asst);
        assert_eq!(v["tool_calls"][0]["function"]["arguments"], "{\"name\":\"Notes\"}");
        assert_eq!(OpenAiCompat::message_json(&ChatMessage::tool_result(&call, "ok"))["tool_call_id"], "c1");
    }

    #[test]
    fn openai_base_gets_v1_once() {
        assert_eq!(OpenAiCompat::new("http://x:8080", "m", None, 5).base, "http://x:8080/v1");
        assert_eq!(OpenAiCompat::new("http://x:8080/v1/", "m", None, 5).base, "http://x:8080/v1");
    }

    #[test]
    fn unreachable_server_is_a_clear_error() {
        // Port 9 (discard) is closed on a normal machine; the error must name the fix.
        let m = Ollama::new("http://127.0.0.1:9", "qwen3:0.6b", 2);
        let e = m.chat(&[ChatMessage::new(Role::User, "hi")], &json!([])).unwrap_err();
        assert!(matches!(e, LlmError::Unreachable { .. }), "{e:?}");
        assert!(e.to_string().contains("ollama serve"));
    }

    #[test]
    fn scripted_replies_in_order() {
        let m = Scripted::new([Scripted::says("one"), Scripted::calls("get_time", json!({}))]);
        assert_eq!(m.chat(&[], &json!([])).unwrap().content, "one");
        assert_eq!(m.chat(&[], &json!([])).unwrap().tool_calls[0].name, "get_time");
        assert_eq!(m.chat(&[], &json!([])).unwrap().content, "");
    }
}
