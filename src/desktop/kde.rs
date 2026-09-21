//! Native KDE Plasma / KWin D-Bus desktop integration.
//!
//! Workspace operations call the stable `org.kde.KWin` interface directly.
//! Window operations are proxied through a small KWin script injected via
//! `org.kde.kwin.Scripting` — the only supported way to enumerate and control
//! windows on Plasma Wayland.

use super::traits::{
    BackendCapabilities, DesktopBackend, DesktopError, DesktopEvent, WindowContext,
    WorkspaceContext,
};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use zbus::Connection;

const KWIN_SERVICE: &str = "org.kde.KWin";
const KWIN_PATH: &str = "/KWin";
const KWIN_IFACE: &str = "org.kde.KWin";
const SCRIPTING_IFACE: &str = "org.kde.kwin.Scripting";
const SCRIPTING_PATH: &str = "/Scripting";
const BRIDGE_SCRIPT_NAME: &str = "tycho-kwin-bridge.js";

const BRIDGE_SCRIPT: &str = r#"
function tychoWins() {
    var ws = workspace.clientList ? workspace.clientList() : workspace.windowList();
    var out = [];
    for (var i = 0; i < ws.length; i++) {
        var w = ws[i];
        out.push({
            id: String(w.windowId),
            title: String(w.caption),
            app: String(w.resourceClass),
            desktop: w.desktop,
            fs: !!w.fullScreen,
            pid: w.pid || 0
        });
    }
    return JSON.stringify(out);
}
function tychoActive() {
    var w = workspace.activeClient || workspace.activeWindow;
    return w ? String(w.windowId) : "";
}
function tychoFocus(id) {
    var ws = workspace.clientList ? workspace.clientList() : workspace.windowList();
    for (var i = 0; i < ws.length; i++) {
        if (String(ws[i].windowId) === String(id)) {
            if (workspace.activeClient !== undefined) { workspace.activeClient = ws[i]; }
            if (workspace.activeWindow !== undefined) { workspace.activeWindow = ws[i]; }
            return "ok";
        }
    }
    return "notfound";
}
function tychoClose(id) {
    var ws = workspace.clientList ? workspace.clientList() : workspace.windowList();
    for (var i = 0; i < ws.length; i++) {
        if (String(ws[i].windowId) === String(id)) { ws[i].closeWindow(); return "ok"; }
    }
    return "notfound";
}
function tychoMove(id, ws2) {
    var ws = workspace.clientList ? workspace.clientList() : workspace.windowList();
    for (var i = 0; i < ws.length; i++) {
        if (String(ws[i].windowId) === String(id)) { ws[i].desktop = ws2; return "ok"; }
    }
    return "notfound";
}
function tychoFullscreen(id) {
    var ws = workspace.clientList ? workspace.clientList() : workspace.windowList();
    for (var i = 0; i < ws.length; i++) {
        if (String(ws[i].windowId) === String(id)) { ws[i].fullScreen = !ws[i].fullScreen; return "ok"; }
    }
    return "notfound";
}
"#;

#[derive(Debug)]
struct KdeDesktopState {
    active_workspace_id: i32,
    active_window_id: Option<String>,
    windows: std::collections::HashMap<String, WindowContext>,
    workspaces: std::collections::HashMap<i32, WorkspaceContext>,
}

pub struct KdeBackend {
    conn: Connection,
    script_path: RwLock<Option<String>>,
    state: Arc<RwLock<KdeDesktopState>>,
    event_tx: broadcast::Sender<DesktopEvent>,
    capabilities: BackendCapabilities,
}

impl KdeBackend {
    pub async fn new() -> Result<Self, DesktopError> {
        let conn = Connection::session()
            .await
            .map_err(|e| DesktopError::ConnectionFailed(format!("session bus: {}", e)))?;
        Self::with_connection(conn).await
    }

    /// Builds a backend on an existing bus connection — also the seam used by
    /// tests with a private `dbus-daemon`.
    pub async fn with_connection(conn: Connection) -> Result<Self, DesktopError> {
        let has_owner: bool = conn
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "NameHasOwner",
                &KWIN_SERVICE,
            )
            .await
            .map_err(|e| DesktopError::IpcError(format!("bus probe failed: {}", e)))?
            .body()
            .deserialize()
            .map_err(|e| DesktopError::IpcError(format!("bus probe reply: {}", e)))?;

