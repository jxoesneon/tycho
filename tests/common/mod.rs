//! Shared test helpers.
//!
//! Each test binary compiles this module independently; not every helper is
//! used by every binary.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpListener;

/// Spawns a single-purpose HTTP server on a random localhost port that
/// answers every request with `body` and HTTP `status`. Returns the base
/// URL (e.g. `http://127.0.0.1:PORT`). The server runs until the test
/// process exits.
pub fn http_server(body: &'static [u8], status: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut req = [0u8; 8192];
            let _ = stream.read(&mut req);
            let response = format!(
                "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                status,
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(body);
        }
    });
    format!("http://127.0.0.1:{}", port)
}

/// Serves `bytes` forever with HTTP 200 — used to emulate the HuggingFace
/// resolve endpoint.
pub fn hf_server(bytes: &'static [u8]) -> String {
    http_server(bytes, "200 OK")
}

/// Synthetic model weights payload large enough to satisfy the default
/// spec floors below STT (tts = 1 MB, router = 512 KB, voices = 512 B).
/// Tests that download the STT spec must pin `expected_min_bytes` low —
/// see `test_models_config` — so this fixture stays small on tmpfs.
pub fn model_weights() -> &'static [u8] {
    Box::leak(vec![0xABu8; 1_572_864].into_boxed_slice())
}

/// `ModelsConfig` whose STT size floor fits the `model_weights` fixture;
/// every other spec floor is already satisfied by it. Use as the
/// struct-update base for configs that download via `hf_server`.
pub fn test_models_config() -> rust_voice_assistant::config::ModelsConfig {
    let mut config = rust_voice_assistant::config::ModelsConfig::default();
    config.stt_model.expected_min_bytes = 1024;
    // The router spec is opt-in by default; fixtures enable it to keep
    // the optional-download path exercised.
    config.router_model = Some(rust_voice_assistant::config::ModelSpecConfig {
        repo_id: "convaiinnovations/laya".to_string(),
        revision: "main".to_string(),
        filename: "model.onnx".to_string(),
        target_filename: "laya_intent_classifier.onnx".to_string(),
        expected_min_bytes: 512,
        subfolder: None,
    });
    config
}

/// Temp dir removed on drop — fixture caches are large, so leaked dirs
/// would quickly exhaust tmpfs.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: impl AsRef<str>) -> Self {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        // CARGO_TARGET_TMPDIR lives under target/ on disk, not tmpfs —
        // multi-MB fixture caches must not exhaust /tmp.
        let base = option_env!("CARGO_TARGET_TMPDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = base.join(format!(
            "tycho-{}-{}-{}",
            tag.as_ref(),
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Builds a `TychoConfig` whose model cache is a fresh temp dir pre-seeded
/// with stub model files — `ensure_models` treats them as present so no
/// network is needed. Returns `(config, cache_dir)`; the dir is removed
/// when the guard drops.
pub fn seeded_cache_config(tag: &str) -> (rust_voice_assistant::config::TychoConfig, TempDir) {
    let dir = TempDir::new(format!("models-{}", tag));
    let mut config = rust_voice_assistant::config::TychoConfig::default();
    let stt = dir
        .join("stt")
        .join(&config.models.stt_model.target_filename);
    let tts = dir
        .join("tts")
        .join(&config.models.tts_model.target_filename);
    let voices = dir
        .join("tts")
        .join(&config.models.tts_voices.target_filename);
    for p in [&stt, &tts, &voices] {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"stub-model-bytes").unwrap();
    }
    if let Some(ref spec) = config.models.router_model {
        let router = dir.join("router").join(&spec.target_filename);
        std::fs::create_dir_all(router.parent().unwrap()).unwrap();
        std::fs::write(&router, b"stub-model-bytes").unwrap();
    }
    config.models.cache_dir = dir.0.clone();
    // Keep persistent memory inside the temp dir — tests must not write
    // to the real ~/.tycho/memory store.
    config.memory.db_path = dir.join("memory.jsonl");
    // Never open the orb overlay during tests.
    config.ui.orb = false;
    // Never bootstrap a local LLM backend during tests.
    config.generation.auto_setup = false;
    // Point the neural router at a dead port by default — a test that
    // forgets to stub it must not leak queries to a live local model.
    config.router.jev_endpoint = "http://127.0.0.1:1".to_string();
    (config, dir)
}

