//! Audio playback sink with atomic cancellation for barge-in interruption.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct AudioPlaybackSink {
    interrupted: Arc<AtomicBool>,
}

impl AudioPlaybackSink {
    pub fn new() -> Self {
        Self {
            interrupted: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn interrupt(&self) {
        self.interrupted.store(true, Ordering::SeqCst);
    }

    pub fn reset(&self) {
        self.interrupted.store(false, Ordering::SeqCst);
    }

    pub fn is_interrupted(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }

    pub async fn play_chunk(&self, _samples: &[f32]) -> bool {
        if self.is_interrupted() {
            return false;
        }
        true
    }
}

impl Default for AudioPlaybackSink {
    fn default() -> Self {
        Self::new()
    }
}
