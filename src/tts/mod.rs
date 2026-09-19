//! Speech synthesis subsystem.

pub mod kokoro;
pub mod traits;

pub use kokoro::KokoroEngine;
pub use traits::TextToSpeech;
