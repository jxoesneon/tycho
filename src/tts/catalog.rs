//! Catalog of supported speech engines and voices plus their
//! "auto-getters": selecting an option that is not on the system
//! triggers a best-effort provision (voice download, userland bundle,
//! or a python venv sidecar). Everything here is synchronous and
//! intended to run inside `tokio::task::spawn_blocking` or during
//! startup bootstrap; `TYCHO_NO_INSTALL` disables every install.

use crate::generation::bootstrap::find_on_path;
use std::path::{Path, PathBuf};
use tracing::info;

/// Engines offered in the settings dropdown, in display order.
/// `auto` resolves to the best available engine at synth time.
pub const ENGINE_OPTIONS: &[&str] = &[
    "auto",
    "piper",
    "kokoro",
    "vibevoice",
    "espeak-ng",
    "espeak",
    "flite",
];

/// Piper voices offered for download even when absent from the model
/// cache, from `rhasspy/piper-voices` revision `v1.0.0`.
pub const PIPER_VOICE_OPTIONS: &[&str] = &[
    "en_US-amy-medium",
    "en_US-lessac-high",
    "en_US-libritts-high",
    "en_US-hfc_female-medium",
    "en_US-hfc_male-medium",
    "en_US-ryan-high",
    "en_GB-alan-medium",
];

/// Kokoro presets shipped inside its bundled voices file.
pub const KOKORO_VOICE_OPTIONS: &[&str] = &[
    "af_heart",
    "af_bella",
    "am_adam",
    "am_michael",
    "bf_emma",
    "bm_george",
];

/// `~/.local/share/tycho` — userland installs live here so they keep
/// working without root and under the systemd unit's sandbox.
pub fn data_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/tycho"))
}

/// `<cache>/tts` — the directory piper voice files and kokoro model
/// files are stored in.
pub fn tts_dir(cache_dir: &Path) -> PathBuf {
    crate::models::manager::expand_tilde(cache_dir).join("tts")
}

/// Piper voices installed in the model cache (`<voice>.onnx` stems that
/// follow the piper naming convention).
pub fn installed_piper_voices(tts_dir: &Path) -> Vec<String> {
    let mut voices: Vec<String> = std::fs::read_dir(tts_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let p = e.path();
                    let stem = p.file_stem()?.to_str()?.to_string();
                    if p.extension().and_then(|x| x.to_str()) == Some("onnx")
                        && piper_voice_repo_path(&stem).is_some()
                    {
                        Some(stem)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    voices.sort();
    voices
}

/// Full voice dropdown list: installed piper voices first, then
/// downloadable piper voices and bundled kokoro presets. The
/// configured voice is added by `SettingEntry::values()` when custom.
pub fn voice_options(cache_dir: &Path) -> Vec<String> {
    let mut v = installed_piper_voices(&tts_dir(cache_dir));
    for o in PIPER_VOICE_OPTIONS.iter().chain(KOKORO_VOICE_OPTIONS) {
        if !v.iter().any(|x| x == o) {
            v.push(o.to_string());
        }
    }
    v
}

/// Engine dropdown list (every supported engine, installed or not).
pub fn engine_options() -> Vec<String> {
    ENGINE_OPTIONS.iter().map(|s| s.to_string()).collect()
}

/// Maps `en_US-amy-medium` -> `en/en_US/amy/medium/en_US-amy-medium.onnx`,
/// the layout inside `rhasspy/piper-voices`. Returns `None` for names
/// that do not follow the `<lang>_<REGION>-<speaker>-<quality>` shape.
pub fn piper_voice_repo_path(voice: &str) -> Option<String> {
    let (locale, rest) = voice.split_once('-')?;
    let (lang, region) = locale.split_once('_')?;
    if lang.len() != 2 || region.len() != 2 {
        return None;
    }
    let (speaker, quality) = rest.rsplit_once('-')?;
    if speaker.is_empty() || quality.is_empty() {
        return None;
    }
    Some(format!("{lang}/{locale}/{speaker}/{quality}/{voice}.onnx"))
}

/// Kokoro preset names look like `af_heart` / `am_adam`: a two-letter
/// language prefix, underscore, lowercase name.
pub fn is_kokoro_voice(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() >= 4
        && b[..2].iter().all(|c| c.is_ascii_lowercase())
        && b[2] == b'_'
        && b[3..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// User-local piper binary: `TYCHO_PIPER_BIN`, PATH, then the
/// bootstrap bundle under `~/.local/share/tycho/piper`.
pub fn piper_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TYCHO_PIPER_BIN") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    find_on_path("piper").or_else(|| {
        let p = data_dir()?.join("piper/piper");
        p.is_file().then_some(p)
    })
}

pub fn kokoro_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("kokoro"))
}

