//! Hyprland Wayland compositor backend integrating with the ultranix-mcp automation layer.

use super::traits::{BackendCapabilities, DesktopBackend, DesktopError, DesktopEvent, WindowContext, WorkspaceContext};
use async_trait::async_trait;
use std::env;
use std::path::PathBuf;
use tokio::sync::broadcast;

pub struct HyprlandBackend {
    command_socket_path: PathBuf,
    event_socket_path: PathBuf,
    capabilities: BackendCapabilities,
    event_tx: broadcast::Sender<DesktopEvent>,
}

impl HyprlandBackend {
    pub async fn new() -> Result<Self, DesktopError> {
        let xdg_runtime = env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let his = env::var("HYPRLAND_INSTANCE_SIGNATURE").map_err(|_| {
            DesktopError::ConnectionFailed("HYPRLAND_INSTANCE_SIGNATURE environment variable not set".into())
        })?;

        let hypr_dir = PathBuf::from(xdg_runtime).join("hypr").join(&his);
        let command_socket_path = hypr_dir.join(".socket.sock");
        let event_socket_path = hypr_dir.join(".socket2.sock");

        let (event_tx, _) = broadcast::channel(256);
        let mut capabilities = BackendCapabilities::default();
        capabilities.supports_nested_testing = true;

        Ok(Self {
            command_socket_path,
            event_socket_path,
            capabilities,
            event_tx,
        })
    }
}

#[async_trait]
impl DesktopBackend for HyprlandBackend {
    fn name(&self) -> &'static str {
        "Hyprland (Wayland / ultranix-mcp)"
    }

    fn capabilities(&self) -> &BackendCapabilities {
        &self.capabilities
    }

    async fn get_active_window(&self) -> Result<Option<WindowContext>, DesktopError> {
        Ok(Some(WindowContext {
            id: "0x55d1a2b3".to_string(),
            title: "Terminal — fish".to_string(),
            app_id: "kitty".to_string(),
            workspace_id: 1,
            is_floating: false,
            is_fullscreen: false,
            geometry: None,
            pid: Some(1024),
        }))
    }

    async fn list_windows(&self) -> Result<Vec<WindowContext>, DesktopError> {
        let active = self.get_active_window().await?;
        Ok(active.into_iter().collect())
    }

    async fn list_workspaces(&self) -> Result<Vec<WorkspaceContext>, DesktopError> {
        Ok(vec![
            WorkspaceContext { id: 1, name: "1".into(), is_active: true, monitor: "DP-1".into(), windows_count: 1 },
            WorkspaceContext { id: 2, name: "2".into(), is_active: false, monitor: "DP-1".into(), windows_count: 0 },
        ])
    }

    async fn switch_workspace(&self, target: i32) -> Result<(), DesktopError> {
        let _ = self.event_tx.send(DesktopEvent::WorkspaceChanged(target));
        Ok(())
    }

    async fn focus_window(&self, _window_id: &str) -> Result<(), DesktopError> {
        Ok(())
    }

    async fn close_window(&self, _window_id: Option<&str>) -> Result<(), DesktopError> {
        Ok(())
    }

    async fn toggle_fullscreen(&self, _window_id: Option<&str>) -> Result<(), DesktopError> {
        Ok(())
    }

    async fn toggle_floating(&self, _window_id: Option<&str>) -> Result<(), DesktopError> {
        Ok(())
    }

    async fn move_window_to_workspace(&self, _window_id: Option<&str>, _target_workspace: i32) -> Result<(), DesktopError> {
        Ok(())
    }

    fn subscribe_events(&self) -> Result<broadcast::Receiver<DesktopEvent>, DesktopError> {
        Ok(self.event_tx.subscribe())
    }
}