/// Serves a response that advertises `declared_len` bytes but closes the
/// connection after `body` — exercises mid-stream truncation errors.
pub fn truncated_server(body: &'static [u8], declared_len: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut req = [0u8; 8192];
            let _ = stream.read(&mut req);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                declared_len
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(body);
        }
    });
    format!("http://127.0.0.1:{}", port)
}

/// Serves a canned OpenAI-compatible chat completion response.
pub fn openai_server(content: &str) -> String {
    let body: &'static [u8] = Box::leak(
        serde_json::json!({
            "choices": [{
                "message": {"role": "assistant", "content": content}
            }]
        })
        .to_string()
        .into_bytes()
        .into_boxed_slice(),
    );
    http_server(body, "200 OK")
}

/// Serves an OpenAI-compatible response whose message content is a JEV
/// verdict: `{"option": <option>, "confidence": <confidence>}`.
pub fn jev_server(option: &str, confidence: f64) -> String {
    openai_server(&serde_json::json!({"option": option, "confidence": confidence}).to_string())
}

// ---- Fake Hyprland Unix-socket IPC ----------------------------------------

use std::path::{Path, PathBuf};

#[cfg(feature = "hyprland")]
mod hypr_fakes {
    use super::{Path, PathBuf};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    pub const HYPR_ACTIVE_WINDOW: &str = r#"{"address":"0xdeadbeef","title":"Terminal","class":"kitty","workspace":{"id":2},"floating":false,"fullscreen":0,"at":[10,20],"size":[800,600],"pid":4242}"#;
    pub const HYPR_CLIENTS: &str = r#"[{"address":"0xdeadbeef","title":"Terminal","class":"kitty","workspace":{"id":2},"floating":false,"fullscreen":0,"at":[10,20],"size":[800,600],"pid":4242},{"address":"0xcafe","title":"Browser","class":"firefox","workspace":{"id":1},"floating":true,"fullscreen":0,"pid":5150}]"#;
    pub const HYPR_WORKSPACES: &str = r#"[{"id":1,"name":"1","monitor":"eDP-1","windows":1},{"id":2,"name":"2","monitor":"eDP-1","windows":1}]"#;
    pub const HYPR_ACTIVE_WORKSPACE: &str = r#"{"id":2,"name":"2","monitor":"eDP-1","windows":1}"#;

    /// Creates `runtime_dir/hypr/<his>/` and returns the command/event socket
    /// paths a `HyprlandBackend` would derive for it.
    pub fn fake_hyprland_paths(runtime_dir: &Path, his: &str) -> (PathBuf, PathBuf) {
        let dir = runtime_dir.join("hypr").join(his);
        std::fs::create_dir_all(&dir).unwrap();
        (dir.join(".socket.sock"), dir.join(".socket2.sock"))
    }

    /// Binds a Unix listener at `path`; each connection is read to EOF, passed
    /// to `responder`, and the reply is written back before closing.
    pub fn unix_rpc_server(
        path: &Path,
        responder: Arc<dyn Fn(&str) -> String + Send + Sync>,
    ) -> tokio::task::JoinHandle<()> {
        let listener = tokio::net::UnixListener::bind(path).unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let responder = responder.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    if stream.read_to_end(&mut buf).await.is_err() {
                        return;
                    }
                    let payload = String::from_utf8_lossy(&buf);
                    let reply = responder(&payload);
                    let _ = stream.write_all(reply.as_bytes()).await;
                });
            }
        })
    }

    /// Canned Hyprland IPC replies; dispatches containing "nonexistent" or
    /// "boom" are rejected to exercise error paths.
    pub fn hypr_reply(payload: &str) -> String {
        if let Some(q) = payload.strip_prefix("j/") {
            match q {
                "activewindow" => HYPR_ACTIVE_WINDOW.to_string(),
                "clients" => HYPR_CLIENTS.to_string(),
                "workspaces" => HYPR_WORKSPACES.to_string(),
                "activeworkspace" => HYPR_ACTIVE_WORKSPACE.to_string(),
                _ => "{}".to_string(),
            }
        } else if payload.starts_with("dispatch ") {
            if payload.contains("nonexistent") || payload.contains("boom") {
                "ERR command rejected".to_string()
            } else {
                "ok".to_string()
            }
        } else {
            String::new()
        }
    }

    /// Serves the canned Hyprland command socket at `path`.
    pub fn hypr_cmd_server(path: &Path) -> tokio::task::JoinHandle<()> {
        unix_rpc_server(path, Arc::new(hypr_reply))
    }

    /// Binds a Unix listener that accepts every connection then immediately
    /// drops it — the peer sees a reset mid-request.
    pub fn unix_drop_server(path: &Path) -> tokio::task::JoinHandle<()> {
        let listener = tokio::net::UnixListener::bind(path).unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                drop(stream);
            }
        })
    }

    /// Serves the Hyprland event socket at `path`: every accepted connection
    /// receives `lines` (one event per line) then is closed.
    pub fn hypr_event_server(path: &Path, lines: Vec<String>) -> tokio::task::JoinHandle<()> {
        let listener = tokio::net::UnixListener::bind(path).unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                for line in &lines {
                    if stream
                        .write_all(format!("{}\n", line).as_bytes())
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                let _ = stream.flush().await;
            }
        })
    }
}
#[cfg(feature = "hyprland")]
#[allow(unused_imports)]
pub use hypr_fakes::*;

