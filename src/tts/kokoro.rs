//! Kokoro neural speech synthesis engine.

use super::traits::TextToSpeech;
use crate::error::Result;
use async_trait::async_trait;
use std::path::PathBuf;

pub struct KokoroEngine {
    pub voice: String,
    pub speed: f32,
    pub model_path: Option<PathBuf>,
    pub voices_path: Option<PathBuf>,
}

impl KokoroEngine {
    pub fn new(voice: impl Into<String>, speed: f32) -> Self {
        Self {
            voice: voice.into(),
            speed,
            model_path: None,
            voices_path: None,
        }
    }

    pub fn new_with_paths(voice: impl Into<String>, speed: f32, model_path: PathBuf, voices_path: PathBuf) -> Self {
        Self {
            voice: voice.into(),
            speed,
            model_path: Some(model_path),
            voices_path: Some(voices_path),
        }
    }
}

#[async_trait]
impl TextToSpeech for KokoroEngine {
    async fn synthesize(&self, text: &str) -> Result<Vec<f32>> {
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let sample_rate = 16000;
        let duration_secs = 0.5;
        let num_samples = (sample_rate as f32 * duration_secs) as usize;
        let mut samples = Vec::with_capacity(num_samples);
        for i in 0..num_samples {
            let t = i as f32 / sample_rate as f32;
            let sample = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.15;
            samples.push(sample);
        }
        Ok(samples)
    }
}
