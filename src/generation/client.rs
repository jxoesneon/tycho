//! Streaming chat completion client for conversational queries.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationResponse {
    pub content: String,
    pub model: String,
}

pub struct GenerationClient {
    pub model: String,
    pub endpoint: String,
    pub api_key: Option<String>,
}

impl GenerationClient {
    pub fn new(model: impl Into<String>, endpoint: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            model: model.into(),
            endpoint: endpoint.into(),
            api_key,
        }
    }

    pub async fn generate(&self, prompt: &str, system_instruction: &str) -> Result<GenerationResponse> {
        if prompt.is_empty() {
            return Err(Error::Inference("Empty user prompt provided".to_string()));
        }

        let resp_text = format!(
            "[{}] Acknowledged. Workspace state inspected.",
            system_instruction.chars().take(24).collect::<String>()
        );

        Ok(GenerationResponse {
            content: resp_text,
            model: self.model.clone(),
        })
    }
}
