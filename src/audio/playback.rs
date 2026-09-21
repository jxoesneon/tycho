//! Audio playback sink driving a real output device via cpal, with atomic
//! cancellation for barge-in interruption and a null mode for tests.

use crate::audio::resample_linear;
use crate::error::{Error, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

enum SinkMode {
    /// Virtual output for tests and headless diagnostics: accepts audio,
    /// discards it, honours interruption.
    Null,
    /// Device mode, not yet opened.
    Uninit,
    /// Live output stream with a shared sample queue and device rate.
    Live {
        queue: Arc<Mutex<VecDeque<f32>>>,
        device_rate: u32,
        shutdown: Arc<AtomicBool>,
        /// RMS of the most recently played buffer (f32 bits) — the echo
        /// reference for barge-in gating without a full AEC.
        played_rms: Arc<std::sync::atomic::AtomicU32>,
        /// Notifies when the queue last drained to empty.
        drained: Arc<tokio::sync::Notify>,
        /// When the queue last reached empty — refractory window base.
        last_drain: Arc<Mutex<Option<std::time::Instant>>>,
    },
}

/// Clones share the same underlying stream — the sink is a handle.
#[derive(Clone)]
pub struct AudioPlaybackSink {
    interrupted: Arc<AtomicBool>,
    mode: Arc<Mutex<SinkMode>>,
    device_name: Option<String>,
    source_rate: u32,
}

impl AudioPlaybackSink {
    /// Plays to the host's default output device with input at `source_rate`
    /// Hz (16 kHz by default).
    pub fn new() -> Self {
        Self {
            interrupted: Arc::new(AtomicBool::new(false)),
            mode: Arc::new(Mutex::new(SinkMode::Uninit)),
            device_name: None,
            source_rate: 16000,
        }
    }

    /// Plays to a named output device (default when `None`) with input
    /// arriving at `source_rate` Hz.
    pub fn with_device(device_name: Option<String>, source_rate: u32) -> Self {
        Self {
            interrupted: Arc::new(AtomicBool::new(false)),
            mode: Arc::new(Mutex::new(SinkMode::Uninit)),
            device_name,
            source_rate,
        }
    }

    /// Virtual output for tests and headless diagnostics.
    pub fn new_null() -> Self {
        Self {
            interrupted: Arc::new(AtomicBool::new(false)),
            mode: Arc::new(Mutex::new(SinkMode::Null)),
            device_name: None,
            source_rate: 16000,
        }
    }

    /// Interrupts playback: buffered audio is discarded immediately so the
    /// barge-in actually silences the device.
    pub fn interrupt(&self) {
        self.interrupted.store(true, Ordering::SeqCst);
        if let SinkMode::Live {
            queue,
            drained,
            last_drain,
            played_rms,
            ..
        } = &*self.mode.lock().unwrap()
        {
            let had_audio = !queue.lock().unwrap().is_empty();
            queue.lock().unwrap().clear();
            played_rms.store(0, Ordering::Relaxed);
            if had_audio {
                *last_drain.lock().unwrap() = Some(std::time::Instant::now());
                drained.notify_waiters();
            }
        }
    }

    pub fn reset(&self) {
        self.interrupted.store(false, Ordering::SeqCst);
    }

    pub fn is_interrupted(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }

    /// True while queued audio remains unplayed — the honest "Tycho is
    /// audibly speaking" signal the pipeline uses to gate capture.
    pub fn is_playing(&self) -> bool {
        match &*self.mode.lock().unwrap() {
            SinkMode::Live { queue, .. } => !queue.lock().unwrap().is_empty(),
            _ => false,
        }
    }

    /// RMS of the most recently played output buffer — the echo
    /// reference for energy-ratio barge-in gating (0.0 when silent).
    pub fn played_rms(&self) -> f32 {
        match &*self.mode.lock().unwrap() {
            SinkMode::Live { played_rms, .. } => f32::from_bits(played_rms.load(Ordering::Relaxed)),
            _ => 0.0,
        }
    }

    /// True when playback ended less than `ms` ago — the echo-decay
    /// refractory window where residual room echo still suppresses
    /// speech triggers.
    pub fn in_refractory(&self, ms: u64) -> bool {
        match &*self.mode.lock().unwrap() {
            SinkMode::Live { last_drain, .. } => match *last_drain.lock().unwrap() {
                Some(t) => t.elapsed() < std::time::Duration::from_millis(ms),
                None => false,
            },
            _ => false,
        }
    }

    /// Resolves once the queued audio has fully played out (or been
    /// interrupted). Returns immediately when nothing is playing.
    pub async fn wait_drained(&self) {
        loop {
            let notify = match &*self.mode.lock().unwrap() {
                SinkMode::Live { queue, drained, .. } => {
                    if queue.lock().unwrap().is_empty() {
                        return;
                    }
                    drained.clone()
                }
                _ => return,
            };
            tokio::select! {
                _ = notify.notified() => {}
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            }
        }
    }

    /// Queues `samples` (mono f32 at `source_rate` Hz) for playback.
    /// Returns `Ok(false)` when interrupted, `Ok(true)` once queued.
    /// Lazily opens the output device on first call and reports honest
    /// errors when no device can be opened.
    pub async fn play_chunk(&self, samples: &[f32]) -> Result<bool> {
        if self.is_interrupted() {
            return Ok(false);
        }
        if samples.is_empty() {
            return Ok(true);
        }

        let mut mode = self.mode.lock().unwrap();
        loop {
            match &*mode {
                SinkMode::Null => return Ok(true),
                SinkMode::Live {
                    queue, device_rate, ..
                } => {
                    let out = resample_linear(samples, self.source_rate, *device_rate);
                    queue.lock().unwrap().extend(out);
                    return Ok(true);
                }
                SinkMode::Uninit => {
                    *mode = open_output(&self.device_name)?;
                }
            }
        }
    }
}

impl Drop for AudioPlaybackSink {
    fn drop(&mut self) {
        if let SinkMode::Live { shutdown, .. } = &*self.mode.lock().unwrap() {
            shutdown.store(true, Ordering::SeqCst);
        }
    }
}

impl Default for AudioPlaybackSink {
    fn default() -> Self {
        Self::new()
    }
}

/// Opens the output device on a dedicated thread (cpal streams are bound
/// to their creation thread on some backends) and returns a live mode
/// with the shared queue, device rate, and shutdown flag once playing.
fn open_output(device_name: &Option<String>) -> Result<SinkMode> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<std::result::Result<u32, String>>();
    let queue = Arc::new(Mutex::new(VecDeque::<f32>::new()));
    let shutdown = Arc::new(AtomicBool::new(false));
    let played_rms = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let drained = Arc::new(tokio::sync::Notify::new());
    let last_drain = Arc::new(Mutex::new(None));
    let name = device_name.clone();
    let thread_queue = queue.clone();
    let thread_shutdown = shutdown.clone();
    let thread_rms = played_rms.clone();
    let thread_drained = drained.clone();
    let thread_drain = last_drain.clone();
    std::thread::spawn(move || {
        match open_output_stream(
            &name,
            thread_queue,
            thread_rms,
            thread_drained,
            thread_drain,
        ) {
            Ok((stream, device_rate)) => {
                let _ = ready_tx.send(Ok(device_rate));
                while !thread_shutdown.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                drop(stream);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        }
    });

    match ready_rx.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(Ok(device_rate)) => Ok(SinkMode::Live {
            queue,
            device_rate,
            shutdown,
            played_rms,
            drained,
            last_drain,
        }),
        Ok(Err(e)) => Err(Error::Audio(e)),
        Err(_) => {
            shutdown.store(true, Ordering::SeqCst);
            Err(Error::Audio("playback init timed out".to_string()))
        }
    }
}

