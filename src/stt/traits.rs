//! Speech recognition interfaces.

use crate::error::Result;
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptionResult {
    pub text: String,
    pub is_final: bool,
    pub confidence: u8,
}

#[async_trait]
pub trait SpeechToText: Send + Sync {
    async fn transcribe_pcm(
        &self,
        samples: &[f32],
        sample_rate: u32,
    ) -> Result<TranscriptionResult>;
}
