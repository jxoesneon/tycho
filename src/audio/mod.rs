//! Low-latency audio capture, playback, and voice activity detection.

pub mod capture;
pub mod earcon;
pub mod playback;
pub mod vad;

pub use capture::AudioCaptureStream;
pub use earcon::Earcon;
pub use playback::AudioPlaybackSink;
pub use vad::{VadState, VoiceActivityDetector};

/// Linear-interpolating resampler for mono f32 audio. Used by the playback
/// sink and TTS engine to reconcile differing source and device rates.
pub fn resample_linear(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || from == 0 || samples.is_empty() {
        return samples.to_vec();
    }
    let out_len = ((samples.len() as u64 * to as u64) / from as u64).max(1) as usize;
    let step = from as f64 / to as f64;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * step;
        let idx = pos as usize;
        let frac = (pos - idx as f64) as f32;
        let a = samples[idx.min(samples.len() - 1)];
        let b = samples[(idx + 1).min(samples.len() - 1)];
        out.push(a + (b - a) * frac);
    }
    out
}