fn open_output_stream(
    device_name: &Option<String>,
    queue: Arc<Mutex<VecDeque<f32>>>,
    played_rms: Arc<std::sync::atomic::AtomicU32>,
    drained: Arc<tokio::sync::Notify>,
    last_drain: Arc<Mutex<Option<std::time::Instant>>>,
) -> std::result::Result<(cpal::Stream, u32), String> {
    let host = cpal::default_host();
    let device = match device_name {
        Some(name) => {
            let mut found = None;
            if let Ok(mut devices) = host.output_devices() {
                found = devices.find(|d| {
                    d.description()
                        .map(|desc| desc.name().contains(name.as_str()))
                        .unwrap_or(false)
                });
            }
            found.ok_or_else(|| format!("output device not found: {}", name))?
        }
        None => host
            .default_output_device()
            .ok_or_else(|| "no default output device".to_string())?,
    };

    let supported = device
        .default_output_config()
        .map_err(|e| format!("output device config: {}", e))?;
    let device_rate = supported.sample_rate();
    let config = supported.config();
    let channels = (config.channels as usize).max(1);

    let err_cb = |e| tracing::warn!("playback stream error: {}", e);
    let stream = match supported.sample_format() {
        SampleFormat::F32 => {
            let rms = played_rms.clone();
            let dr = drained.clone();
            let ld = last_drain.clone();
            device.build_output_stream(
                config,
                move |data: &mut [f32], _| drain_to_f32(data, channels, &queue, &rms, &dr, &ld),
                err_cb,
                None,
            )
        }
        SampleFormat::I16 => {
            let queue = queue.clone();
            device.build_output_stream(
                config,
                move |data: &mut [i16], _| {
                    drain_to_i16(data, channels, &queue, &played_rms, &drained, &last_drain)
                },
                err_cb,
                None,
            )
        }
        fmt => return Err(format!("unsupported output sample format: {:?}", fmt)),
    }
    .map_err(|e| format!("build output stream: {}", e))?;

    stream
        .play()
        .map_err(|e| format!("start output stream: {}", e))?;
    Ok((stream, device_rate))
}

