//! Low-latency audio capture, playback, and voice activity detection.

pub mod capture;
pub mod playback;
pub mod vad;

pub use capture::AudioCaptureStream;
pub use playback::AudioPlaybackSink;
pub use vad::{VadState, VoiceActivityDetector};
