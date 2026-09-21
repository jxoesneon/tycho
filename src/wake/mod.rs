//! Wake-word gating via an openWakeWord sidecar.
//!
//! Same sidecar pattern as the TTS engines: a python venv under
//! `~/.local/share/tycho/wake` runs `oww_daemon.py`, which reads raw
//! s16le 16 kHz mono on stdin and prints one detection line per hit on
//! stdout. The coordinator feeds mic frames into the engine; a
//! detection triggers an utterance capture exactly like an orb click.
//! Provisioning (venv + `openwakeword` + model download) is the
//! automatic getter for the `wake.model` setting and honors
//! `TYCHO_NO_INSTALL`.

use crate::generation::bootstrap::find_on_path;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::info;

/// Frame bytes buffered between the run loop and the sidecar writer
/// thread. When the python side falls behind, oldest audio is dropped
/// rather than stalling capture.
const FEED_QUEUE: usize = 64;

/// Minimum interval between accepted detections; repeated hits inside
/// the window (and stale scores after an utterance) are swallowed.
const COOLDOWN: Duration = Duration::from_secs(2);

/// How long `spawn` waits for the wrapper's `READY` line. Cold python
/// plus onnxruntime import on a weak CPU takes a few seconds.
const READY_TIMEOUT: Duration = Duration::from_secs(15);

/// Models offered by the `wake.model` dropdown — the set bundled
/// inside the `openwakeword` package. `off` disables wake-word
/// gating entirely; a filesystem path to a custom `.onnx` model is
/// also accepted via `tycho.toml` even though it is not listed here.
pub const MODEL_OPTIONS: &[&str] = &[
    "off",
    "hey_jarvis",
    "alexa",
    "hey_mycroft",
    "timer",
    "weather",
];

pub fn model_options() -> Vec<String> {
    MODEL_OPTIONS.iter().map(|s| s.to_string()).collect()
}

/// True when a configured model name means "disabled".
pub fn is_disabled(model: &str) -> bool {
    model.trim().is_empty() || model.trim().eq_ignore_ascii_case("off")
}

fn wake_dir() -> Option<PathBuf> {
    crate::tts::catalog::data_dir().map(|d| d.join("wake"))
}

/// `venv/bin/python` of the openWakeWord sidecar, when provisioned.
pub fn sidecar_python() -> Option<PathBuf> {
    let p = wake_dir()?.join("venv/bin/python");
    p.is_file().then_some(p)
}

/// Path of the streaming wrapper script.
pub fn sidecar_script() -> Option<PathBuf> {
    let p = wake_dir()?.join("oww_daemon.py");
    p.is_file().then_some(p)
}

/// True when the sidecar is usable without provisioning.
pub fn sidecar_ready() -> bool {
    sidecar_python().is_some() && sidecar_script().is_some()
}

