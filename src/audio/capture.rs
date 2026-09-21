//! Real-time audio capture from a system input device via cpal, with a
//! scripted virtual input for tests and headless diagnostics.

use crate::error::{Error, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct AudioCaptureStream {
    running: Arc<AtomicBool>,
    buffer_size: usize,
    scripted: Option<Vec<Vec<f32>>>,
    device_name: Option<String>,
}

impl AudioCaptureStream {
    /// Captures from the host's default input device.
    pub fn new(buffer_size: usize) -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            buffer_size,
            scripted: None,
            device_name: None,
        }
    }

    /// Captures from a named input device (falls back to default when `None`).
    pub fn with_device(buffer_size: usize, device_name: Option<String>) -> Self {
        Self {
            device_name,
            ..Self::new(buffer_size)
        }
    }

    /// Creates a virtual input that replays `frames` once and then ends the
    /// stream. Used for tests and headless diagnostics where no capture
    /// device exists.
    pub fn new_scripted(frames: Vec<Vec<f32>>) -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            buffer_size: 0,
            scripted: Some(frames),
            device_name: None,
        }
    }

    /// Starts capture, returning a receiver of mono f32 frames of
    /// `buffer_size` samples. Errors when no input device is available or
    /// the device rejects its stream configuration.
    ///
    /// The channel is bounded and the real-time callback uses
    /// `try_send` — when the consumer falls behind (e.g. during
    /// transcription/generation), frames are dropped rather than
    /// stalling the audio thread or accumulating stale audio that the
    /// VAD would later mistake for live speech.
    pub fn start(&self) -> Result<mpsc::Receiver<Vec<f32>>> {
        let (tx, rx) = mpsc::channel(256);
        let running = self.running.clone();
        running.store(true, Ordering::SeqCst);

        if let Some(frames) = self.scripted.clone() {
            tokio::spawn(async move {
                for frame in frames {
                    if !running.load(Ordering::SeqCst) || tx.send(frame).await.is_err() {
                        return;
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
                }
            });
            return Ok(rx);
        }

        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
        let frame_size = self.buffer_size.max(1);
        let device_name = self.device_name.clone();
        let running_thread = running.clone();

        std::thread::spawn(move || {
            match open_capture_stream(&device_name, frame_size, tx.clone(), running_thread.clone())
            {
                Ok(stream) => {
                    let _ = ready_tx.send(Ok(()));
                    // Park while the device stream runs its callbacks.
                    while running_thread.load(Ordering::SeqCst) && !tx.is_closed() {
                        std::thread::sleep(std::time::Duration::from_millis(25));
                    }
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
            running_thread.store(false, Ordering::SeqCst);
        });

        match ready_rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(rx),
            Ok(Err(e)) => {
                self.running.store(false, Ordering::SeqCst);
                Err(Error::Audio(e))
            }
            Err(_) => {
                self.running.store(false, Ordering::SeqCst);
                Err(Error::Audio("capture init timed out".to_string()))
            }
        }
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

/// Opens the input device on the calling thread (cpal streams are bound to
/// their creation thread on some backends), wires mono f32 chunk forwarding
/// into `tx`, and returns the live stream. The caller parks while
/// `running` is set and the channel is open.
fn open_capture_stream(
    device_name: &Option<String>,
    frame_size: usize,
    tx: mpsc::Sender<Vec<f32>>,
    running: Arc<AtomicBool>,
) -> std::result::Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = match device_name {
        Some(name) => {
            let mut found = None;
            if let Ok(mut devices) = host.input_devices() {
                found = devices.find(|d| {
                    d.description()
                        .map(|desc| desc.name().contains(name.as_str()))
                        .unwrap_or(false)
                });
            }
            found.ok_or_else(|| format!("input device not found: {}", name))?
        }
        None => host
            .default_input_device()
            .ok_or_else(|| "no default input device".to_string())?,
    };

    let supported = device
        .default_input_config()
        .map_err(|e| format!("input device config: {}", e))?;
    let config = supported.config();
    let channels = (config.channels as usize).max(1);

    // Shared accumulation buffer: the callback appends mono samples and
    // forwards whole frames to the async receiver.
    let pending = Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));
    let pending_cb = pending.clone();
    let tx_cb = tx.clone();
    let running_cb = running.clone();

    let push = move |mono: f32| {
        let mut frames = Vec::new();
        {
            let mut buf = pending_cb.lock().unwrap();
            buf.push(mono);
            while buf.len() >= frame_size {
                frames.push(buf.drain(..frame_size).collect::<Vec<f32>>());
            }
        }
        for frame in frames {
            if !running_cb.load(Ordering::SeqCst) {
                return;
            }
            // Never block a real-time audio callback: a full channel
            // drops the frame; the consumer drains the backlog after
            // processing so stale audio cannot masquerade as live speech.
            if tx_cb.try_send(frame).is_err() {
                continue;
            }
        }
    };

    let err_cb = |e| tracing::warn!("capture stream error: {}", e);
    let stream = match supported.sample_format() {
        SampleFormat::F32 => device.build_input_stream(
            config,
            move |data: &[f32], _| {
                for s in mono_from_f32(data, channels) {
                    push(s);
                }
            },
            err_cb,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            config,
            move |data: &[i16], _| {
                for s in mono_from_i16(data, channels) {
                    push(s);
                }
            },
            err_cb,
            None,
        ),
        SampleFormat::U16 => device.build_input_stream(
            config,
            move |data: &[u16], _| {
                for s in mono_from_u16(data, channels) {
                    push(s);
                }
            },
            err_cb,
            None,
        ),
        fmt => return Err(format!("unsupported input sample format: {:?}", fmt)),
    }
    .map_err(|e| format!("build input stream: {}", e))?;

    stream
        .play()
        .map_err(|e| format!("start input stream: {}", e))?;
    Ok(stream)
}

