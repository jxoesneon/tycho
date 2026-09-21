//! First-run bootstrap for the local OpenAI-compatible backend.
//!
//! When the configuration still points at the built-in default endpoint
//! (Ollama on localhost) with no API key, `ensure_local_backend` makes
//! "install -> launch -> works" real: it installs Ollama when missing,
//! starts `ollama serve` detached, and pulls the configured model.
//! Custom endpoints and configured API keys are treated as user-managed
//! and never touched. Every failure is reported, never fatal — the
//! pipeline keeps running and fast-path desktop intents still work.

use crate::config::GenerationConfig;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Built-in local endpoint. Auto-setup engages only while the
/// configuration points here.
pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:11434/v1/chat/completions";

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const SERVE_TIMEOUT: Duration = Duration::from_secs(20);
const SERVE_POLL: Duration = Duration::from_millis(250);

/// Serve readiness deadline; `TYCHO_SERVE_TIMEOUT_MS` overrides it
/// (tests use a short value).
fn serve_timeout() -> Duration {
    std::env::var("TYCHO_SERVE_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(SERVE_TIMEOUT)
}

/// True when stdin is an interactive terminal — the only context where
/// an installer may legitimately prompt for sudo credentials.
fn interactive() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

/// Outcome of the bootstrap attempt. Always nonfatal.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendStatus {
    /// Server reachable and the configured model is present.
    Ready,
    /// Bootstrap intentionally did nothing.
    Skipped(&'static str),
    /// Setup was attempted but could not complete.
    Unavailable(String),
}

/// `http://host:port/path` -> `http://host:port`
pub fn server_base(endpoint: &str) -> String {
    match endpoint.find("://") {
        Some(i) => match endpoint[i + 3..].find('/') {
            Some(j) => endpoint[..i + 3 + j].to_string(),
            None => endpoint.to_string(),
        },
        None => endpoint.to_string(),
    }
}

/// Locates the Ollama binary. `TYCHO_OLLAMA_BIN` overrides all lookup;
/// otherwise checks PATH, then the user-local install dir
/// (`~/.local/share/tycho/bin`) used by the tarball fallback — which
/// keeps working under the systemd unit's `ProtectHome=read-only`.
pub fn ollama_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TYCHO_OLLAMA_BIN") {
        let p = PathBuf::from(p);
        return if p.is_file() { Some(p) } else { None };
    }
    find_on_path("ollama").or_else(|| {
        let home = std::env::var_os("HOME")?;
        let p = PathBuf::from(home).join(".local/share/tycho/bin/ollama");
        is_executable(&p).then_some(p)
    })
}

/// First executable file named `name` found on PATH.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join(name);
            is_executable(&candidate).then_some(candidate)
        })
    })
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.is_file()
        && p.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

/// Parses `GET /api/tags` output; matches `model` or `model:latest`.
pub fn model_present(tags_json: &str, model: &str) -> bool {
    #[derive(serde::Deserialize)]
    struct Entry {
        name: String,
    }
    #[derive(serde::Deserialize)]
    struct Tags {
        #[serde(default)]
        models: Vec<Entry>,
    }
    let Ok(tags) = serde_json::from_str::<Tags>(tags_json) else {
        return false;
    };
    let want = if model.contains(':') {
        model.to_string()
    } else {
        format!("{model}:latest")
    };
    tags.models
        .iter()
        .any(|e| e.name == want || e.name == model)
}

async fn probe(client: &reqwest::Client, base: &str) -> Option<String> {
    let resp = client.get(format!("{base}/api/tags")).send().await.ok()?;
    if resp.status().is_success() {
        resp.text().await.ok()
    } else {
        None
    }
}

