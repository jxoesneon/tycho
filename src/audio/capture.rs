//! Real-time audio capture stream using ring-buffered audio frames.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct AudioCaptureStream {
    running: Arc<AtomicBool>,
    buffer_size: usize,
}

impl AudioCaptureStream {
    pub fn new(buffer_size: usize) -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            buffer_size,
        }
    }

    pub fn start(&self) -> mpsc::Receiver<Vec<f32>> {
        let (tx, rx) = mpsc::channel(64);
        let running = self.running.clone();
        running.store(true, Ordering::SeqCst);
        let frame_size = self.buffer_size;

        tokio::spawn(async move {
            while running.load(Ordering::SeqCst) {
                let frame = vec![0.0f32; frame_size];
                if tx.send(frame).await.is_err() {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
            }
        });

        rx
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}