/// Mixes interleaved `channels` of f32 samples down to mono.
fn mono_from_f32(data: &[f32], channels: usize) -> impl Iterator<Item = f32> + '_ {
    data.chunks(channels)
        .map(|c| c.iter().sum::<f32>() / c.len() as f32)
}

/// Mixes interleaved `channels` of signed 16-bit samples down to mono f32.
fn mono_from_i16(data: &[i16], channels: usize) -> impl Iterator<Item = f32> + '_ {
    data.chunks(channels)
        .map(|c| c.iter().map(|s| *s as f32 / i16::MAX as f32).sum::<f32>() / c.len() as f32)
}

/// Mixes interleaved `channels` of unsigned 16-bit samples down to mono f32
/// (centred on the unsigned midpoint).
fn mono_from_u16(data: &[u16], channels: usize) -> impl Iterator<Item = f32> + '_ {
    let mid = u16::MAX as f32 / 2.0;
    data.chunks(channels)
        .map(move |c| c.iter().map(|s| (*s as f32 - mid) / mid).sum::<f32>() / c.len() as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_mixdown_formats() {
        // f32 stereo → mono mean.
        let out: Vec<f32> = mono_from_f32(&[0.5, -0.5, 0.25, 0.25], 2).collect();
        assert_eq!(out, vec![0.0, 0.25]);

        // i16 scaled by i16::MAX.
        let out: Vec<f32> = mono_from_i16(&[i16::MAX, 0], 2).collect();
        assert!((out[0] - 0.5).abs() < 1e-4);

        // u16 centred on its midpoint: max ≈ +1, 0 ≈ -1.
        let hi: Vec<f32> = mono_from_u16(&[u16::MAX], 1).collect();
        let lo: Vec<f32> = mono_from_u16(&[0], 1).collect();
        assert!(hi[0] > 0.99 && lo[0] < -0.99);

        // Trailing partial frames still yield a sample.
        assert_eq!(mono_from_f32(&[0.8], 2).count(), 1);
    }
}
