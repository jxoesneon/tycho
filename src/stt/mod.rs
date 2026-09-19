//! Speech-to-text (STT) traits and Whisper engine.

pub mod traits;
pub mod whisper;

pub use traits::{SpeechToText, TranscriptionResult};
pub use whisper::WhisperEngine;