/// `venv/bin/python` of the kokoro sidecar, when provisioned.
pub fn kokoro_python() -> Option<PathBuf> {
    let p = kokoro_dir()?.join("venv/bin/python");
    p.is_file().then_some(p)
}

/// The wrapper script that turns kokoro inference into WAV on stdout.
pub fn kokoro_script() -> Option<PathBuf> {
    let p = kokoro_dir()?.join("kokoro_tts.py");
    p.is_file().then_some(p)
}

pub fn vibevoice_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("vibevoice"))
}

pub fn vibevoice_python() -> Option<PathBuf> {
    let p = vibevoice_dir()?.join("venv/bin/python");
    p.is_file().then_some(p)
}

pub fn vibevoice_script() -> Option<PathBuf> {
    let p = vibevoice_dir()?.join("vibevoice_tts.py");
    p.is_file().then_some(p)
}

/// Minimum plausible sizes for managed kokoro assets. Pre-allocated
/// download stubs (`ONNX_WEIGHT_CONTAINER_V1` containers) exist in the
/// wild — a size floor keeps them from counting as real models.
const KOKORO_MODEL_MIN_BYTES: u64 = 32 * 1024 * 1024;
const KOKORO_VOICES_MIN_BYTES: u64 = 64 * 1024;

/// First `kokoro*.onnx` model inside the tts cache that clears the
/// size floor (stub/partial downloads are ignored).
pub fn find_kokoro_model(tts_dir: &Path) -> Option<PathBuf> {
    find_prefixed(tts_dir, "kokoro", "onnx", KOKORO_MODEL_MIN_BYTES)
}

/// First `voices*.bin` file inside the tts cache (kokoro voice bank)
/// that clears the size floor.
pub fn find_kokoro_voices(tts_dir: &Path) -> Option<PathBuf> {
    find_prefixed(tts_dir, "voices", "bin", KOKORO_VOICES_MIN_BYTES)
}

fn find_prefixed(dir: &Path, prefix: &str, ext: &str, min_bytes: u64) -> Option<PathBuf> {
    let mut hits: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|x| x.to_str()) == Some(ext)
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(prefix))
                    .unwrap_or(false)
                && p.metadata().map(|m| m.len() >= min_bytes).unwrap_or(false)
        })
        .collect();
    hits.sort();
    hits.into_iter().next()
}

/// True when the named engine is usable right now without any
/// provisioning. `auto` is ready when anything at all can synthesize.
pub fn engine_ready(engine: &str) -> bool {
    match engine {
        "auto" => {
            crate::generation::bootstrap::tts_engine_bin().is_some()
                || (kokoro_python().is_some() && kokoro_script().is_some())
        }
        "piper" => piper_bin().is_some(),
        "kokoro" => kokoro_python().is_some() && kokoro_script().is_some(),
        "vibevoice" => vibevoice_python().is_some() && vibevoice_script().is_some(),
        other => find_on_path(other).is_some(),
    }
}

/// Provisions whatever the current engine+voice selection requires.
/// Errors are aggregated and reported honestly — a missing engine or
/// voice never silently downgrades.
pub fn ensure_selection(engine: &str, voice: &str, tts_dir: &Path) -> Result<(), String> {
    let mut errs = Vec::new();
    if let Err(e) = ensure_engine(engine) {
        errs.push(format!("engine {engine}: {e}"));
    }
    if let Err(e) = ensure_voice(engine, voice, tts_dir) {
        errs.push(format!("voice {voice}: {e}"));
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("; "))
    }
}

/// Ensures the named engine exists, provisioning it when a getter is
/// available. System-package engines report an actionable error.
pub fn ensure_engine(engine: &str) -> Result<(), String> {
    if engine_ready(engine) {
        return Ok(());
    }
    match engine {
        "auto" | "piper" => crate::generation::bootstrap::install_tts_engine()
            .map_err(|e| format!("piper install: {e}")),
        "kokoro" => provision_kokoro(),
        "vibevoice" => provision_vibevoice(),
        "espeak-ng" | "espeak" | "flite" => Err(format!(
            "{engine} requires a system package — install it with your package manager"
        )),
        other => Err(format!(
            "{other} is an external engine — install it on PATH yourself"
        )),
    }
}

