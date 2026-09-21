//! In-memory stateful mock backend for headless execution and automated tests.

use super::traits::{
    BackendCapabilities, DesktopBackend, DesktopError, DesktopEvent, WindowContext,
    WorkspaceContext,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

#[derive(Debug)]
struct MockDesktopState {
    active_workspace_id: i32,
    active_window_id: Option<String>,
    windows: HashMap<String, WindowContext>,
    workspaces: HashMap<i32, WorkspaceContext>,
}

#[derive(Clone)]
pub struct MockBackend {
    state: Arc<RwLock<MockDesktopState>>,
    event_tx: broadcast::Sender<DesktopEvent>,
    capabilities: BackendCapabilities,
}

impl MockBackend {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(128);
        let mut workspaces = HashMap::new();
        for id in 1..=5 {
            workspaces.insert(
                id,
                WorkspaceContext {
                    id,
                    name: format!("workspace-{}", id),
                    is_active: id == 1,
                    monitor: "DP-1".to_string(),
                    windows_count: usize::from(id == 1),
                },
            );
        }

        let mut windows = HashMap::new();
        let default_win = WindowContext {
            id: "win-001".to_string(),
            title: "Terminal".to_string(),
            app_id: "kitty".to_string(),
            workspace_id: 1,
            is_floating: false,
            is_fullscreen: false,
            geometry: None,
            pid: Some(1234),
        };
        windows.insert(default_win.id.clone(), default_win);

        let state = Arc::new(RwLock::new(MockDesktopState {
            active_workspace_id: 1,
            active_window_id: Some("win-001".to_string()),
            windows,
            workspaces,
        }));

        let capabilities = BackendCapabilities {
            supports_nested_testing: true,
            ..Default::default()
        };

        Self {
            state,
            event_tx,
            capabilities,
        }
    }
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DesktopBackend for MockBackend {
    fn name(&self) -> &'static str {
        "Mock Compositor"
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
        let mut state = self.state.write().await;
        state
            .workspaces
            .entry(target)
            .or_insert_with(|| WorkspaceContext {
                id: target,
                name: format!("workspace-{}", target),
                is_active: false,
                monitor: "DP-1".to_string(),
                windows_count: 0,
            });
        let old_active = state.active_workspace_id;
        if let Some(old) = state.workspaces.get_mut(&old_active) {
            old.is_active = false;
        }
        if let Some(new) = state.workspaces.get_mut(&target) {
            new.is_active = true;
        }
        state.active_workspace_id = target;

        let active_win = state
            .windows
            .values()
            .find(|w| w.workspace_id == target)
            .map(|w| w.id.clone());
        state.active_window_id = active_win.clone();

        let _ = self.event_tx.send(DesktopEvent::WorkspaceChanged(target));
        let win_ctx = active_win.and_then(|id| state.windows.get(&id).cloned());
        let _ = self
            .event_tx
            .send(DesktopEvent::ActiveWindowChanged(win_ctx));
        Ok(())
    }

    async fn focus_window(&self, window_id: &str) -> Result<(), DesktopError> {
        let mut state = self.state.write().await;
        if let Some(win) = state.windows.get(window_id).cloned() {
            state.active_window_id = Some(window_id.to_string());
            if state.active_workspace_id != win.workspace_id {
                let old = state.active_workspace_id;
                if let Some(ws) = state.workspaces.get_mut(&old) {
                    ws.is_active = false;
                }
                if let Some(ws) = state.workspaces.get_mut(&win.workspace_id) {
                    ws.is_active = true;
                }
                state.active_workspace_id = win.workspace_id;
                let _ = self
                    .event_tx
                    .send(DesktopEvent::WorkspaceChanged(win.workspace_id));
            }
            let _ = self
                .event_tx
                .send(DesktopEvent::ActiveWindowChanged(Some(win)));
            Ok(())
        } else {
            Err(DesktopError::WindowNotFound(window_id.to_string()))
        }
    }

    async fn close_window(&self, window_id: Option<&str>) -> Result<(), DesktopError> {
        let mut state = self.state.write().await;
        let target_id = match window_id {
            Some(id) => id.to_string(),
            None => state
                .active_window_id
                .clone()
                .ok_or_else(|| DesktopError::WindowNotFound("no active window".into()))?,
        };

        if let Some(removed) = state.windows.remove(&target_id) {
            if let Some(ws) = state.workspaces.get_mut(&removed.workspace_id) {
                if ws.windows_count > 0 {
                    ws.windows_count -= 1;
                }
            }
            if state.active_window_id.as_deref() == Some(&target_id) {
                state.active_window_id = None;
                let _ = self.event_tx.send(DesktopEvent::ActiveWindowChanged(None));
            }
            let _ = self.event_tx.send(DesktopEvent::WindowClosed(target_id));
            Ok(())
        } else {
            Err(DesktopError::WindowNotFound(target_id))
        }
    }

    async fn toggle_fullscreen(&self, window_id: Option<&str>) -> Result<(), DesktopError> {
        let mut state = self.state.write().await;
        let target_id = match window_id {
            Some(id) => id.to_string(),
            None => state
                .active_window_id
                .clone()
                .ok_or_else(|| DesktopError::WindowNotFound("no active window".into()))?,
        };
        if let Some(win) = state.windows.get_mut(&target_id) {
            win.is_fullscreen = !win.is_fullscreen;
            Ok(())
        } else {
            Err(DesktopError::WindowNotFound(target_id))
        }
    }

    async fn toggle_floating(&self, window_id: Option<&str>) -> Result<(), DesktopError> {
        let mut state = self.state.write().await;
        let target_id = match window_id {
            Some(id) => id.to_string(),
            None => state
                .active_window_id
                .clone()
                .ok_or_else(|| DesktopError::WindowNotFound("no active window".into()))?,
        };
        if let Some(win) = state.windows.get_mut(&target_id) {
            win.is_floating = !win.is_floating;
            Ok(())
        } else {
            Err(DesktopError::WindowNotFound(target_id))
        }
    }

    async fn move_window_to_workspace(
        &self,
        window_id: Option<&str>,
        target_workspace: i32,
    ) -> Result<(), DesktopError> {
        let mut state = self.state.write().await;
        let target_id = match window_id {
            Some(id) => id.to_string(),
            None => state
                .active_window_id
                .clone()
                .ok_or_else(|| DesktopError::WindowNotFound("no active window".into()))?,
        };
        if let Some(win) = state.windows.get_mut(&target_id) {
            let old_ws = win.workspace_id;
            win.workspace_id = target_workspace;
            if let Some(ws) = state.workspaces.get_mut(&old_ws) {
                if ws.windows_count > 0 {
                    ws.windows_count -= 1;
                }
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
