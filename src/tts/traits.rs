//! Text-to-speech audio synthesis traits.

use crate::error::Result;
use async_trait::async_trait;

#[async_trait]
pub trait TextToSpeech: Send + Sync {
    async fn synthesize(&self, text: &str) -> Result<Vec<f32>>;
}