        if !has_owner {
            return Err(DesktopError::ConnectionFailed(
                "org.kde.KWin is not owned on the session bus".into(),
            ));
        }

        let (event_tx, _) = broadcast::channel(256);
        let backend = Self {
            conn,
            script_path: RwLock::new(None),
            state: Arc::new(RwLock::new(KdeDesktopState {
                active_workspace_id: 1,
                active_window_id: None,
                windows: std::collections::HashMap::new(),
                workspaces: std::collections::HashMap::new(),
            })),
            event_tx,
            capabilities: BackendCapabilities::default(),
        };
        backend.refresh_state().await;
        Ok(backend)
    }

    async fn kwin_call<B>(&self, method: &str, body: &B) -> Result<zbus::Message, DesktopError>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        self.conn
            .call_method(
                Some(KWIN_SERVICE),
                KWIN_PATH,
                Some(KWIN_IFACE),
                method,
                body,
            )
            .await
            .map_err(|e| DesktopError::IpcError(format!("KWin {}: {}", method, e)))
    }

    /// Ensures the KWin bridge script is loaded and returns its object path
    /// (e.g. `/Scripting/Script3`).
    async fn ensure_bridge(&self) -> Result<String, DesktopError> {
        if let Some(path) = self.script_path.read().await.clone() {
            return Ok(path);
        }

        let dir = std::env::temp_dir().join("tycho-kwin");
        std::fs::create_dir_all(&dir).map_err(|e| DesktopError::Internal(e.to_string()))?;
        let script_file = dir.join(BRIDGE_SCRIPT_NAME);
        std::fs::write(&script_file, BRIDGE_SCRIPT)
            .map_err(|e| DesktopError::Internal(e.to_string()))?;

        let reply = self
            .conn
            .call_method(
                Some(KWIN_SERVICE),
                SCRIPTING_PATH,
                Some(SCRIPTING_IFACE),
                "loadScript",
                &(&script_file.to_string_lossy().to_string(),),
            )
            .await
            .map_err(|e| {
                DesktopError::UnsupportedOperation(format!("KWin scripting unavailable: {}", e))
            })?;
        let script_id: i32 = reply
            .body()
            .deserialize()
            .map_err(|e| DesktopError::IpcError(format!("loadScript reply: {}", e)))?;
        let object_path = format!("/Scripting/Script{}", script_id);

        // start() is a no-op if the script is already running on older
        // Plasma versions; ignore its result.
        let _ = self
            .conn
            .call_method(
                Some(KWIN_SERVICE),
                object_path.as_str(),
                Some(SCRIPTING_IFACE),
                "start",
                &(),
            )
            .await;

        *self.script_path.write().await = Some(object_path.clone());
        Ok(object_path)
    }

    /// Invokes a function in the injected bridge script and returns its
    /// string result.
    async fn bridge_call(&self, func: &str, args: &[&str]) -> Result<String, DesktopError> {
        let object_path = self.ensure_bridge().await?;
        let body: Vec<String> = std::iter::once(func.to_string())
            .chain(args.iter().map(|a| a.to_string()))
            .collect();
        let reply = self
            .conn
            .call_method(
                Some(KWIN_SERVICE),
                object_path.as_str(),
                Some(SCRIPTING_IFACE),
                "call",
                &(body,),
            )
            .await
            .map_err(|e| DesktopError::IpcError(format!("bridge {}: {}", func, e)))?;
        let body = reply.body();
        let value: zbus::zvariant::Value = body
            .deserialize()
            .map_err(|e| DesktopError::IpcError(format!("bridge {} reply: {}", func, e)))?;
        match value {
            zbus::zvariant::Value::Str(s) => Ok(s.to_string()),
            other => Ok(format!("{:?}", other)),
        }
    }

    async fn current_desktop(&self) -> Result<i32, DesktopError> {
        let reply = self.kwin_call("currentDesktop", &()).await?;
        reply
            .body()
            .deserialize::<i32>()
            .map_err(|e| DesktopError::IpcError(format!("currentDesktop reply: {}", e)))
    }

    async fn desktop_names(&self) -> Vec<String> {
        // Plasma exposes desktop names via the VirtualDesktopManager object;
        // schema varies across versions so we accept an array of strings.
        if let Ok(reply) = self
            .conn
            .call_method(
                Some(KWIN_SERVICE),
                "/VirtualDesktopManager",
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &("org.kde.KWin.VirtualDesktopManager", "desktops"),
            )
            .await
        {
            if let Ok(zbus::zvariant::Value::Array(arr)) =
                reply.body().deserialize::<zbus::zvariant::Value>()
            {
                let mut names = Vec::new();
                for entry in arr.iter() {
                    if let zbus::zvariant::Value::Structure(s) = entry {
                        if let Some(zbus::zvariant::Value::Str(name)) = s.fields().get(1) {
                            names.push(name.to_string());
                        }
                    }
                }
                if !names.is_empty() {
                    return names;
                }
            }
        }
        Vec::new()
    }

    /// Refreshes the cached state from KWin over D-Bus. Individual queries
    /// degrade gracefully — a partially populated model beats no backend.
    async fn refresh_state(&self) {
        let current = self.current_desktop().await.unwrap_or(1);
        let names = self.desktop_names().await;
        // Report exactly the desktops KWin names; the floor is the live
        // current index so a nameless session still lists its real desktop.
        let count = (names.len() as i32).max(current);

        let mut workspaces = std::collections::HashMap::new();
        for id in 1..=count {
            workspaces.insert(
                id,
                WorkspaceContext {
                    id,
                    name: names
                        .get((id - 1) as usize)
                        .cloned()
                        .unwrap_or_else(|| format!("Desktop {}", id)),
                    is_active: id == current,
                    monitor: String::new(),
                    windows_count: 0,
                },
            );
        }

        let mut windows = std::collections::HashMap::new();
        let mut active_window_id = None;

        if let Ok(json) = self.bridge_call("tychoWins", &[]).await {
            if let Ok(list) = serde_json::from_str::<Vec<serde_json::Value>>(&json) {
                for w in list {
                    let ctx = WindowContext {
                        id: w
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        title: w
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        app_id: w
                            .get("app")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        workspace_id: w.get("desktop").and_then(|v| v.as_i64()).unwrap_or(-1)
                            as i32,
                        is_floating: false,
                        is_fullscreen: w.get("fs").and_then(|v| v.as_bool()).unwrap_or(false),
                        geometry: None,
                        pid: w.get("pid").and_then(|v| v.as_u64()).map(|p| p as u32),
                    };
                    if let Some(ws) = workspaces.get_mut(&ctx.workspace_id) {
                        ws.windows_count += 1;
                    }
                    windows.insert(ctx.id.clone(), ctx);
                }
            }
        }
        if let Ok(active) = self.bridge_call("tychoActive", &[]).await {
            if !active.is_empty() && windows.contains_key(&active) {
                active_window_id = Some(active);
            }
        }

        let mut state = self.state.write().await;
        state.active_workspace_id = current;
        state.active_window_id = active_window_id;
        state.windows = windows;
        state.workspaces = workspaces;
    }
}