/// Provisions the openWakeWord sidecar: python venv, the
/// `openwakeword` and `onnxruntime` packages, and the streaming
/// wrapper script. A stale wrapper (from an older tycho build) is
/// rewritten; a venv whose `openwakeword` import fails is repaired.
pub fn ensure_wake() -> Result<(), String> {
    let dir = wake_dir().ok_or("HOME not set")?;
    let script_path = dir.join("oww_daemon.py");
    let script_current = std::fs::read_to_string(&script_path)
        .map(|s| s == OWW_WRAPPER)
        .unwrap_or(false);
    let venv = dir.join("venv");
    let venv_python = venv.join("bin/python");
    if venv_python.is_file() && script_current {
        return Ok(());
    }
    if std::env::var_os("TYCHO_NO_INSTALL").is_some() {
        return Err("auto-install disabled via TYCHO_NO_INSTALL".to_string());
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("wake dir: {e}"))?;
    if !venv_python.is_file() {
        let python = find_on_path("python3").ok_or("python3 required to provision openwakeword")?;
        info!("creating wake-word venv at {}", venv.display());
        run(&python, &["-m", "venv", &venv.to_string_lossy()])?;
    }
    // Import check covers venvs that exist but predate a failed or
    // interrupted package install.
    let import_ok = Command::new(&venv_python)
        .args(["-c", "import openwakeword"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !import_ok {
        let pip = venv.join("bin/pip");
        info!("installing openwakeword into {}", venv.display());
        run(&pip, &["install", "--quiet", "openwakeword", "onnxruntime"])?;
    }
    std::fs::write(&script_path, OWW_WRAPPER).map_err(|e| format!("wake wrapper: {e}"))?;
    Ok(())
}

fn run(cmd: &Path, args: &[&str]) -> Result<(), String> {
    let status = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .status()
        .map_err(|e| format!("{} failed to start: {e}", cmd.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} exited with {status}", cmd.display()))
    }
}

/// A live sidecar: frames go in via `feed_f32`, detections come out
/// via `try_detect`. Killing the struct kills the python child.
pub struct WakeWordEngine {
    feed_tx: mpsc::SyncSender<Vec<u8>>,
    // `mpsc::Receiver` is Send but not Sync; the coordinator's
    // `run_forever` future must be Sync-compatible across awaits,
    // so detection lines funnel through a mutex.
    detect_rx: Mutex<mpsc::Receiver<String>>,
    child: Child,
    cooldown_until: Option<Instant>,
}

impl WakeWordEngine {
    /// Spawns the provisioned sidecar for `model` at `threshold`,
    /// waiting for the wrapper's `READY` line so a child that dies
    /// during python import reports an error instead of looking live.
    /// `vad_gate` enables the bundled Silero speech gate (0 disables).
    pub fn spawn(model: &str, threshold: f32, vad_gate: f32) -> std::io::Result<Self> {
        let py = sidecar_python().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "wake sidecar python")
        })?;
        let script = sidecar_script().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "wake sidecar script")
        })?;
        Self::spawn_inner(
            &py,
            &[
                script.to_string_lossy().to_string(),
                "--model".to_string(),
                model.to_string(),
                "--threshold".to_string(),
                format!("{threshold:.2}"),
                "--vad-gate".to_string(),
                format!("{vad_gate:.2}"),
            ],
            true,
        )
    }

    /// Spawns `cmd` with `args` as the detection process — the seam
    /// tests use to inject a shim without a real venv.
    pub fn spawn_with(cmd: &Path, args: &[String]) -> std::io::Result<Self> {
        Self::spawn_inner(cmd, args, false)
    }

    fn spawn_inner(cmd: &Path, args: &[String], await_ready: bool) -> std::io::Result<Self> {
        let stderr = wake_log().map(Stdio::from).unwrap_or(Stdio::null());
        let mut child = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("wake sidecar stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("wake sidecar stdout unavailable"))?;

        let (feed_tx, feed_rx) = mpsc::sync_channel::<Vec<u8>>(FEED_QUEUE);
        let (detect_tx, detect_rx) = mpsc::channel::<String>();
        let (ready_tx, ready_rx) = mpsc::channel::<String>();

        std::thread::spawn(move || {
            for buf in feed_rx.iter() {
                if stdin.write_all(&buf).is_err() {
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(name) = line.strip_prefix("READY ") {
                    let _ = ready_tx.send(name.to_string());
                    continue;
                }
                if detect_tx.send(line).is_err() {
                    break;
                }
            }
        });

        if await_ready {
            let deadline = Instant::now() + READY_TIMEOUT;
            loop {
                if ready_rx.try_recv().is_ok() {
                    break;
                }
                if child.try_wait()?.is_some() {
                    return Err(std::io::Error::other(
                        "wake sidecar exited during startup (see wake.log)",
                    ));
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(std::io::Error::other(
                        "wake sidecar produced no READY signal",
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        Ok(Self {
            feed_tx,
            detect_rx: Mutex::new(detect_rx),
            child,
            cooldown_until: None,
        })
    }

    /// Queues one capture frame for the sidecar. Resamples to 16 kHz
    /// when the capture rate differs; drops frames when the python
    /// side is behind rather than blocking the audio loop.
    pub fn feed_f32(&mut self, frame: &[f32], sample_rate: u32) {
        let pcm: Vec<f32> = if sample_rate == 16000 {
            frame.to_vec()
        } else {
            crate::audio::resample_linear(frame, sample_rate, 16000)
        };
        let mut bytes = Vec::with_capacity(pcm.len() * 2);
        for s in pcm {
            let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let _ = self.feed_tx.try_send(bytes);
    }

    /// Next pending detection line, if any.
    pub fn try_detect(&self) -> Option<String> {
        self.detect_rx.lock().ok()?.try_recv().ok()
    }

    /// Swallows every queued detection — used when an utterance
    /// finishes so stale scores cannot retrigger immediately.
    pub fn drain(&self) {
        if let Ok(rx) = self.detect_rx.lock() {
            while rx.try_recv().is_ok() {}
        }
    }

    /// True while the post-detection cooldown is active.
    pub fn cooling_down(&self) -> bool {
        self.cooldown_until
            .map(|t| Instant::now() < t)
            .unwrap_or(false)
    }

    /// Starts the cooldown window after an accepted detection.
    pub fn note_trigger(&mut self) {
        self.cooldown_until = Some(Instant::now() + COOLDOWN);
    }
}

impl Drop for WakeWordEngine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// stderr of the sidecar is appended to the tycho state log so wrapper
/// failures are diagnosable without polluting stdout (which carries
/// detection lines).
fn wake_log() -> Option<std::fs::File> {
    let home = std::env::var_os("HOME")?;
    let dir = PathBuf::from(home).join(".local/state/tycho");
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("wake.log"))
        .ok()
}

/// Streaming wrapper: reads 80 ms (1280-sample) s16le chunks on stdin,
/// prints `READY <model>` once inference is loaded, then one
/// `<model> <score>` line per hit above `--threshold`. `--vad-gate`
/// enables openWakeWord's bundled Silero VAD so detections only count
/// when actual speech is present — the documented fix for false
/// accepts on non-speech noise. `--speex` requests speexdsp noise
/// suppression when the optional `speexdsp_ns` package is importable.
const OWW_WRAPPER: &str = r#"import argparse, glob, os, sys
import numpy as np

p = argparse.ArgumentParser()
p.add_argument("--model", required=True)
p.add_argument("--threshold", type=float, default=0.5)
p.add_argument("--vad-gate", type=float, default=0.5)
p.add_argument("--speex", action="store_true")
a = p.parse_args()

import openwakeword
from openwakeword.model import Model

if os.path.isfile(a.model):
    paths = [a.model]
elif a.model in openwakeword.models:
    paths = [openwakeword.models[a.model]["model_path"]]
else:
    res = os.path.join(os.path.dirname(openwakeword.__file__), "resources", "models")
    hits = sorted(glob.glob(os.path.join(res, a.model + "*.onnx")))
    if not hits:
        sys.exit(f"unknown wake model '{a.model}': not a bundled name or .onnx path")
    paths = hits

kw = {}
if a.vad_gate > 0.0:
    kw["vad_threshold"] = a.vad_gate
if a.speex:
    import importlib.util
    if importlib.util.find_spec("speexdsp_ns"):
        kw["enable_speex_noise_suppression"] = True
    else:
        print("speexdsp_ns unavailable; noise suppression disabled", file=sys.stderr, flush=True)
m = Model(wakeword_model_paths=paths, **kw)
print(f"READY {a.model}", flush=True)

CHUNK = 1280 * 2  # 80 ms of s16le 16 kHz
while True:
    buf = sys.stdin.buffer.read(CHUNK)
    if not buf or len(buf) < CHUNK:
        break
    audio = np.frombuffer(buf, dtype=np.int16)
    for name, score in m.predict(audio).items():
        if score >= a.threshold:
            print(f"{name} {score:.3f}", flush=True)
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_values() {
        assert!(is_disabled("off"));
        assert!(is_disabled("Off"));
        assert!(is_disabled(""));
        assert!(is_disabled("  "));
        assert!(!is_disabled("hey_jarvis"));
    }

    #[test]
    fn options_include_off_and_pretrained() {
        let v = model_options();
        assert_eq!(v[0], "off");
        assert!(v.iter().any(|x| x == "hey_jarvis"));
        assert!(v.iter().any(|x| x == "alexa"));
    }

    #[test]
    fn engine_spawn_feed_detect_kill() {
        // Shim emits a detection line periodically; stdin is ignored.
        let dir = std::env::temp_dir().join(format!("tycho-wake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shim = dir.join("shim.sh");
        std::fs::write(
            &shim,
            "#!/bin/sh\nwhile true; do echo 'hey_jarvis 0.99'; sleep 0.05; done\n",
        )
        .unwrap();
        let mut eng =
            WakeWordEngine::spawn_with(Path::new("/bin/sh"), &[shim.to_string_lossy().to_string()])
                .unwrap();
        eng.feed_f32(&[0.0f32; 512], 16000);
        let hit = (0..100).find_map(|_| {
            std::thread::sleep(Duration::from_millis(10));
            eng.try_detect()
        });
        assert!(hit.unwrap().contains("hey_jarvis"));
        eng.drain();
        assert!(eng.try_detect().is_none());
        drop(eng); // Drop kills the shim child.
    }

    #[test]
    fn no_install_gate_refuses_provisioning() {
        // HOME → empty temp dir so the sidecar is definitely absent,
        // then TYCHO_NO_INSTALL must produce an honest refusal.
        let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("tycho-wake-noinst-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old_home = std::env::var_os("HOME");
        std::env::set_var("HOME", &dir);
        std::env::set_var("TYCHO_NO_INSTALL", "1");
        let res = ensure_wake();
        std::env::remove_var("TYCHO_NO_INSTALL");
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(res.unwrap_err().contains("TYCHO_NO_INSTALL"));
    }

    #[test]
    fn cooldown_window() {
        let dir = std::env::temp_dir().join(format!("tycho-wake-cd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shim = dir.join("shim.sh");
        std::fs::write(&shim, "#!/bin/sh\nsleep 60\n").unwrap();
        let mut eng =
            WakeWordEngine::spawn_with(Path::new("/bin/sh"), &[shim.to_string_lossy().to_string()])
                .unwrap();
        assert!(!eng.cooling_down());
        eng.note_trigger();
        assert!(eng.cooling_down());
    }
}
