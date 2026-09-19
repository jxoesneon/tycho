//! High-level desktop orchestrator and environment auto-detection manager.

use super::hyprland::HyprlandBackend;
use super::kde::KdeBackend;
use super::mock::MockBackend;
use super::traits::{DesktopBackend, DesktopError, WindowContext, WorkspaceContext};
use std::env;
use std::sync::Arc;

#[derive(Clone)]
pub struct DesktopManager {
    backend: Arc<dyn DesktopBackend>,
}

impl DesktopManager {
    pub async fn init_auto() -> Result<Self, DesktopError> {
        let xdg = env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
        let hypr_sig = env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok();
        let kde_session = env::var("KDE_FULL_SESSION").is_ok() || xdg.contains("kde") || xdg.contains("plasma");

        if hypr_sig || xdg.contains("hyprland") {
            if let Ok(b) = HyprlandBackend::new().await {
                return Ok(Self { backend: Arc::new(b) });
            }
        }

        if kde_session {
            if let Ok(b) = KdeBackend::new().await {
                return Ok(Self { backend: Arc::new(b) });
            }
        }

        Ok(Self {
            backend: Arc::new(MockBackend::new()),
        })
    }

    pub fn with_backend(backend: Arc<dyn DesktopBackend>) -> Self {
        Self { backend }
    }

    pub fn init_mock() -> Self {
        Self {
            backend: Arc::new(MockBackend::new()),
        }
    }

    pub fn backend(&self) -> &dyn DesktopBackend {
        self.backend.as_ref()
    }

    pub async fn get_context_summary(&self) -> String {
        match self.backend.get_active_window().await {
            Ok(Some(win)) => format!("Active App: {} | Window: \\\"{}\\\" | Workspace: {}", win.app_id, win.title, win.workspace_id),
            Ok(None) => "Active App: None (Empty Desktop)".to_string(),
            Err(e) => format!("Desktop Context Unavailable: {}", e),
        }
    }

    pub async fn dispatch_action(&self, action: &str, arg: Option<&str>) -> Result<String, DesktopError> {
        match action {
            "workspace_switch" | "switch_workspace" => {
                let target: i32 = arg
                    .ok_or_else(|| DesktopError::UnsupportedOperation("missing workspace target".into()))?
                    .parse()
                    .map_err(|_| DesktopError::UnsupportedOperation("invalid workspace number".into()))?;
                self.backend.switch_workspace(target).await?;
                Ok(format!("Switched to workspace {}", target))
            }
            "window_close" | "close_window" => {
                self.backend.close_window(arg).await?;
                Ok("Closed window".to_string())
            }
            "window_focus" | "focus_window" => {
                let win_id = arg.ok_or_else(|| DesktopError::UnsupportedOperation("missing window id".into()))?;
                self.backend.focus_window(win_id).await?;
                Ok(format!("Focused window {}", win_id))
            }
            "window_fullscreen" | "toggle_fullscreen" => {
                self.backend.toggle_fullscreen(arg).await?;
                Ok("Toggled fullscreen".to_string())
            }
            "window_float" | "toggle_floating" => {
                self.backend.toggle_floating(arg).await?;
                Ok("Toggled floating mode".to_string())
            }
            "move_to_workspace" => {
                let target: i32 = arg
                    .ok_or_else(|| DesktopError::UnsupportedOperation("missing workspace target".into()))?
                    .parse()
                    .map_err(|_| DesktopError::UnsupportedOperation("invalid workspace number".into()))?;
                self.backend.move_window_to_workspace(None, target).await?;
                Ok(format!("Moved active window to workspace {}", target))
            }
            "get_context" => Ok(self.get_context_summary().await),
            "list_workspaces" => {
                let workspaces = self.backend.list_workspaces().await?;
                let summary = workspaces
                    .iter()
                    .map(|w| format!("{}: {}{}", w.id, w.name, if w.is_active { " (active)" } else { "" }))
                    .collect::<Vec<_>>()
                    .join(", ");
                Ok(format!("Workspaces: [{}]", summary))
            }
            unsupported => Err(DesktopError::UnsupportedOperation(format!("Action '{}' is not recognized", unsupported))),
        }
    }
}
