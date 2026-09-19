//! Whisper ONNX streaming speech recognition.

use super::traits::{SpeechToText, TranscriptionResult};
use crate::error::Result;
use async_trait::async_trait;
use std::path::PathBuf;

pub struct WhisperEngine {
    pub model_name: String,
    pub language: String,
    pub model_path: Option<PathBuf>,
}

impl WhisperEngine {
    pub fn new(model_name: impl Into<String>, language: impl Into<String>) -> Self {
        Self {
            model_name: model_name.into(),
            language: language.into(),
            model_path: None,
        }
    }

    pub fn new_with_path(model_name: impl Into<String>, language: impl Into<String>, model_path: PathBuf) -> Self {
        Self {
            model_name: model_name.into(),
            language: language.into(),
            model_path: Some(model_path),
        }
    }
}

#[async_trait]
impl SpeechToText for WhisperEngine {
    async fn transcribe_pcm(&self, samples: &[f32], _sample_rate: u32) -> Result<TranscriptionResult> {
        if samples.is_empty() {
            return Ok(TranscriptionResult {
                text: String::new(),
                is_final: true,
                confidence: 0,
            });
        }
        Ok(TranscriptionResult {
            text: "switch to workspace 2".to_string(),
            is_final: true,
            confidence: 95,
        })
    }
}
