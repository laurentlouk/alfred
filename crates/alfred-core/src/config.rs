//! `config.toml` in Alfred's home folder. Every field has a default, so an empty or missing file works and
//! an old file keeps working when fields are added.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub llm: LlmConfig,
    pub voice: VoiceConfig,
    pub ui: UiConfig,
    pub search: SearchConfig,
    pub shortcuts: ShortcutsConfig,
    pub desktop: DesktopConfig,
    pub browser: BrowserConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutsConfig {
    /// Offer the user's Shortcuts as actions.
    pub enabled: bool,
    /// Shortcut names that run without asking even when the model picked them. A shortcut you name
    /// yourself ("run Bonjour") never asks.
    pub trusted: Vec<String>,
}

impl Default for ShortcutsConfig {
    fn default() -> Self {
        Self { enabled: true, trusted: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopConfig {
    /// Operate apps on screen through `agent-desktop` when it is installed.
    pub enabled: bool,
    /// The model that drives on-screen steps. Measured in live Mail runs: qwen3:4b reads an app but loses
    /// its place in multi-step tasks; qwen3:8b plans and follows menu commands.
    pub model: String,
    /// The most steps one request may take.
    pub max_steps: u32,
    /// The `agent-desktop` program: a name looked up on PATH and in Homebrew's folders, or a full path.
    pub command: String,
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self { enabled: true, model: "qwen3:8b".into(), max_steps: 8, command: "agent-desktop".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserConfig {
    /// Do tasks on websites through `agent-browser` when it is installed. The steps are decided by the
    /// model in `[desktop] model`, which needs structured output.
    pub enabled: bool,
    /// The `agent-browser` program: a name looked up on PATH and in Homebrew's folders, or a full path.
    pub command: String,
    /// The browser's own profile folder, so its sign-ins survive between runs. Never one of the user's
    /// Chrome profiles. A leading `~/` is the home folder.
    pub profile: String,
    /// Show the browser window, so the user can watch and step in for a CAPTCHA or a sign-in code.
    pub headed: bool,
    /// Work in the user's own running Chrome, where they are signed in, instead of Alfred's window: in a
    /// tab of Alfred's own, never the user's tabs. Chrome lets Alfred in once remote debugging is on at
    /// `chrome://inspect/#remote-debugging`, and asks the user to allow it. `profile` and `headed` are
    /// then unused.
    pub use_chrome: bool,
    /// The most steps one request may take.
    pub max_steps: u32,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            command: "agent-browser".into(),
            profile: home_dir().join("browser").display().to_string(),
            headed: true,
            use_chrome: false,
            max_steps: 10,
        }
    }
}

impl BrowserConfig {
    /// The profile folder as a path, with `~/` expanded.
    pub fn profile_dir(&self) -> PathBuf {
        let p = self.profile.trim();
        match p.strip_prefix("~/") {
            Some(rest) => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join(rest),
            None if p.is_empty() => home_dir().join("browser"),
            None => PathBuf::from(p),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Ollama's native API (`/api/chat`).
    Ollama,
    /// Any server with an OpenAI-compatible `/v1/chat/completions` (llama-server, LM Studio, mlx-lm).
    #[serde(alias = "openai-compatible", alias = "openai_compatible")]
    OpenAi,
}

/// How the model picks an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolMode {
    /// The model answers with JSON constrained to a schema whose `action` is an enum of the tools. Far more
    /// reliable on sub-1B models: measured 14 of 17 right against 8 of 17 with native tool calls.
    #[default]
    Structured,
    /// The server's own tool calling (`tools` in the request). Fine for larger models.
    Native,
}

/// When the model thinks before answering (a model that can't think just answers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Thinking {
    /// Only for questions that need it: the model flags them while deciding, and clear signs in the words
    /// (a sum, "step by step", "compare") count too. Actions such as opening an app never think.
    #[default]
    Auto,
    /// Never: every answer comes from the quick decision.
    Never,
    /// Every answer in words thinks first.
    Always,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub provider: Provider,
    pub tool_mode: ToolMode,
    pub base_url: String,
    /// A really small model by default, so Alfred runs on any Apple-silicon Mac. `qwen3.5:0.8b` or
    /// `qwen3:4b` follow instructions better.
    pub model: String,
    /// Read from the environment variable named here, never stored in the file.
    pub api_key_env: String,
    pub timeout_secs: u64,
    /// The context window asked of the model, in tokens.
    pub context_tokens: u32,
    /// The most tokens one model reply may use: enough for a decision, never a runaway.
    pub max_reply_tokens: u32,
    /// Earlier turns sent back to the model for context.
    pub history_turns: usize,
    /// When the model thinks before answering in words.
    pub thinking: Thinking,
    /// The most tokens a thinking answer may use, thoughts included.
    pub think_tokens: u32,
    /// Another model on the same server that thinks hard questions through, such as a small one that thinks
    /// fast while a larger `model` decides. Unset, `model` does both.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub think_model: Option<String>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: Provider::Ollama,
            tool_mode: ToolMode::Structured,
            base_url: "http://127.0.0.1:11434".into(),
            model: "qwen3:0.6b".into(),
            api_key_env: "ALFRED_API_KEY".into(),
            timeout_secs: 60,
            context_tokens: 8192,
            max_reply_tokens: 256,
            history_turns: 3,
            thinking: Thinking::Auto,
            think_tokens: 2048,
            think_model: None,
        }
    }
}

impl LlmConfig {
    /// The same server and settings with another model (the one that drives on-screen steps).
    pub fn with_model(&self, model: &str) -> Self {
        Self { model: model.to_string(), ..self.clone() }
    }