/// Tracks per-callback playback RMS and records when the queue last
/// emptied so `is_playing`/`played_rms`/refractory stay honest.
fn note_buffer(
    data_rms: f32,
    emptied: bool,
    played_rms: &std::sync::atomic::AtomicU32,
    drained: &tokio::sync::Notify,
    last_drain: &Mutex<Option<std::time::Instant>>,
) {
    played_rms.store(data_rms.to_bits(), Ordering::Relaxed);
    // Only a playing→empty transition starts the refractory clock and
    // wakes drain waiters — idle underrun must not.
    if emptied && data_rms > 0.0 {
        *last_drain.lock().unwrap() = Some(std::time::Instant::now());
        drained.notify_waiters();
    }
}

/// Fills the f32 output buffer from the queue: one queued sample per
/// frame replicated across `channels`, silence on underrun.
fn drain_to_f32(
    data: &mut [f32],
    channels: usize,
    queue: &Mutex<VecDeque<f32>>,
    played_rms: &std::sync::atomic::AtomicU32,
    drained: &tokio::sync::Notify,
    last_drain: &Mutex<Option<std::time::Instant>>,
) {
    let mut sum = 0.0f32;
    let mut had_audio = false;
    {
        let mut q = queue.lock().unwrap();
        for frame in data.chunks_mut(channels) {
            let s = q.pop_front().unwrap_or(0.0);
            had_audio |= s != 0.0;
            sum += s * s;
            for slot in frame.iter_mut() {
                *slot = s;
            }
        }
        let emptied = q.is_empty();
        drop(q);
        let rms = if had_audio {
            (sum / (data.len() / channels).max(1) as f32).sqrt()
        } else {
            0.0
        };
        note_buffer(rms, emptied, played_rms, drained, last_drain);
    }
}

/// Fills the i16 output buffer from the queue with f32→s16 scaling.
fn drain_to_i16(
    data: &mut [i16],
    channels: usize,
    queue: &Mutex<VecDeque<f32>>,
    played_rms: &std::sync::atomic::AtomicU32,
    drained: &tokio::sync::Notify,
    last_drain: &Mutex<Option<std::time::Instant>>,
) {
    let mut sum = 0.0f32;
    let mut had_audio = false;
    {
        let mut q = queue.lock().unwrap();
        for frame in data.chunks_mut(channels) {
            let s = q.pop_front().unwrap_or(0.0);
            had_audio |= s != 0.0;
            sum += s * s;
            let v = (s * i16::MAX as f32) as i16;
            for slot in frame.iter_mut() {
                *slot = v;
            }
        }
        let emptied = q.is_empty();
        drop(q);
        let rms = if had_audio {
            (sum / (data.len() / channels).max(1) as f32).sqrt()
        } else {
            0.0
        };
        note_buffer(rms, emptied, played_rms, drained, last_drain);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain_fixture() -> (
        std::sync::atomic::AtomicU32,
        tokio::sync::Notify,
        Mutex<Option<std::time::Instant>>,
    ) {
        (
            std::sync::atomic::AtomicU32::new(0),
            tokio::sync::Notify::new(),
            Mutex::new(None),
        )
    }

    #[test]
    fn drain_fills_channels_and_underruns_with_silence() {
        let (rms, dr, ld) = drain_fixture();
        let queue = Mutex::new(VecDeque::from(vec![0.5f32, -0.25]));
        // Stereo buffer of two frames: sample replicated, then silence.
        let mut buf = [0.0f32; 6];
        drain_to_f32(&mut buf, 2, &queue, &rms, &dr, &ld);
        assert_eq!(buf, [0.5, 0.5, -0.25, -0.25, 0.0, 0.0]);
        // The queue emptied on a buffer containing audio → drain noted.
        assert!(ld.lock().unwrap().is_some());
        assert!(f32::from_bits(rms.load(Ordering::Relaxed)) > 0.0);

        // i16 scaling: 0.5 → half of i16::MAX.
        let queue = Mutex::new(VecDeque::from(vec![0.5f32]));
        let mut buf = [0i16; 2];
        drain_to_i16(&mut buf, 1, &queue, &rms, &dr, &ld);
        assert_eq!(buf[0], i16::MAX / 2);
        assert_eq!(buf[1], 0);
    }

    #[test]
    fn idle_underrun_does_not_start_refractory() {
        let (rms, dr, ld) = drain_fixture();
        let queue = Mutex::new(VecDeque::new());
        let mut buf = [0.0f32; 4];
        drain_to_f32(&mut buf, 1, &queue, &rms, &dr, &ld);
        assert!(ld.lock().unwrap().is_none());
        assert_eq!(f32::from_bits(rms.load(Ordering::Relaxed)), 0.0);
    }
}
