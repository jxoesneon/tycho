//! Whisper speech recognition via whisper.cpp (whisper-rs), running ggml
//! models on CPU.

use super::traits::{SpeechToText, TranscriptionResult};
use crate::audio::resample_linear;
use crate::error::{Error, Result};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const WHISPER_RATE: u32 = 16000;

pub struct WhisperEngine {
    pub model_name: String,
    pub language: String,
    pub model_path: Option<PathBuf>,
    ctx: Mutex<Option<Arc<WhisperContext>>>,
}

impl WhisperEngine {
    /// Engine with no model configured — `transcribe_pcm` reports an honest
    /// error until a model path is provided.
    pub fn new(model_name: impl Into<String>, language: impl Into<String>) -> Self {
        Self {
            model_name: model_name.into(),
            language: language.into(),
            model_path: None,
            ctx: Mutex::new(None),
        }
    }

    /// Engine backed by a ggml whisper model file. The file must exist;
    /// inference context is loaded lazily on first use.
    pub fn new_with_path(
        model_name: impl Into<String>,
        language: impl Into<String>,
        model_path: PathBuf,
    ) -> Result<Self> {
        if !model_path.is_file() {
            return Err(Error::ModelNotFound(model_path));
        }
        Ok(Self {
            model_name: model_name.into(),
            language: language.into(),
            model_path: Some(model_path),
            ctx: Mutex::new(None),
        })
    }

    fn context(&self) -> Result<Arc<WhisperContext>> {
        let mut guard = self.ctx.lock().unwrap();
        if let Some(ctx) = guard.as_ref() {
            return Ok(ctx.clone());
        }
        let path = self.model_path.clone().ok_or_else(|| {
            Error::ModelMissing(format!("{} model path not set", self.model_name))
        })?;
        if !path.is_file() {
            return Err(Error::ModelNotFound(path));
        }
        let ctx = WhisperContext::new_with_params(&path, WhisperContextParameters::default())
            .map_err(|e| Error::Transcription(format!("load {}: {}", path.display(), e)))?;
        let ctx = Arc::new(ctx);
        *guard = Some(ctx.clone());
        Ok(ctx)
    }
}

#[async_trait]
impl SpeechToText for WhisperEngine {
    async fn transcribe_pcm(
        &self,
        samples: &[f32],
        sample_rate: u32,
    ) -> Result<TranscriptionResult> {
        if samples.is_empty() {
            return Ok(TranscriptionResult {
                text: String::new(),
                is_final: true,
                confidence: 0,
            });
        }

        let ctx = self.context()?;
        let language = self.language.clone();
        let pcm = resample_linear(samples, sample_rate, WHISPER_RATE);

        tokio::task::spawn_blocking(move || {
            let mut state = ctx
                .create_state()
                .map_err(|e| Error::Transcription(format!("whisper state: {}", e)))?;
            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_language(Some(language.as_str()));
            params.set_no_timestamps(true);
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);

            state
                .full(params, &pcm)
                .map_err(|e| Error::Transcription(format!("whisper inference: {}", e)))?;

            let n = state.full_n_segments();
            let mut text = String::new();
            let mut prob_sum = 0.0f32;
            let mut prob_count = 0u32;
            let mut prev_seg = String::new();
            for i in 0..n {
                if let Some(seg) = state.get_segment(i) {
                    let seg_text = seg.to_str_lossy()?;
                    let trimmed = seg_text.trim();
                    // Whisper captions non-speech audio as "(wind blowing)",
                    // "[BLANK_AUDIO]", "*music*" — drop it rather than feeding
                    // noise hallucinations to the router.
                    if is_non_speech_segment(trimmed) {
                        continue;
                    }
                    // Consecutive identical segments are the classic noise
                    // hallucination loop — collapse to one occurrence.
                    if trimmed.eq_ignore_ascii_case(&prev_seg) {
                        continue;
                    }
                    prev_seg = trimmed.to_string();
                    text.push_str(&seg_text);
                    for t in 0..seg.n_tokens() {
                        if let Some(tok) = seg.get_token(t) {
                            prob_sum += tok.token_probability();
                            prob_count += 1;
                        }
                    }
                }
            }
            let confidence = if prob_count > 0 {
                ((prob_sum / prob_count as f32) * 100.0).clamp(0.0, 100.0) as u8
            } else {
                0
            };
            Ok(TranscriptionResult {
                text: text.trim().to_string(),
                is_final: true,
                confidence,
            })
        })
        .await
        .map_err(|e| Error::Transcription(format!("whisper task: {}", e)))?
    }
}

/// True when a segment is entirely non-speech captions: one or more
/// `()`, `[]`, or `*…*` groups — e.g. `(wind blowing)`, `[BLANK_AUDIO]`,
/// or a hallucination loop like `(mumbling) (mumbling) (mumbling)`.
fn is_non_speech_segment(s: &str) -> bool {
    let mut rest = s;
    let mut stripped_any = false;
    loop {
        let t = rest.trim_start();
        let close = match t.as_bytes().first() {
            Some(b'(') => b')',
            Some(b'[') => b']',
            Some(b'*') => b'*',
            _ => break,
        };
        let Some(rel) = t[1..].find(close as char) else {
            break;
        };
        let end = 1 + rel;
        if t[1..end].trim().is_empty() {
            break;
        }
        stripped_any = true;
        rest = &t[end + 1..];
    }
    stripped_any && rest.trim().is_empty()
}

impl From<whisper_rs::WhisperError> for Error {
    fn from(e: whisper_rs::WhisperError) -> Self {
        Error::Transcription(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::is_non_speech_segment;

    #[test]
    fn single_caption_is_non_speech() {
        for s in ["(wind blowing)", "[BLANK_AUDIO]", "*music*", "(silence)"] {
            assert!(is_non_speech_segment(s), "{s}");
        }
    }

    #[test]
    fn repeated_caption_loop_is_non_speech() {
        assert!(is_non_speech_segment(
            "(mumbling) (mumbling) (mumbling) (mumbling) (mumbling) (mumbling)"
        ));
        assert!(is_non_speech_segment("[noise][noise]"));
        assert!(is_non_speech_segment("*music* (applause)"));
    }

    #[test]
    fn real_speech_is_not_non_speech() {
        for s in [
            "switch to workspace 2",
            "hello (world)",
            "(aside) but real words follow",
            "what's the weather",
            "x",
            "()",
            "(unclosed",
        ] {
            assert!(!is_non_speech_segment(s), "{s}");
        }
    }
}
