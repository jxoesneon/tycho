//! Tycho central configuration schema and loader.

use crate::generation::AssistantPersona;
use crate::models::hub::HuggingFaceModelSpec;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub struct SttConfig {
    pub engine: String,
    pub model_path: Option<PathBuf>,
    pub language: String,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            engine: "whisper-onnx".to_string(),
            model_path: None,
            language: "en".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    pub fast_path_confidence_threshold: f32,
    pub fallback_to_laya: bool,
    pub jev_api_key: Option<String>,
    pub laya_hf_repo: String,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            fast_path_confidence_threshold: 0.82,
            fallback_to_laya: true,
            jev_api_key: None,
            laya_hf_repo: "convaiinnovations/laya".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub struct GenerationConfig {
    pub model: String,
    pub endpoint: String,
    pub api_key: Option<String>,
    pub temperature: f32,
    pub max_tokens: u32,
    pub persona: AssistantPersona,
}

impl Default for GenerationConfig {
    fn default() -> Self {
        Self {
            model: "local-chat".to_string(),
            endpoint: "http://127.0.0.1:11434/v1/chat/completions".to_string(),
            api_key: None,
            temperature: 0.7,
            max_tokens: 1024,
            persona: AssistantPersona::Consigliere,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsConfig {
    pub engine: String,
    pub voice: String,
    pub speed: f32,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            engine: "kokoro".to_string(),
            voice: "af_heart".to_string(),
            speed: 1.05,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub enable_long_term: bool,
    pub db_path: PathBuf,
    pub max_history_turns: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enable_long_term: true,
            db_path: PathBuf::from("~/.tycho/memory"),
            max_history_turns: 20,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
        let mut spec = HuggingFaceModelSpec::new(&self.repo_id, &self.filename, &self.target_filename)
            .with_revision(&self.revision)
            .with_min_bytes(self.expected_min_bytes);
        if let Some(ref sub) = self.subfolder {
            spec = spec.with_subfolder(sub);
        }
        spec
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
                repo_id: "onnx-community/whisper-tiny.en".to_string(),
                revision: "main".to_string(),
                filename: "onnx/model.onnx".to_string(),
                target_filename: "whisper-tiny.en.onnx".to_string(),
                expected_min_bytes: 1024 * 1024,
                subfolder: None,
            },
            tts_model: ModelSpecConfig {
                repo_id: "onnx-community/Kokoro-82M-ONNX".to_string(),
                revision: "main".to_string(),
                filename: "kokoro-v0_19.onnx".to_string(),
                target_filename: "kokoro-v0_19.onnx".to_string(),
                expected_min_bytes: 1024 * 1024,
                subfolder: None,
            },
            tts_voices: ModelSpecConfig {
                repo_id: "onnx-community/Kokoro-82M-ONNX".to_string(),
                revision: "main".to_string(),
                filename: "voices.bin".to_string(),
                target_filename: "voices.bin".to_string(),
                expected_min_bytes: 10 * 1024,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TychoConfig {
    pub audio: AudioConfig,
    pub vad: VadConfig,
    pub stt: SttConfig,
    pub router: RouterConfig,
    pub desktop: DesktopConfig,
    pub generation: GenerationConfig,
    pub tts: TtsConfig,
    pub memory: MemoryConfig,
    pub models: ModelsConfig,
}
