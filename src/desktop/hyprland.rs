//! Hyprland Wayland compositor backend over the native Unix-socket IPC.

use super::traits::{
    BackendCapabilities, DesktopBackend, DesktopError, DesktopEvent, WindowContext, WindowGeometry,
    WorkspaceContext,
};
use async_trait::async_trait;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::broadcast;

const SYNTAX_UNKNOWN: u8 = 0;
const SYNTAX_LUA: u8 = 2;

/// Quotes a string for embedding inside a Lua expression.
fn lua_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Translates a legacy text dispatcher command into the `hl.dsp`
/// expression form Hyprland >= 0.55 expects inside `hl.dispatch(...)`.
fn lua_dispatch_command(cmd: &str) -> Result<String, DesktopError> {
    let (name, args) = cmd.split_once(' ').unwrap_or((cmd, ""));
    let args = args.trim();
    let unsupported =
        || DesktopError::IpcError(format!("cannot translate dispatch '{}' to lua form", cmd));
    Ok(match name {
        "workspace" => format!("hl.dsp.focus({{ workspace = {} }})", args),
        "focuswindow" => format!("hl.dsp.focus({{ window = {} }})", lua_str(args)),
        "killactive" => "hl.dsp.window.close()".to_string(),
        "closewindow" => {
            format!("hl.dsp.window.close({{ window = {} }})", lua_str(args))
        }
        "fullscreen" => "hl.dsp.window.fullscreen({})".to_string(),
        "togglefloating" => "hl.dsp.window.float({})".to_string(),
        "movetoworkspace" => match args.split_once(',') {
            Some((ws, win)) => format!(
                "hl.dsp.window.move({{ workspace = {}, window = {} }})",
                ws.trim(),
                lua_str(win.trim())
            ),
            None => format!("hl.dsp.window.move({{ workspace = {} }})", args),
        },
        _ => return Err(unsupported()),
    })
}

pub struct HyprlandBackend {
    command_socket_path: PathBuf,
    event_socket_path: PathBuf,
    capabilities: BackendCapabilities,
    event_tx: broadcast::Sender<DesktopEvent>,
    event_reader_started: AtomicBool,
    /// `dispatch` wire syntax: 0 = unprobed, 1 = legacy text,
    /// 2 = Lua dispatcher expressions (Hyprland >= 0.55).
    dispatch_syntax: AtomicU8,
}

impl HyprlandBackend {
    pub async fn new() -> Result<Self, DesktopError> {
        let xdg_runtime = env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let his = env::var("HYPRLAND_INSTANCE_SIGNATURE").map_err(|_| {
            DesktopError::ConnectionFailed(
                "HYPRLAND_INSTANCE_SIGNATURE environment variable not set".into(),
            )
        })?;

        let hypr_dir = PathBuf::from(xdg_runtime).join("hypr").join(&his);
        let command_socket_path = hypr_dir.join(".socket.sock");
        let event_socket_path = hypr_dir.join(".socket2.sock");

        if !command_socket_path.exists() {
            return Err(DesktopError::ConnectionFailed(format!(
                "Hyprland command socket not found at {}",
                command_socket_path.display()
            )));
        }

        let (event_tx, _) = broadcast::channel(256);
        let capabilities = BackendCapabilities {
            supports_nested_testing: true,
            ..Default::default()
        };

        Ok(Self {
            command_socket_path,
            event_socket_path,
            capabilities,
            event_tx,
            event_reader_started: AtomicBool::new(false),
            dispatch_syntax: AtomicU8::new(SYNTAX_UNKNOWN),
        })
    }