/// Installs Ollama. In an interactive session uses the system package
/// manager or the official install script (both can prompt for sudo);
/// otherwise — or when that fails — downloads the official tarball into
/// `~/.local/share/tycho`, which needs no root and stays writable under
/// the systemd unit's sandbox. `TYCHO_NO_INSTALL` disables every
/// automatic install. Public so a missing backend can be provisioned
/// ahead of launch.
pub fn install_ollama() -> Result<(), String> {
    if std::env::var_os("TYCHO_NO_INSTALL").is_some() {
        return Err("auto-install disabled via TYCHO_NO_INSTALL".to_string());
    }
    if interactive() {
        let ok = if find_on_path("pacman").is_some() {
            info!("installing ollama via pacman");
            std::process::Command::new("sudo")
                .args(["pacman", "-S", "--needed", "--noconfirm", "ollama"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        } else {
            info!("installing ollama via the official install script");
            std::process::Command::new("sh")
                .args(["-c", "curl -fsSL https://ollama.com/install.sh | sh"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        if ok && ollama_bin().is_some() {
            return Ok(());
        }
        warn!("system install unavailable — falling back to user-local install");
    }
    install_ollama_userland()
}

/// Sudo-free install: streams the official release tarball through
/// `zstd -d | tar` into `~/.local/share/tycho`, yielding `bin/ollama`.
/// `TYCHO_OLLAMA_URL` overrides the download location (tests serve a
/// small fixture instead of the ~1.4GB release).
fn install_ollama_userland() -> Result<(), String> {
    const TARBALL: &str =
        "https://github.com/ollama/ollama/releases/latest/download/ollama-linux-amd64.tar.zst";
    let url = std::env::var("TYCHO_OLLAMA_URL").unwrap_or_else(|_| TARBALL.to_string());
    let home = std::env::var_os("HOME").ok_or("HOME not set")?;
    let dest = PathBuf::from(home).join(".local/share/tycho");
    std::fs::create_dir_all(&dest).map_err(|e| format!("install dir: {e}"))?;

    info!("downloading ollama into {}", dest.display());
    let mut curl = std::process::Command::new("curl")
        .args(["-fsSL", &url])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl failed to start: {e}"))?;
    let curl_out = curl
        .stdout
        .take()
        .ok_or_else(|| "curl stdout unavailable".to_string())?;
    let mut zstd = std::process::Command::new("zstd")
        .arg("-d")
        .stdin(curl_out)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("zstd failed to start: {e}"))?;
    let zstd_out = zstd
        .stdout
        .take()
        .ok_or_else(|| "zstd stdout unavailable".to_string())?;
    let ok = std::process::Command::new("tar")
        .args(["-x", "-C"])
        .arg(&dest)
        .stdin(zstd_out)
        .status()
        .map_err(|e| format!("tar failed to start: {e}"))?
        .success();
    let _ = curl.wait();
    let _ = zstd.wait();
    if ok {
        Ok(())
    } else {
        Err("user-local install failed".to_string())
    }
}

/// Locates a usable speech engine: piper/espeak-ng/espeak/flite on
/// PATH, then the user-local piper installed under
/// `~/.local/share/tycho/piper`. `TYCHO_PIPER_BIN` overrides the piper
/// lookup specifically.
pub fn tts_engine_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TYCHO_PIPER_BIN") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    for name in ["piper", "espeak-ng", "espeak", "flite"] {
        if let Some(p) = find_on_path(name) {
            return Some(p);
        }
    }
    let home = std::env::var_os("HOME")?;
    let p = PathBuf::from(home).join(".local/share/tycho/piper/piper");
    is_executable(&p).then_some(p)
}

/// Installs a speech engine. Interactive sessions on Arch-based systems
/// try `pacman -S espeak-ng`; every context falls back to the
/// self-contained piper release tarball (binary + espeak-ng-data +
/// libs) extracted under `~/.local/share/tycho`. `TYCHO_NO_INSTALL`
/// disables every automatic install; `TYCHO_PIPER_URL` overrides the
/// download location.
pub fn install_tts_engine() -> Result<(), String> {
    if std::env::var_os("TYCHO_NO_INSTALL").is_some() {
        return Err("auto-install disabled via TYCHO_NO_INSTALL".to_string());
    }
    if interactive() && find_on_path("pacman").is_some() {
        info!("installing espeak-ng via pacman");
        let ok = std::process::Command::new("sudo")
            .args(["pacman", "-S", "--needed", "--noconfirm", "espeak-ng"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok && tts_engine_bin().is_some() {
            return Ok(());
        }
        warn!("system tts install unavailable — falling back to user-local piper");
    }
    install_piper_userland()
}

/// Sudo-free piper install: streams the release tarball through `tar`
/// into `~/.local/share/tycho`, yielding `piper/piper`. The bundle is
/// self-contained — espeak-ng-data and shared libs resolve relative to
/// the binary.
fn install_piper_userland() -> Result<(), String> {
    const TARBALL: &str =
        "https://github.com/rhasspy/piper/releases/latest/download/piper_linux_x86_64.tar.gz";
    let url = std::env::var("TYCHO_PIPER_URL").unwrap_or_else(|_| TARBALL.to_string());
    let home = std::env::var_os("HOME").ok_or("HOME not set")?;
    let dest = PathBuf::from(home).join(".local/share/tycho");
    std::fs::create_dir_all(&dest).map_err(|e| format!("install dir: {e}"))?;

    info!("downloading piper into {}", dest.display());
    let mut curl = std::process::Command::new("curl")
        .args(["-fsSL", &url])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl failed to start: {e}"))?;
    let curl_out = curl
        .stdout
        .take()
        .ok_or_else(|| "curl stdout unavailable".to_string())?;
    let ok = std::process::Command::new("tar")
        .args(["-xzf", "-", "-C"])
        .arg(&dest)
        .stdin(curl_out)
        .status()
        .map_err(|e| format!("tar failed to start: {e}"))?
        .success();
    let _ = curl.wait();
    if ok {
        Ok(())
    } else {
        Err("user-local piper install failed".to_string())
    }
}

/// Ensures the configured speech engine exists. `auto`/`piper` install
/// the piper userland bundle when nothing is available; `kokoro` and
/// `vibevoice` provision their python sidecars; system-package engines
/// (`espeak*`, `flite`) are verified and report an actionable error.
/// Returns a status for the caller to log; never fails the pipeline.
pub fn ensure_tts_engine(engine: &str, auto_setup: bool) -> BackendStatus {
    if !auto_setup {
        return BackendStatus::Skipped("auto-setup disabled in config");
    }
    match engine {
        "auto" | "piper" => {
            if tts_engine_bin().is_some() {
                return BackendStatus::Ready;
            }
            match install_tts_engine() {
                Ok(()) if tts_engine_bin().is_some() => BackendStatus::Ready,
                Ok(()) => {
                    BackendStatus::Unavailable("install finished but no engine found".to_string())
                }
                Err(e) => BackendStatus::Unavailable(e),
            }
        }
        "kokoro" | "vibevoice" => {
            if crate::tts::catalog::engine_ready(engine) {
                BackendStatus::Ready
            } else {
                match crate::tts::catalog::ensure_engine(engine) {
                    Ok(()) => BackendStatus::Ready,
                    Err(e) => BackendStatus::Unavailable(e),
                }
            }
        }
        "espeak-ng" | "espeak" | "flite" => {
            if find_on_path(engine).is_some() {
                BackendStatus::Ready
            } else {
                BackendStatus::Unavailable(format!(
                    "{engine} not installed — install it with your package manager or pick another engine"
                ))
            }
        }
        other => {
            if find_on_path(other).is_some() {
                BackendStatus::Ready
            } else {
                BackendStatus::Skipped("external tts engine — user-managed")
            }
        }
    }
}

/// Starts `ollama serve` detached into its own session, logging to the
/// Tycho state dir.
fn start_server(bin: &Path) -> Result<(), String> {
    let home = std::env::var_os("HOME").ok_or("HOME not set")?;
    let state_dir = PathBuf::from(home).join(".local/state/tycho");
    std::fs::create_dir_all(&state_dir).map_err(|e| format!("state dir: {e}"))?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state_dir.join("ollama.log"))
        .map_err(|e| format!("ollama log: {e}"))?;
    let log_err = log.try_clone().map_err(|e| e.to_string())?;

    std::process::Command::new("setsid")
        .arg(bin)
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_err)
        .spawn()
        .map_err(|e| format!("ollama serve failed to start: {e}"))?;
    Ok(())
}

async fn wait_ready(client: &reqwest::Client, base: &str) -> Option<String> {
    let deadline = Instant::now() + serve_timeout();
    while Instant::now() < deadline {
        if let Some(tags) = probe(client, base).await {
            return Some(tags);
        }
        tokio::time::sleep(SERVE_POLL).await;
    }
    None
}

async fn pull_model(bin: &Path, model: &str) -> Result<(), String> {
    info!("pulling model {model} — first download may take a while");
    let bin = bin.to_path_buf();
    let model = model.to_string();
    let status = tokio::task::spawn_blocking(move || {
        std::process::Command::new(bin)
            .arg("pull")
            .arg(model)
            .status()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("ollama pull failed to start: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("ollama pull exited with {status}"))
    }
}

/// Ensures a local generation backend is installed, serving, and has
/// `gen.model` plus any extra model pulled. Returns a status for the
/// caller to log; never fails the pipeline.
pub async fn ensure_local_backend(
    gen: &GenerationConfig,
    extra_model: Option<&str>,
) -> BackendStatus {
    if !gen.auto_setup {
        return BackendStatus::Skipped("auto-setup disabled in config");
    }
    if gen.api_key.is_some() {
        return BackendStatus::Skipped("api key configured");
    }
    if gen.endpoint != DEFAULT_ENDPOINT {
        return BackendStatus::Skipped("custom generation endpoint");
    }

    let base = server_base(&gen.endpoint);
    let client = match reqwest::Client::builder().timeout(PROBE_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => return BackendStatus::Unavailable(format!("http client: {e}")),
    };

    let tags = match probe(&client, &base).await {
        Some(t) => t,
        None => {
            info!("generation endpoint {base} is down — bootstrapping local backend");
            let bin = match ollama_bin() {
                Some(b) => Some(b),
                None => match install_ollama() {
                    Ok(()) => ollama_bin(),
                    Err(e) => {
                        warn!("ollama install failed: {e}");
                        None
                    }
                },
            };
            let Some(bin) = bin else {
                return BackendStatus::Unavailable(
                    "ollama not found and automatic install failed".to_string(),
                );
            };
            if let Err(e) = start_server(&bin) {
                return BackendStatus::Unavailable(e);
            }
            match wait_ready(&client, &base).await {
                Some(t) => t,
                None => {
                    return BackendStatus::Unavailable(format!(
                        "ollama serve did not answer within {}s",
                        serve_timeout().as_secs()
                    ))
                }
            }
        }
    };

    // Ensure each referenced model is pulled.
    let mut wanted = vec![gen.model.clone()];
    if let Some(extra) = extra_model {
        if extra != gen.model {
            wanted.push(extra.to_string());
        }
    }
    for model in wanted {
        if model_present(&tags, &model) {
            continue;
        }
        let Some(bin) = ollama_bin() else {
            return BackendStatus::Unavailable(format!(
                "model {model} not pulled and ollama binary is missing"
            ));
        };
        if let Err(e) = pull_model(&bin, &model).await {
            return BackendStatus::Unavailable(e);
        }
    }

    match probe(&client, &base).await {
        Some(_) => {
            info!("local generation backend ready at {base}");
            BackendStatus::Ready
        }
        None => BackendStatus::Unavailable("backend stopped responding".to_string()),
    }
}
