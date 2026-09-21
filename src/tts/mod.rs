//! Speech synthesis subsystem.

pub mod catalog;
pub mod engine;
pub mod traits;

pub use engine::SpeechSynthesizer;
pub use traits::TextToSpeech;