    /// Builds a backend pointed at explicit socket paths — also the seam
    /// used by tests against fake IPC servers.
    pub fn with_sockets(command_socket_path: PathBuf, event_socket_path: PathBuf) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            command_socket_path,
            event_socket_path,
            capabilities: BackendCapabilities {
                supports_nested_testing: true,
                ..Default::default()
            },
            event_tx,
            event_reader_started: AtomicBool::new(false),
            dispatch_syntax: AtomicU8::new(SYNTAX_UNKNOWN),
        }
    }

    pub fn command_socket_path(&self) -> &Path {
        &self.command_socket_path
    }

    pub fn event_socket_path(&self) -> &Path {
        &self.event_socket_path
    }

    /// Sends one IPC request to the command socket and returns the raw reply.
    async fn request(&self, payload: &str) -> Result<String, DesktopError> {
        let mut stream = UnixStream::connect(&self.command_socket_path)
            .await
            .map_err(|e| {
                DesktopError::ConnectionFailed(format!(
                    "connect {}: {}",
                    self.command_socket_path.display(),
                    e
                ))
            })?;
        stream
            .write_all(payload.as_bytes())
            .await
            .map_err(|e| DesktopError::IpcError(format!("write: {}", e)))?;
        stream
            .shutdown()
            .await
            .map_err(|e| DesktopError::IpcError(format!("shutdown: {}", e)))?;
        let mut buf = Vec::new();
        stream
            .read_to_end(&mut buf)
            .await
            .map_err(|e| DesktopError::IpcError(format!("read: {}", e)))?;
        Ok(String::from_utf8_lossy(&buf).to_string())
    }

    async fn request_json(&self, query: &str) -> Result<serde_json::Value, DesktopError> {
        let raw = self.request(&format!("j/{}", query)).await?;
        serde_json::from_str(&raw)
            .map_err(|e| DesktopError::IpcError(format!("malformed {} reply: {}", query, e)))
    }

    async fn dispatch(&self, cmd: &str) -> Result<(), DesktopError> {
        let lua_mode = self.dispatch_syntax.load(Ordering::SeqCst) == SYNTAX_LUA;
        let payload = if lua_mode {
            lua_dispatch_command(cmd)?
        } else {
            cmd.to_string()
        };
        let resp = self.request(&format!("dispatch {}", payload)).await?;
        if resp.trim_start().starts_with("ok") {
            return Ok(());
        }
        // Hyprland >= 0.55 feeds `dispatch` args to the Lua evaluator as
        // `hl.dispatch(...)` — legacy text commands are rejected. Retry
        // with the dispatcher-expression form and pin the syntax.
        if !lua_mode && resp.contains("hl.dispatch") {
            let lua = lua_dispatch_command(cmd)?;
            let resp = self.request(&format!("dispatch {}", lua)).await?;
            if resp.trim_start().starts_with("ok") {
                self.dispatch_syntax.store(SYNTAX_LUA, Ordering::SeqCst);
                return Ok(());
            }
            return Err(DesktopError::IpcError(format!(
                "dispatch '{}' rejected: {}",
                cmd,
                resp.trim()
            )));
        }
        Err(DesktopError::IpcError(format!(
            "dispatch '{}' rejected: {}",
            cmd,
            resp.trim()
        )))
    }

    fn ensure_event_reader(&self) {
        if self.event_reader_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let socket = self.event_socket_path.clone();
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            let stream = match UnixStream::connect(&socket).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("hyprland event socket unavailable: {}", e);
                    return;
                }
            };
            let mut lines = BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(event) = Self::map_event_line(&line) {
                    let _ = tx.send(event);
                }
            }
        });
    }

    fn map_event_line(line: &str) -> Option<DesktopEvent> {
        let (name, payload) = line.split_once(">>")?;
        match name {
            "workspace" => payload
                .trim()
                .parse::<i32>()
                .ok()
                .map(DesktopEvent::WorkspaceChanged),
            "openwindow" => {
                // 0xADDR,WORKSPACE,CLASS,TITLE
                let mut parts = payload.splitn(4, ',');
                let addr = parts.next()?.to_string();
                let ws = parts.next()?.parse::<i32>().unwrap_or(-1);
                let class = parts.next()?.to_string();
                let title = parts.next().unwrap_or("").to_string();
                Some(DesktopEvent::WindowOpened(WindowContext {
                    id: addr,
                    title,
                    app_id: class,
                    workspace_id: ws,
                    is_floating: false,
                    is_fullscreen: false,
                    geometry: None,
                    pid: None,
                }))
            }
            "closewindow" => Some(DesktopEvent::WindowClosed(payload.trim().to_string())),
            "focusedmon" => {
                let mon = payload.split(',').next().unwrap_or("").to_string();
                Some(DesktopEvent::MonitorChanged(mon))
            }
            "activewindowv2" => Some(DesktopEvent::ActiveWindowChanged(None)),
            _ => None,
        }
    }

    fn parse_window(v: &serde_json::Value) -> Option<WindowContext> {
        let workspace_id = v
            .pointer("/workspace/id")
            .and_then(|w| w.as_i64())
            .unwrap_or(-1) as i32;
        let geometry = match (v.get("at"), v.get("size")) {
            (Some(at), Some(size)) => Some(WindowGeometry {
                x: at.get(0).and_then(|x| x.as_i64()).unwrap_or(0) as i32,
                y: at.get(1).and_then(|y| y.as_i64()).unwrap_or(0) as i32,
                width: size.get(0).and_then(|w| w.as_i64()).unwrap_or(0) as u32,
                height: size.get(1).and_then(|h| h.as_i64()).unwrap_or(0) as u32,
            }),
            _ => None,
        };
        Some(WindowContext {
            id: v.get("address")?.as_str()?.to_string(),
            title: v
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string(),
            app_id: v
                .get("class")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string(),
            workspace_id,
            is_floating: v.get("floating").and_then(|f| f.as_bool()).unwrap_or(false),
            is_fullscreen: v
                .get("fullscreen")
                .and_then(|f| f.as_i64())
                .map(|f| f != 0)
                .unwrap_or(false),
            geometry,
            pid: v.get("pid").and_then(|p| p.as_u64()).map(|p| p as u32),
        })
    }
}