/// Ensures the named voice exists for engines that consume Tycho-managed
/// voices. System engines have no managed voices and pass through.
fn ensure_voice(engine: &str, voice: &str, tts_dir: &Path) -> Result<(), String> {
    match engine {
        "kokoro" => ensure_kokoro_models(tts_dir),
        "piper" | "auto" => {
            if is_kokoro_voice(voice) {
                provision_kokoro().and_then(|_| ensure_kokoro_models(tts_dir))
            } else {
                ensure_piper_voice(voice, tts_dir)
            }
        }
        _ => Ok(()),
    }
}

/// Downloads `<voice>.onnx` + `.onnx.json` from `rhasspy/piper-voices`
/// into the tts cache when absent.
fn ensure_piper_voice(voice: &str, tts_dir: &Path) -> Result<(), String> {
    let model = tts_dir.join(format!("{voice}.onnx"));
    let meta = tts_dir.join(format!("{voice}.onnx.json"));
    if model.is_file() && meta.is_file() {
        return Ok(());
    }
    let Some(repo_path) = piper_voice_repo_path(voice) else {
        return Err(format!("no automatic getter for voice '{voice}'"));
    };
    no_install()?;
    std::fs::create_dir_all(tts_dir).map_err(|e| format!("tts dir: {e}"))?;
    for (repo_file, dest) in [
        (repo_path.clone(), model),
        (format!("{repo_path}.json"), meta),
    ] {
        if dest.is_file() {
            continue;
        }
        let url = format!("https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/{repo_file}");
        info!("fetching piper voice: {url}");
        download(&url, &dest)?;
    }
    Ok(())
}

/// Ensures kokoro model files exist in the tts cache; downloads the
/// official `model-files-v1.0` release assets when absent.
fn ensure_kokoro_models(tts_dir: &Path) -> Result<(), String> {
    if find_kokoro_model(tts_dir).is_some() && find_kokoro_voices(tts_dir).is_some() {
        return Ok(());
    }
    no_install()?;
    std::fs::create_dir_all(tts_dir).map_err(|e| format!("tts dir: {e}"))?;
    const BASE: &str =
        "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0";
    for (name, dest, min) in [
        (
            "kokoro-v1.0.onnx",
            tts_dir.join("kokoro-v1.0.onnx"),
            KOKORO_MODEL_MIN_BYTES,
        ),
        (
            "voices-v1.0.bin",
            tts_dir.join("voices-v1.0.bin"),
            KOKORO_VOICES_MIN_BYTES,
        ),
    ] {
        let ok = dest.metadata().map(|m| m.len() >= min).unwrap_or(false);
        if ok {
            continue;
        }
        // Clear a stub/partial file occupying the destination name.
        let _ = std::fs::remove_file(&dest);
        let url = format!("{BASE}/{name}");
        info!("fetching kokoro asset: {url}");
        download(&url, &dest)?;
    }
    Ok(())
}

fn no_install() -> Result<(), String> {
    if std::env::var_os("TYCHO_NO_INSTALL").is_some() {
        Err("auto-install disabled via TYCHO_NO_INSTALL".to_string())
    } else {
        Ok(())
    }
}

fn download(url: &str, dest: &Path) -> Result<(), String> {
    let tmp = dest.with_extension("part");
    let ok = std::process::Command::new("curl")
        .args(["-fsSL", url, "-o"])
        .arg(&tmp)
        .status()
        .map_err(|e| format!("curl failed to start: {e}"))?
        .success();
    if ok {
        std::fs::rename(&tmp, dest).map_err(|e| format!("rename: {e}"))
    } else {
        let _ = std::fs::remove_file(&tmp);
        Err(format!("download failed: {url}"))
    }
}

/// Runs `cmd` with args, requiring a zero exit status.
fn run(cmd: &Path, args: &[&str]) -> Result<(), String> {
    let status = std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("{} failed to start: {e}", cmd.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} exited with {status}", cmd.display()))
    }
}