    pub fn api_key(&self) -> Option<String> {
        std::env::var(&self.api_key_env).ok().filter(|k| !k.trim().is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    /// Speak replies out loud.
    pub speak: bool,
    /// A Kokoro v1.0 voice: `bm_george` (British, male) suits a butler; `af_heart`, `bf_emma`, `am_michael`…
    pub voice: String,
    pub speed: f32,
    /// Seconds of quiet that end dictation when what was said doesn't sound finished ("open Notes and…");
    /// a finished sentence ends sooner.
    pub silence_secs: f32,
    /// Longest dictation, in seconds.
    pub max_listen_secs: f32,
    /// Keep the microphone open and listen for "Alfred", like "Hey Siri". The microphone indicator stays
    /// on while it does; everything is heard on the Mac and nothing is kept until "Alfred" is said.
    pub wake_word: bool,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self { speak: true, voice: "bm_george".into(), speed: 1.05, silence_secs: 1.2, max_listen_secs: 20.0, wake_word: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// The global shortcut that summons Alfred, in Rune's accelerator syntax.
    pub shortcut: String,
    /// Hide the panel when another app comes forward. Off by default: the panel then waits behind that
    /// app's windows like any app's, and ⌘Tab or the Dock icon brings it back.
    pub hide_on_blur: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { shortcut: "ctrl+shift+space".into(), hide_on_blur: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    /// `google`, `duckduckgo` or `bing`.
    pub engine: String,
    /// A browser app to use, such as "Safari"; empty for the default browser.
    pub browser: String,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self { engine: "google".into(), browser: String::new() }
    }
}

impl Config {
    /// Read `path`, falling back to defaults when it is missing. A malformed file is an error, so a typo
    /// doesn't silently reset everything.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Write the file with every field, so users can see what they can change.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, format!("# Alfred settings. Delete a line to get its default back.\n\n{text}"))
            .map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Alfred's folder: `$ALFRED_HOME`, else `~/Library/Application Support/Alfred` on macOS and
/// `$XDG_DATA_HOME/alfred` (or `~/.local/share/alfred`) elsewhere.
pub fn home_dir() -> PathBuf {
    if let Some(h) = std::env::var_os("ALFRED_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Alfred")
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share")).join("alfred")
    }
}

pub fn config_path() -> PathBuf {
    home_dir().join("config.toml")
}

pub fn models_dir() -> PathBuf {
    home_dir().join("models")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_use_a_tiny_local_model_and_a_butler_voice() {
        let c = Config::default();
        assert_eq!(c.llm.provider, Provider::Ollama);
        assert_eq!(c.llm.model, "qwen3:0.6b");
        assert_eq!(c.voice.voice, "bm_george");
        assert_eq!(c.ui.shortcut, "ctrl+shift+space");
        assert!(c.shortcuts.enabled && c.shortcuts.trusted.is_empty());
        assert_eq!(c.desktop.model, "qwen3:8b");
        assert_eq!(c.llm.with_model("qwen3:4b").base_url, c.llm.base_url);
    }

    #[test]
    fn the_browser_is_on_headed_with_its_own_profile_under_alfreds_folder() {
        let b = Config::default().browser;
        assert!(b.enabled && b.headed);
        assert_eq!((b.command.as_str(), b.max_steps), ("agent-browser", 10));
        assert_eq!(b.profile_dir(), home_dir().join("browser"));
        let c: Config = toml::from_str("[browser]\nprofile = \"~/Browsers/alfred\"\nheaded = false\n").unwrap();
        assert!(!c.browser.headed && c.browser.enabled);
        assert!(c.browser.profile_dir().ends_with("Browsers/alfred") && !c.browser.profile_dir().starts_with("~"));
        let blank = BrowserConfig { profile: " ".into(), ..BrowserConfig::default() };
        assert_eq!(blank.profile_dir(), home_dir().join("browser"));
        assert!(!b.use_chrome, "the user's own Chrome only when they ask");
        let mine: Config = toml::from_str("[browser]\nuse_chrome = true\n").unwrap();
        assert!(mine.browser.use_chrome);
    }

    #[test]
    fn partial_files_keep_other_defaults() {
        let c: Config = toml::from_str("[llm]\nmodel = \"qwen3.5:0.8b\"\n[search]\nengine = \"duckduckgo\"\n").unwrap();
        assert_eq!(c.llm.model, "qwen3.5:0.8b");
        assert_eq!(c.llm.base_url, "http://127.0.0.1:11434");
        assert_eq!(c.search.engine, "duckduckgo");
        assert!(c.voice.speak);
    }

    #[test]
    fn a_thinking_model_is_optional() {
        assert_eq!(Config::default().llm.think_model, None);
        assert!(!toml::to_string(&Config::default()).unwrap().contains("think_model"), "unset, it stays out of the file");
        let c: Config = toml::from_str("[llm]\nmodel = \"gemma4:26b\"\nthink_model = \"gemma4:e4b\"\n").unwrap();
        assert_eq!(c.llm.think_model.as_deref(), Some("gemma4:e4b"));
        let thinker = crate::llm::thinker_from_config(&c.llm).expect("a separate thinker");
        assert!(thinker.describe().contains("gemma4:e4b"), "{}", thinker.describe());
        let same = toml::from_str::<Config>("[llm]\nmodel = \"gemma4:e4b\"\nthink_model = \"gemma4:e4b\"\n").unwrap();
        assert!(crate::llm::thinker_from_config(&same.llm).is_none(), "the same model needs no second one");
    }

    #[test]
    fn provider_accepts_spellings() {
        let c: Config = toml::from_str("[llm]\nprovider = \"openai-compatible\"\n").unwrap();
        assert_eq!(c.llm.provider, Provider::OpenAi);
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("alfred-config-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        let mut c = Config::default();
        c.voice.voice = "af_heart".into();
        c.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), c);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_is_defaults_but_broken_file_is_an_error() {
        assert_eq!(Config::load(Path::new("/nonexistent/alfred/config.toml")).unwrap(), Config::default());
        let dir = std::env::temp_dir().join(format!("alfred-config-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[llm\nmodel = 3").unwrap();
        assert!(Config::load(&path).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