#[async_trait]
impl DesktopBackend for HyprlandBackend {
    fn name(&self) -> &'static str {
        "Hyprland (Wayland socket IPC)"
    }

    fn capabilities(&self) -> &BackendCapabilities {
        &self.capabilities
    }

    async fn get_active_window(&self) -> Result<Option<WindowContext>, DesktopError> {
        let v = self.request_json("activewindow").await?;
        Ok(Self::parse_window(&v))
    }

    async fn list_windows(&self) -> Result<Vec<WindowContext>, DesktopError> {
        let v = self.request_json("clients").await?;
        let list = v
            .as_array()
            .ok_or_else(|| DesktopError::IpcError("clients reply is not an array".into()))?;
        Ok(list.iter().filter_map(Self::parse_window).collect())
    }

    async fn list_workspaces(&self) -> Result<Vec<WorkspaceContext>, DesktopError> {
        let workspaces = self.request_json("workspaces").await?;
        let active = self.request_json("activeworkspace").await.ok();
        let active_id = active
            .as_ref()
            .and_then(|a| a.get("id"))
            .and_then(|i| i.as_i64())
            .unwrap_or(-1);

        let list = workspaces
            .as_array()
            .ok_or_else(|| DesktopError::IpcError("workspaces reply is not an array".into()))?;
        Ok(list
            .iter()
            .filter_map(|w| {
                let id = w.get("id")?.as_i64()? as i32;
                Some(WorkspaceContext {
                    id,
                    name: w
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string(),
                    is_active: id as i64 == active_id,
                    monitor: w
                        .get("monitor")
                        .and_then(|m| m.as_str())
                        .unwrap_or("")
                        .to_string(),
                    windows_count: w.get("windows").and_then(|c| c.as_u64()).unwrap_or(0) as usize,
                })
            })
            .collect())
    }

    async fn switch_workspace(&self, target: i32) -> Result<(), DesktopError> {
        self.dispatch(&format!("workspace {}", target)).await?;
        let _ = self.event_tx.send(DesktopEvent::WorkspaceChanged(target));
        Ok(())
    }

    async fn focus_window(&self, window_id: &str) -> Result<(), DesktopError> {
        self.dispatch(&format!("focuswindow address:{}", window_id))
            .await
    }

    async fn close_window(&self, window_id: Option<&str>) -> Result<(), DesktopError> {
        match window_id {
            Some(id) => {
                self.dispatch(&format!("closewindow address:{}", id))
                    .await?;
                let _ = self
                    .event_tx
                    .send(DesktopEvent::WindowClosed(id.to_string()));
            }
            None => self.dispatch("killactive").await?,
        }
        Ok(())
    }

    async fn toggle_fullscreen(&self, window_id: Option<&str>) -> Result<(), DesktopError> {
        if let Some(id) = window_id {
            self.dispatch(&format!("focuswindow address:{}", id))
                .await?;
        }
        self.dispatch("fullscreen").await
    }

    async fn toggle_floating(&self, window_id: Option<&str>) -> Result<(), DesktopError> {
        if let Some(id) = window_id {
            self.dispatch(&format!("focuswindow address:{}", id))
                .await?;
        }
        self.dispatch("togglefloating").await
    }

    async fn move_window_to_workspace(
        &self,
        window_id: Option<&str>,
        target_workspace: i32,
    ) -> Result<(), DesktopError> {
        match window_id {
            Some(id) => {
                self.dispatch(&format!(
                    "movetoworkspace {},address:{}",
                    target_workspace, id
                ))
                .await
            }
            None => {
                self.dispatch(&format!("movetoworkspace {}", target_workspace))
                    .await
            }
        }
    }

    fn subscribe_events(&self) -> Result<broadcast::Receiver<DesktopEvent>, DesktopError> {
        self.ensure_event_reader();
        Ok(self.event_tx.subscribe())
    }
}