// ---- Fake KDE session bus + KWin service ----------------------------------

#[cfg(feature = "kde")]
mod kde_fakes {
    use std::sync::Arc;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};

    #[derive(Debug, Clone)]
    pub struct FakeKwinWindow {
        pub id: String,
        pub title: String,
        pub app: String,
        pub desktop: i32,
        pub fullscreen: bool,
        pub pid: u32,
    }

    #[derive(Debug, Default)]
    pub struct FakeKwinState {
        pub current_desktop: i32,
        pub active: Option<String>,
        pub windows: Vec<FakeKwinWindow>,
        /// `call` returns a D-Bus error.
        pub fail_call: bool,
        /// `call` returns a non-string variant.
        pub bad_reply: bool,
        /// `loadScript` returns a D-Bus error.
        pub fail_load: bool,
        /// `tychoActive` returns an id that is not in the window list.
        pub ghost_active: bool,
        /// VirtualDesktopManager.desktops returns a non-array variant.
        pub bad_desktops: bool,
        /// VirtualDesktopManager.desktops returns `as` instead of `a(ss)`.
        pub desktops_strings: bool,
        /// `currentDesktop` returns a D-Bus error.
        pub fail_current: bool,
        /// `currentDesktop` returns the wrong D-Bus type (`s` instead of `i`).
        pub bad_current: bool,
        /// `loadScript` returns the wrong D-Bus type (`s` instead of `i`).
        pub bad_load_type: bool,
        /// `tychoWins` returns a string that is not valid JSON.
        pub bad_wins: bool,
    }

    pub type SharedKwin = Arc<std::sync::Mutex<FakeKwinState>>;

    /// Seeds a KWin state with one Konsole window on desktop 1.
    pub fn fake_kwin_state() -> SharedKwin {
        Arc::new(std::sync::Mutex::new(FakeKwinState {
            current_desktop: 1,
            active: Some("kde-win-001".to_string()),
            windows: vec![FakeKwinWindow {
                id: "kde-win-001".to_string(),
                title: "Konsole".to_string(),
                app: "org.kde.konsole".to_string(),
                desktop: 1,
                fullscreen: false,
                pid: 1337,
            }],
            ..Default::default()
        }))
    }

    /// State exercising degradation edges: a window on an unknown desktop, a
    /// ghost active id, a failing `call`, and a non-array desktops property.
    pub fn fake_kwin_state_exotic() -> SharedKwin {
        Arc::new(std::sync::Mutex::new(FakeKwinState {
            current_desktop: 1,
            active: None,
            windows: vec![FakeKwinWindow {
                id: "kde-win-777".to_string(),
                title: "Orphan".to_string(),
                app: "x".to_string(),
                desktop: 7,
                fullscreen: false,
                pid: 1,
            }],
            ghost_active: true,
            bad_desktops: true,
            ..Default::default()
        }))
    }

    /// Spawns a private `dbus-daemon` session bus. Returns the bus address and
    /// the child process (killed on drop).
    pub async fn fake_bus() -> Option<(String, tokio::process::Child)> {
        let mut child = tokio::process::Command::new("dbus-daemon")
            .args(["--session", "--print-address", "--nofork"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        let mut stdout = child.stdout.take()?;
        let mut line = String::new();
        AsyncBufReadExt::read_line(&mut tokio::io::BufReader::new(&mut stdout), &mut line)
            .await
            .ok()?;
        tokio::spawn(async move {
            let mut sink = Vec::new();
            let _ = stdout.read_to_end(&mut sink).await;
        });
        let addr = line.trim().to_string();
        if addr.is_empty() {
            None
        } else {
            Some((addr, child))
        }
    }

    pub struct FakeKwin {
        pub state: SharedKwin,
    }

    #[zbus::interface(name = "org.kde.KWin")]
    impl FakeKwin {
        #[zbus(name = "currentDesktop")]
        fn current_desktop(&self) -> zbus::fdo::Result<i32> {
            let state = self.state.lock().unwrap();
            if state.fail_current {
                return Err(zbus::fdo::Error::Failed("currentDesktop disabled".into()));
            }
            Ok(state.current_desktop)
        }

        #[zbus(name = "setCurrentDesktop")]
        fn set_current_desktop(&self, desktop: i32) {
            self.state.lock().unwrap().current_desktop = desktop;
        }
    }

    /// Variant whose `currentDesktop` replies with `s` instead of `i` — the
    /// backend's reply deserializer must report an IPC error.
    pub struct FakeKwinBadCurrent {
        pub state: SharedKwin,
    }

    #[zbus::interface(name = "org.kde.KWin")]
    impl FakeKwinBadCurrent {
        #[zbus(name = "currentDesktop")]
        fn current_desktop(&self) -> String {
            "one".to_string()
        }

        #[zbus(name = "setCurrentDesktop")]
        fn set_current_desktop(&self, desktop: i32) {
            self.state.lock().unwrap().current_desktop = desktop;
        }
    }

    pub struct FakeScripting {
        pub state: SharedKwin,
    }

    #[zbus::interface(name = "org.kde.kwin.Scripting")]
    impl FakeScripting {
        #[zbus(name = "loadScript")]
        fn load_script(&self, _path: String) -> zbus::fdo::Result<i32> {
            if self.state.lock().unwrap().fail_load {
                return Err(zbus::fdo::Error::Failed("script load disabled".into()));
            }
            Ok(0)
        }

        #[zbus(name = "start")]
        fn start(&self) {}

        #[zbus(name = "call")]
        fn call(&self, args: Vec<String>) -> zbus::fdo::Result<zbus::zvariant::Value<'static>> {
            let func = args.first().cloned().unwrap_or_default();
            let mut state = self.state.lock().unwrap();
            if state.fail_call {
                return Err(zbus::fdo::Error::Failed("call disabled".into()));
            }
            if state.bad_reply {
                return Ok(zbus::zvariant::Value::U64(7));
            }
            let reply = match func.as_str() {
                "tychoWins" => {
                    if state.bad_wins {
                        "{not json".to_string()
                    } else {
                        let list: Vec<serde_json::Value> = state
                            .windows
                            .iter()
                            .map(|w| {
                                serde_json::json!({
                                    "id": w.id, "title": w.title, "app": w.app,
                                    "desktop": w.desktop, "fs": w.fullscreen, "pid": w.pid
                                })
                            })
                            .collect();
                        serde_json::to_string(&list).unwrap()
                    }
                }
                "tychoActive" => {
                    if state.ghost_active {
                        "ghost-window".to_string()
                    } else {
                        state.active.clone().unwrap_or_default()
                    }
                }
                "tychoFocus" => {
                    let id = args.get(1).cloned().unwrap_or_default();
                    if state.windows.iter().any(|w| w.id == id) {
                        state.active = Some(id);
                        "ok".to_string()
                    } else {
                        "notfound".to_string()
                    }
                }
                "tychoClose" => {
                    let id = args.get(1).cloned().unwrap_or_default();
                    if let Some(pos) = state.windows.iter().position(|w| w.id == id) {
                        state.windows.remove(pos);
                        if state.active.as_deref() == Some(id.as_str()) {
                            state.active = None;
                        }
                        "ok".to_string()
                    } else {
                        "notfound".to_string()
                    }
                }
                "tychoMove" => {
                    let id = args.get(1).cloned().unwrap_or_default();
                    let desktop = args
                        .get(2)
                        .and_then(|d| d.parse::<i32>().ok())
                        .unwrap_or(-1);
                    if let Some(w) = state.windows.iter_mut().find(|w| w.id == id) {
                        w.desktop = desktop;
                        "ok".to_string()
                    } else {
                        "notfound".to_string()
                    }
                }
                "tychoFullscreen" => {
                    let id = args.get(1).cloned().unwrap_or_default();
                    if let Some(w) = state.windows.iter_mut().find(|w| w.id == id) {
                        w.fullscreen = !w.fullscreen;
                        "ok".to_string()
                    } else {
                        "notfound".to_string()
                    }
                }
                _ => "notfound".to_string(),
            };
            Ok(zbus::zvariant::Value::Str(reply.into()))
        }
    }

    /// Fake `org.kde.KWin.VirtualDesktopManager` exposing a `desktops`
    /// property (`a(ss)`) on `/VirtualDesktopManager`; zbus's built-in
    /// `org.freedesktop.DBus.Properties` implementation serves `Get`.
    pub struct FakeVdm;

    #[zbus::interface(name = "org.kde.KWin.VirtualDesktopManager")]
    impl FakeVdm {
        #[zbus(property, name = "desktops")]
        fn desktops(&self) -> Vec<(String, String)> {
            ["Main", "Dev", "Media"]
                .iter()
                .enumerate()
                .map(|(i, n)| ((i + 1).to_string(), n.to_string()))
                .collect()
        }
    }

    /// Variant whose `desktops` property has the wrong type — the backend's
    /// name parser must fall back to generated names.
    pub struct FakeVdmBad;

    #[zbus::interface(name = "org.kde.KWin.VirtualDesktopManager")]
    impl FakeVdmBad {
        #[zbus(property, name = "desktops")]
        fn desktops(&self) -> String {
            "not-an-array".to_string()
        }
    }

    /// Variant whose `desktops` property is `as` — array entries that are not
    /// structures yield no names, so generated fallbacks must be used.
    pub struct FakeVdmStrings;

    #[zbus::interface(name = "org.kde.KWin.VirtualDesktopManager")]
    impl FakeVdmStrings {
        #[zbus(property, name = "desktops")]
        fn desktops(&self) -> Vec<String> {
            vec!["1".to_string(), "2".to_string()]
        }
    }

    /// Variant whose `loadScript` replies with `s` instead of `i` — the
    /// backend's reply deserializer must report an IPC error.
    pub struct FakeScriptingBadLoad {
        pub state: SharedKwin,
    }

    #[zbus::interface(name = "org.kde.kwin.Scripting")]
    impl FakeScriptingBadLoad {
        #[zbus(name = "loadScript")]
        fn load_script(&self, _path: String) -> String {
            "script-zero".to_string()
        }

        #[zbus(name = "start")]
        fn start(&self) {}

        #[zbus(name = "call")]
        fn call(&self, _args: Vec<String>) -> zbus::zvariant::Value<'static> {
            let _ = &self.state;
            zbus::zvariant::Value::Str("".into())
        }
    }

    /// Connects to the private bus at `addr`, registers the fake KWin service
    /// objects and owns the `org.kde.KWin` name.
    pub async fn fake_kwin_bus(addr: &str, state: SharedKwin) -> zbus::Connection {
        let conn = zbus::connection::Builder::address(addr)
            .unwrap()
            .build()
            .await
            .unwrap();
        if state.lock().unwrap().bad_current {
            conn.object_server()
                .at(
                    "/KWin",
                    FakeKwinBadCurrent {
                        state: state.clone(),
                    },
                )
                .await
                .unwrap();
        } else {
            conn.object_server()
                .at(
                    "/KWin",
                    FakeKwin {
                        state: state.clone(),
                    },
                )
                .await
                .unwrap();
        }
        if state.lock().unwrap().bad_load_type {
            conn.object_server()
                .at(
                    "/Scripting",
                    FakeScriptingBadLoad {
                        state: state.clone(),
                    },
                )
                .await
                .unwrap();
        } else {
            conn.object_server()
                .at(
                    "/Scripting",
                    FakeScripting {
                        state: state.clone(),
                    },
                )
                .await
                .unwrap();
        }
        conn.object_server()
            .at(
                "/Scripting/Script0",
                FakeScripting {
                    state: state.clone(),
                },
            )
            .await
            .unwrap();
        if state.lock().unwrap().bad_desktops {
            conn.object_server()
                .at("/VirtualDesktopManager", FakeVdmBad)
                .await
                .unwrap();
        } else if state.lock().unwrap().desktops_strings {
            conn.object_server()
                .at("/VirtualDesktopManager", FakeVdmStrings)
                .await
                .unwrap();
        } else {
            conn.object_server()
                .at("/VirtualDesktopManager", FakeVdm)
                .await
                .unwrap();
        }
        conn.request_name("org.kde.KWin").await.unwrap();
        conn
    }

    /// Backend-side connection to a bus address (the `with_connection` seam).
    pub async fn bus_client(addr: &str) -> zbus::Connection {
        zbus::connection::Builder::address(addr)
            .unwrap()
            .build()
            .await
            .unwrap()
    }
}
#[cfg(feature = "kde")]
#[allow(unused_imports)]
pub use kde_fakes::*;