/// Kokoro sidecar: python venv with `kokoro-onnx` + `soundfile`, plus a
/// wrapper script that writes WAV to stdout. Model files live in the
/// shared tts cache and are fetched by `ensure_kokoro_models`.
fn provision_kokoro() -> Result<(), String> {
    if kokoro_python().is_some() && kokoro_script().is_some() {
        return Ok(());
    }
    no_install()?;
    let dir = kokoro_dir().ok_or("HOME not set")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("kokoro dir: {e}"))?;
    let python = find_on_path("python3").ok_or("python3 required to provision kokoro")?;
    let venv = dir.join("venv");
    if !venv.join("bin/python").is_file() {
        info!("creating kokoro venv at {}", venv.display());
        run(&python, &["-m", "venv", &venv.to_string_lossy()])?;
    }
    let pip = venv.join("bin/pip");
    info!("installing kokoro-onnx into {}", venv.display());
    run(&pip, &["install", "--quiet", "kokoro-onnx", "soundfile"])?;
    std::fs::write(dir.join("kokoro_tts.py"), KOKORO_WRAPPER)
        .map_err(|e| format!("kokoro wrapper: {e}"))?;
    Ok(())
}

/// VibeVoice sidecar: clones the upstream repo, installs it into a
/// venv, and wraps its own file-inference demo script so the heavy
/// model API stays upstream-owned. Expect a multi-GB install.
fn provision_vibevoice() -> Result<(), String> {
    if vibevoice_python().is_some() && vibevoice_script().is_some() {
        return Ok(());
    }
    no_install()?;
    let dir = vibevoice_dir().ok_or("HOME not set")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("vibevoice dir: {e}"))?;
    let python = find_on_path("python3").ok_or("python3 required to provision vibevoice")?;
    let git = find_on_path("git").ok_or("git required to provision vibevoice")?;
    let repo = dir.join("repo");
    if !repo.join("pyproject.toml").is_file() {
        info!("cloning microsoft/VibeVoice into {}", repo.display());
        run(
            &git,
            &[
                "clone",
                "--depth",
                "1",
                "https://github.com/microsoft/VibeVoice",
                &repo.to_string_lossy(),
            ],
        )?;
    }
    let venv = dir.join("venv");
    if !venv.join("bin/python").is_file() {
        info!("creating vibevoice venv at {}", venv.display());
        run(&python, &["-m", "venv", &venv.to_string_lossy()])?;
    }
    let pip = venv.join("bin/pip");
    info!("installing vibevoice (multi-GB) into {}", venv.display());
    run(&pip, &["install", "--quiet", "-e", &repo.to_string_lossy()])?;
    std::fs::write(dir.join("vibevoice_tts.py"), VIBEVOICE_WRAPPER)
        .map_err(|e| format!("vibevoice wrapper: {e}"))?;
    Ok(())
}

/// Kokoro wrapper: reads text + paths from argv, writes WAV to stdout.
const KOKORO_WRAPPER: &str = r#"import argparse, io, sys
import numpy as np
import soundfile as sf
from kokoro_onnx import Kokoro

p = argparse.ArgumentParser()
p.add_argument("--model", required=True)
p.add_argument("--voices", required=True)
p.add_argument("--voice", default="af_heart")
p.add_argument("--speed", type=float, default=1.0)
p.add_argument("--text", required=True)
a = p.parse_args()

k = Kokoro(a.model, a.voices)
samples, rate = k.create(a.text, voice=a.voice, speed=a.speed, lang="en-us")
buf = io.BytesIO()
sf.write(buf, np.asarray(samples, dtype=np.float32), rate, format="WAV")
sys.stdout.buffer.write(buf.getvalue())
"#;

/// VibeVoice wrapper: delegates to the repo's own file-inference demo
/// script so the model API stays upstream-owned, then streams the
/// newest produced .wav on stdout.
const VIBEVOICE_WRAPPER: &str = r#"import argparse, glob, os, subprocess, sys, tempfile

p = argparse.ArgumentParser()
p.add_argument("--repo", required=True)
p.add_argument("--model", default="microsoft/VibeVoice-Realtime-0.5B")
p.add_argument("--speaker", default="Carter")
p.add_argument("--text", required=True)
a = p.parse_args()