#[async_trait]
impl DesktopBackend for KdeBackend {
    fn name(&self) -> &'static str {
        "KDE Plasma (KWin / D-Bus)"
    }

    fn capabilities(&self) -> &BackendCapabilities {
        &self.capabilities
    }

    async fn get_active_window(&self) -> Result<Option<WindowContext>, DesktopError> {
        let state = self.state.read().await;
        Ok(state
            .active_window_id
            .as_ref()
            .and_then(|id| state.windows.get(id).cloned()))
    }

    async fn list_windows(&self) -> Result<Vec<WindowContext>, DesktopError> {
        let state = self.state.read().await;
        Ok(state.windows.values().cloned().collect())
    }

    async fn list_workspaces(&self) -> Result<Vec<WorkspaceContext>, DesktopError> {
        let state = self.state.read().await;
        let mut list: Vec<_> = state.workspaces.values().cloned().collect();
        list.sort_by_key(|w| w.id);
        Ok(list)
    }

    async fn switch_workspace(&self, target: i32) -> Result<(), DesktopError> {
        self.kwin_call("setCurrentDesktop", &(target,)).await?;
        let mut state = self.state.write().await;
        if !state.workspaces.contains_key(&target) {
            return Err(DesktopError::WorkspaceNotFound(target));
        }
        let previous = state.active_workspace_id;
        if let Some(old) = state.workspaces.get_mut(&previous) {
            old.is_active = false;
        }
        if let Some(new) = state.workspaces.get_mut(&target) {
            new.is_active = true;
        }
        state.active_workspace_id = target;
        let _ = self.event_tx.send(DesktopEvent::WorkspaceChanged(target));
        Ok(())
    }

    async fn focus_window(&self, window_id: &str) -> Result<(), DesktopError> {
        let res = self.bridge_call("tychoFocus", &[window_id]).await?;
        if res != "ok" {
            return Err(DesktopError::WindowNotFound(window_id.to_string()));
        }
        let mut state = self.state.write().await;
        if state.windows.contains_key(window_id) {
            state.active_window_id = Some(window_id.to_string());
            let ctx = state.windows.get(window_id).cloned();
            let _ = self.event_tx.send(DesktopEvent::ActiveWindowChanged(ctx));
            Ok(())
        } else {
            Err(DesktopError::WindowNotFound(window_id.to_string()))
        }
    }

    async fn close_window(&self, window_id: Option<&str>) -> Result<(), DesktopError> {
        let target_id = match window_id {
            Some(id) => id.to_string(),
            None => self
                .state
                .read()
                .await
                .active_window_id
                .clone()
                .ok_or_else(|| DesktopError::WindowNotFound("no active window".into()))?,
        };
        let res = self.bridge_call("tychoClose", &[&target_id]).await?;
        if res != "ok" {
            return Err(DesktopError::WindowNotFound(target_id));
        }
        let mut state = self.state.write().await;
        if let Some(removed) = state.windows.remove(&target_id) {
            if let Some(ws) = state.workspaces.get_mut(&removed.workspace_id) {
                ws.windows_count = ws.windows_count.saturating_sub(1);
            }
            if state.active_window_id.as_deref() == Some(&target_id) {
                state.active_window_id = None;
                let _ = self.event_tx.send(DesktopEvent::ActiveWindowChanged(None));
            }
        }
        let _ = self.event_tx.send(DesktopEvent::WindowClosed(target_id));
        Ok(())
    }

    async fn toggle_fullscreen(&self, window_id: Option<&str>) -> Result<(), DesktopError> {
        let target_id = match window_id {
            Some(id) => id.to_string(),
            None => self
                .state
                .read()
                .await
                .active_window_id
                .clone()
                .ok_or_else(|| DesktopError::WindowNotFound("no active window".into()))?,
        };
        let res = self.bridge_call("tychoFullscreen", &[&target_id]).await?;
        if res != "ok" {
            return Err(DesktopError::WindowNotFound(target_id));
        }
        let mut state = self.state.write().await;
        if let Some(win) = state.windows.get_mut(&target_id) {
            win.is_fullscreen = !win.is_fullscreen;
            Ok(())
        } else {
            Err(DesktopError::WindowNotFound(target_id))
        }
    }

    async fn toggle_floating(&self, window_id: Option<&str>) -> Result<(), DesktopError> {
        // KWin has no direct floating toggle; approximated via the keepAbove
        // hint on X11 sessions and unsupported on Wayland.
        let _ = window_id;
        Err(DesktopError::UnsupportedOperation(
            "window floating toggle is not exposed by KWin scripting".into(),
        ))
    }

    async fn move_window_to_workspace(
        &self,
        window_id: Option<&str>,
        target_workspace: i32,
    ) -> Result<(), DesktopError> {
        let target_id = match window_id {
            Some(id) => id.to_string(),
            None => self
                .state
                .read()
                .await
                .active_window_id
                .clone()
                .ok_or_else(|| DesktopError::WindowNotFound("no active window".into()))?,
        };
        let res = self
            .bridge_call("tychoMove", &[&target_id, &target_workspace.to_string()])
            .await?;
        if res != "ok" {
            return Err(DesktopError::WindowNotFound(target_id));
        }
        let mut state = self.state.write().await;
        if let Some(win) = state.windows.get_mut(&target_id) {
            let old_ws = win.workspace_id;
            win.workspace_id = target_workspace;
            if let Some(ws) = state.workspaces.get_mut(&old_ws) {
                ws.windows_count = ws.windows_count.saturating_sub(1);
            }
            if let Some(ws) = state.workspaces.get_mut(&target_workspace) {
                ws.windows_count += 1;
            }
            Ok(())
        } else {
            Err(DesktopError::WindowNotFound(target_id))
        }
    }

    fn subscribe_events(&self) -> Result<broadcast::Receiver<DesktopEvent>, DesktopError> {
        Ok(self.event_tx.subscribe())
    }
}
