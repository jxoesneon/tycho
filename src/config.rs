//! Tycho central configuration schema and loader.

use crate::generation::AssistantPersona;
use crate::models::hub::HuggingFaceModelSpec;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub buffer_size: usize,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16000,
            channels: 1,
            buffer_size: 512,
            input_device: None,
            output_device: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VadConfig {
    pub energy_threshold: f32,
    pub min_speech_duration_ms: u64,
    pub min_silence_duration_ms: u64,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            energy_threshold: 0.02,
            min_speech_duration_ms: 250,
            min_silence_duration_ms: 600,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SttConfig {
    pub engine: String,
    pub model_path: Option<PathBuf>,
    pub language: String,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            engine: "whisper".to_string(),
            model_path: None,
            language: "en".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RouterConfig {
    pub fast_path_confidence_threshold: f32,
    pub fallback_to_laya: bool,
    pub jev_api_key: Option<String>,
    pub jev_endpoint: String,
    pub jev_model: String,
    pub laya_hf_repo: String,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            fast_path_confidence_threshold: 0.82,
            fallback_to_laya: true,
            jev_api_key: None,
            jev_endpoint: "http://127.0.0.1:11434/v1/chat/completions".to_string(),
            jev_model: "llama3.2:3b".to_string(),
            laya_hf_repo: "convaiinnovations/laya".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopConfig {
    pub backend: String,
    pub auto_detect: bool,
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            backend: "auto".to_string(),
            auto_detect: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GenerationConfig {
    pub model: String,
    pub endpoint: String,
    pub api_key: Option<String>,
    pub temperature: f32,
    pub max_tokens: u32,
    /// When the endpoint is the built-in local default and no API key is
    /// set, bootstrap a local backend (install + serve + pull) on first
    /// run so the assistant works out of the box.
    pub auto_setup: bool,
    pub persona: AssistantPersona,
}

impl Default for GenerationConfig {
    fn default() -> Self {
        Self {
            model: "llama3.2:3b".to_string(),
            endpoint: "http://127.0.0.1:11434/v1/chat/completions".to_string(),
            api_key: None,
            temperature: 0.7,
            max_tokens: 1024,
            auto_setup: true,
            persona: AssistantPersona::Consigliere,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsConfig {
    pub engine: String,
    pub voice: String,
    pub speed: f32,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            engine: "auto".to_string(),
            voice: "en_US-amy-medium".to_string(),
            speed: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WakeConfig {
    /// Wake-word model name ("hey_jarvis", "alexa", "hey_mycroft",
    /// "hey_rhasspy", or a custom openWakeWord model) — "off" disables
    /// wake-word gating so plain VAD activation applies.
    pub model: String,
    /// Detection score threshold, 0.0..=1.0. Lower values catch more
    /// hits (and more false accepts); higher is stricter.
    pub threshold: f32,
    /// openWakeWord's built-in Silero VAD gate (0.0..=1.0): detections
    /// only count when speech is actually present, which suppresses
    /// false accepts on non-speech noise. 0.0 disables the gate.
    pub vad_gate: f32,
    /// Seconds of wake-word-free listening after each completed turn —
    /// Alexa-style follow-up mode. 0 disables follow-ups.
    pub follow_up_seconds: u64,
}

impl Default for WakeConfig {
    fn default() -> Self {
        Self {
            model: "off".to_string(),
            threshold: 0.5,
            vad_gate: 0.5,
            follow_up_seconds: 6,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    pub enable_long_term: bool,
    pub db_path: PathBuf,
    pub max_history_turns: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enable_long_term: true,
            db_path: PathBuf::from("~/.local/share/tycho/memory.jsonl"),
            max_history_turns: 20,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelSpecConfig {
    pub repo_id: String,
    pub revision: String,
    pub filename: String,
    pub target_filename: String,
    pub expected_min_bytes: u64,
    pub subfolder: Option<String>,
}

impl ModelSpecConfig {
    pub fn to_hf_spec(&self) -> HuggingFaceModelSpec {
        let mut spec =
            HuggingFaceModelSpec::new(&self.repo_id, &self.filename, &self.target_filename)
                .with_revision(&self.revision)
                .with_min_bytes(self.expected_min_bytes);
        if let Some(ref sub) = self.subfolder {
            spec = spec.with_subfolder(sub);
        }
        spec
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelsConfig {
    pub cache_dir: PathBuf,
    pub auto_download_on_first_run: bool,
    pub hf_endpoint: String,
    pub hf_token: Option<String>,
    pub stt_model: ModelSpecConfig,
    pub tts_model: ModelSpecConfig,
    pub tts_voices: ModelSpecConfig,
    pub router_model: Option<ModelSpecConfig>,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            cache_dir: PathBuf::from("~/.local/share/tycho/models"),
            auto_download_on_first_run: true,
            hf_endpoint: "https://huggingface.co".to_string(),
            hf_token: std::env::var("HF_TOKEN").ok(),
            stt_model: ModelSpecConfig {
                repo_id: "ggerganov/whisper.cpp".to_string(),
                revision: "main".to_string(),
                filename: "ggml-tiny.en.bin".to_string(),
                target_filename: "ggml-tiny.en.bin".to_string(),
                expected_min_bytes: 32 * 1024 * 1024,
                subfolder: None,
            },
            tts_model: ModelSpecConfig {
                repo_id: "rhasspy/piper-voices".to_string(),
                revision: "main".to_string(),
                filename: "en/en_US/amy/medium/en_US-amy-medium.onnx".to_string(),
                target_filename: "en_US-amy-medium.onnx".to_string(),
                expected_min_bytes: 1024 * 1024,
                subfolder: None,
            },
            tts_voices: ModelSpecConfig {
                repo_id: "rhasspy/piper-voices".to_string(),
                revision: "main".to_string(),
                filename: "en/en_US/amy/medium/en_US-amy-medium.onnx.json".to_string(),
                target_filename: "en_US-amy-medium.onnx.json".to_string(),
                expected_min_bytes: 512,
                subfolder: None,
            },
            router_model: Some(ModelSpecConfig {
                repo_id: "convaiinnovations/laya".to_string(),
                revision: "main".to_string(),
                filename: "model.onnx".to_string(),
                target_filename: "laya_intent_classifier.onnx".to_string(),
                expected_min_bytes: 512 * 1024,
                subfolder: None,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Show the persistent floating status orb (wlr layer-shell overlay).
    pub orb: bool,
    /// Activation mode: "auto" (VAD always-on), "manual" (orb click
    /// only), or "off" (ignores all input).
    pub mode: String,
    /// Orb placement: "auto" (compositor heuristic), a named preset
    /// ("bottom-left", …), or "custom" (uses margin_x/margin_y).
    pub position: String,
    /// Custom placement: distance in px from the left screen edge.
    pub margin_x: i32,
    /// Custom placement: distance in px from the top screen edge.
    pub margin_y: i32,
    /// Nonverbal audio cues (soft chime on wake/listen, blip when
    /// deliberation starts) — the hardware-norm listening signal that
    /// also works headless.
    pub earcons: bool,
    /// Chat transcript verbosity: "quiet" (conversation turns only),
    /// "normal" (+ lifecycle status lines), "verbose" (+ routing and
    /// synthesis diagnostics).
    pub chat_verbosity: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            orb: true,
            mode: "auto".to_string(),
            position: "auto".to_string(),
            margin_x: -1,
            margin_y: -1,
            earcons: true,
            chat_verbosity: "normal".to_string(),
        }
    }
}

/// Rewrites the `[ui]` table of the active config file, preserving all
/// other sections. `None` fields keep their existing values. No-op when
/// no config path can be resolved.
pub fn save_ui_patch(
    mode: Option<&str>,
    position: Option<&str>,
    margin_x: Option<i32>,
    margin_y: Option<i32>,
) -> crate::error::Result<()> {
    match config_file_path() {
        Some(path) => save_ui_patch_to(&path, mode, position, margin_x, margin_y),
        None => Ok(()),
    }
}

/// `save_ui_patch` against an explicit path. Creates the file and
/// parent directory when missing.
pub fn save_ui_patch_to(
    path: &std::path::Path,
    mode: Option<&str>,
    position: Option<&str>,
    margin_x: Option<i32>,
    margin_y: Option<i32>,
) -> crate::error::Result<()> {
    let mut doc: toml::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
    if !doc.is_table() {
        doc = toml::Value::Table(toml::map::Map::new());
    }
    let root = doc.as_table_mut().expect("checked above");
    let ui = root
        .entry("ui".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    if !ui.is_table() {
        *ui = toml::Value::Table(toml::map::Map::new());
    }
    let ui = ui.as_table_mut().expect("checked above");
    if let Some(m) = mode {
        ui.insert("mode".to_string(), toml::Value::String(m.to_string()));
    }
    if let Some(p) = position {
        ui.insert("position".to_string(), toml::Value::String(p.to_string()));
    }
    if let Some(x) = margin_x {
        ui.insert("margin_x".to_string(), toml::Value::Integer(x as i64));
    }
    if let Some(y) = margin_y {
        ui.insert("margin_y".to_string(), toml::Value::Integer(y as i64));
    }

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(crate::error::Error::Io)?;
    }
    let text =
        toml::to_string_pretty(&doc).map_err(|e| crate::error::Error::Config(e.to_string()))?;
    std::fs::write(path, text).map_err(crate::error::Error::Io)
}

/// Writes a single dotted-key setting (e.g. `vad.energy_threshold`)
/// into the active config file, preserving all other values. `value`
/// keeps its TOML type so numbers stay numbers. No-op when no config
/// path can be resolved.
pub fn save_config_patch(key: &str, value: toml::Value) -> crate::error::Result<()> {
    match config_file_path() {
        Some(path) => save_config_patch_to(&path, key, value),
        None => Ok(()),
    }
}

/// `save_config_patch` against an explicit path. Intermediate tables
/// are created as needed; the file and parent directory are created
/// when missing.
pub fn save_config_patch_to(
    path: &std::path::Path,
    key: &str,
    value: toml::Value,
) -> crate::error::Result<()> {
    let mut doc: toml::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
    if !doc.is_table() {
        doc = toml::Value::Table(toml::map::Map::new());
    }
    let mut table = doc.as_table_mut().expect("checked above");
    let mut parts = key.split('.').peekable();
    while let Some(part) = parts.next() {
        if part.is_empty() {
            return Err(crate::error::Error::Config(format!(
                "invalid setting key '{key}'"
            )));
        }
        if parts.peek().is_none() {
            table.insert(part.to_string(), value.clone());
            break;
        }
        let next = table
            .entry(part.to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        if !next.is_table() {
            *next = toml::Value::Table(toml::map::Map::new());
        }
        table = next.as_table_mut().expect("checked above");
    }

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(crate::error::Error::Io)?;
    }
    let text =
        toml::to_string_pretty(&doc).map_err(|e| crate::error::Error::Config(e.to_string()))?;
    std::fs::write(path, text).map_err(crate::error::Error::Io)
}

/// Path of the config file `load()`/`save_ui_patch` operate on, whether
/// or not it exists yet.
pub fn config_file_path() -> Option<PathBuf> {
    std::env::var("TYCHO_CONFIG")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config/tycho/tycho.toml"))
        })
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TychoConfig {
    pub audio: AudioConfig,
    pub vad: VadConfig,
    pub stt: SttConfig,
    pub router: RouterConfig,
    pub desktop: DesktopConfig,
    pub generation: GenerationConfig,
    pub tts: TtsConfig,
    pub wake: WakeConfig,
    pub memory: MemoryConfig,
    pub models: ModelsConfig,
    pub ui: UiConfig,
}

impl TychoConfig {
    /// Loads configuration from `$TYCHO_CONFIG` or `~/.config/tycho/tycho.toml`,
    /// falling back to compiled defaults when no file exists. Environment
    /// variables always take precedence over file values.
    pub fn load() -> crate::error::Result<Self> {
        let explicit = std::env::var("TYCHO_CONFIG").ok().map(PathBuf::from);
        let default_path = std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(".config/tycho/tycho.toml"));

        match explicit.or(default_path) {
            Some(path) if path.exists() => Self::from_file(&path),
            _ => Ok(Self::default().with_env_overrides()),
        }
    }

    pub fn from_file(path: &std::path::Path) -> crate::error::Result<Self> {
        let text = std::fs::read_to_string(path).map_err(crate::error::Error::Io)?;
        let cfg: Self = toml::from_str(&text).map_err(crate::error::Error::Toml)?;
        Ok(cfg.with_env_overrides())
    }

    fn with_env_overrides(mut self) -> Self {
        if let Ok(dir) = std::env::var("TYCHO_CACHE_DIR") {
            self.models.cache_dir = PathBuf::from(dir);
        }
        if let Ok(token) = std::env::var("HF_TOKEN") {
            self.models.hf_token = Some(token);
        }
        if let Ok(key) = std::env::var("TYCHO_JEV_API_KEY") {
            self.router.jev_api_key = Some(key);
        }
        if let Ok(key) = std::env::var("TYCHO_API_KEY") {
            self.generation.api_key = Some(key);
        }
        if let Ok(endpoint) = std::env::var("TYCHO_ENDPOINT") {
            self.generation.endpoint = endpoint;
        }
        self
    }
}