with tempfile.TemporaryDirectory() as d:
    txt = os.path.join(d, "in.txt")
    with open(txt, "w") as f:
        f.write(a.text)
    out_dir = os.path.join(d, "out")
    demo = os.path.join(a.repo, "demo", "realtime_model_inference_from_file.py")
    subprocess.run(
        [sys.executable, demo, "--model_path", a.model,
         "--txt_path", txt, "--speaker_name", a.speaker,
         "--output_dir", out_dir],
        check=True,
    )
    wavs = sorted(
        glob.glob(os.path.join(out_dir, "**", "*.wav"), recursive=True),
        key=os.path.getmtime,
    )
    if not wavs:
        sys.exit("vibevoice produced no audio")
    sys.stdout.buffer.write(open(wavs[-1], "rb").read())
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tycho-catalog-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn piper_repo_path_derivation() {
        assert_eq!(
            piper_voice_repo_path("en_US-amy-medium").as_deref(),
            Some("en/en_US/amy/medium/en_US-amy-medium.onnx")
        );
        assert_eq!(
            piper_voice_repo_path("en_GB-alan-medium").as_deref(),
            Some("en/en_GB/alan/medium/en_GB-alan-medium.onnx")
        );
        assert_eq!(
            piper_voice_repo_path("en_US-libritts_r-medium").as_deref(),
            Some("en/en_US/libritts_r/medium/en_US-libritts_r-medium.onnx")
        );
        assert!(piper_voice_repo_path("af_heart").is_none());
        assert!(piper_voice_repo_path("amy").is_none());
        assert!(piper_voice_repo_path("").is_none());
    }

    #[test]
    fn kokoro_voice_shape() {
        assert!(is_kokoro_voice("af_heart"));
        assert!(is_kokoro_voice("am_adam"));
        assert!(!is_kokoro_voice("en_US-amy-medium"));
        assert!(!is_kokoro_voice("Carter"));
        assert!(!is_kokoro_voice("a_b"));
    }

    #[test]
    fn engine_options_lists_everything() {
        let v = engine_options();
        for e in [
            "auto",
            "piper",
            "kokoro",
            "vibevoice",
            "espeak-ng",
            "espeak",
            "flite",
        ] {
            assert!(v.iter().any(|x| x == e), "missing {e}");
        }
    }

    #[test]
    fn voice_options_merge_installed_and_known() {
        let tmp = tmpdir("voice-opts");
        let tts = tmp.join("tts");
        std::fs::create_dir_all(&tts).unwrap();
        std::fs::write(tts.join("en_US-custom-medium.onnx"), b"x").unwrap();
        // non-voice files must not leak into the list
        std::fs::write(tts.join("kokoro-v1.0.onnx"), b"x").unwrap();
        std::fs::write(tts.join("laya_intent_classifier.onnx"), b"x").unwrap();
        let v = voice_options(&tmp);
        assert!(v.iter().any(|x| x == "en_US-custom-medium"));
        assert!(v.iter().any(|x| x == "en_US-lessac-high"));
        assert!(v.iter().any(|x| x == "af_heart"));
        assert!(!v.iter().any(|x| x == "kokoro-v1.0"));
        assert!(!v.iter().any(|x| x == "laya_intent_classifier"));
        // no duplicates
        let mut sorted = v.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), v.len());
    }

    #[test]
    fn ensure_voice_no_install_errors() {
        let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
        std::env::set_var("TYCHO_NO_INSTALL", "1");
        let tmp = tmpdir("no-install");
        let tts = tmp.join("tts");
        let r = ensure_piper_voice("en_US-lessac-high", &tts);
        std::env::remove_var("TYCHO_NO_INSTALL");
        assert!(r.unwrap_err().contains("TYCHO_NO_INSTALL"));
    }

    #[test]
    fn ensure_voice_unknown_name_errors() {
        let tmp = tmpdir("unknown-voice");
        let r = ensure_piper_voice("not-a-voice", &tmp.join("tts"));
        assert!(r.unwrap_err().contains("no automatic getter"));
    }

    #[test]
    fn system_engine_missing_reports_package_hint() {
        if find_on_path("espeak-ng").is_some() {
            assert!(ensure_engine("espeak-ng").is_ok());
        } else {
            let e = ensure_engine("espeak-ng").unwrap_err();
            assert!(e.contains("package manager"), "{e}");
        }
    }

    #[test]
    fn unknown_engine_reports_external() {
        let e = ensure_engine("definitely-not-a-tts-engine").unwrap_err();
        assert!(e.contains("external engine"), "{e}");
    }
}
