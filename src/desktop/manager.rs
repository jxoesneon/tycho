//! High-level desktop orchestrator and environment auto-detection manager.

#[cfg(feature = "hyprland")]
use super::hyprland::HyprlandBackend;
#[cfg(feature = "kde")]
use super::kde::KdeBackend;
use super::mock::MockBackend;
use super::traits::{DesktopBackend, DesktopError};
#[cfg(any(feature = "hyprland", feature = "kde"))]
use std::env;
use std::sync::Arc;
use tracing::{info, warn};

#[derive(Clone)]
pub struct DesktopManager {
    backend: Arc<dyn DesktopBackend>,
}

impl DesktopManager {
    /// Binds a desktop backend according to `desktop.backend` /
    /// `desktop.auto_detect` config: a named backend is tried directly,
    /// "auto" runs session detection (unless `auto_detect` is off, which
    /// selects the mock), and "mock"/"headless" forces the in-memory
    /// backend. Falls back to the mock with a warning when the requested
    /// or detected compositor doesn't answer.
    pub async fn init_with_config(backend: &str, auto_detect: bool) -> Self {
        match backend.trim().to_lowercase().as_str() {
            "hyprland" => return Self::init_hyprland().await,
            "kde" | "plasma" => return Self::init_kde().await,
            "mock" | "headless" | "none" => {
                info!("desktop backend: mock (configured)");
                return Self::init_mock();
            }
            _ => {}
        }
        if !auto_detect {
            warn!("desktop.backend=auto but auto_detect=false; desktop control disabled");
            return Self::init_mock();
        }
        Self::init_auto().await
    }

    #[cfg(feature = "hyprland")]
    async fn init_hyprland() -> Self {
        match HyprlandBackend::new().await {
            Ok(b) => Self {
                backend: Arc::new(b),
            },
            Err(e) => {
                warn!("hyprland backend requested but failed to connect ({e}); desktop control simulated");
                Self::init_mock()
            }
        }
    }

    #[cfg(not(feature = "hyprland"))]
    async fn init_hyprland() -> Self {
        warn!("hyprland backend requested but not compiled in; desktop control simulated");
        Self::init_mock()
    }

    #[cfg(feature = "kde")]
    async fn init_kde() -> Self {
        match KdeBackend::new().await {
            Ok(b) => Self {
                backend: Arc::new(b),
            },
            Err(e) => {
                warn!(
                    "kde backend requested but failed to connect ({e}); desktop control simulated"
                );
                Self::init_mock()
            }
        }
    }

    #[cfg(not(feature = "kde"))]
    async fn init_kde() -> Self {
        warn!("kde backend requested but not compiled in; desktop control simulated");
        Self::init_mock()
    }

    /// Detects the running compositor and binds its backend, falling back
    /// to the in-memory mock when no real compositor answers — or when no
    /// compositor feature was compiled in.
    pub async fn init_auto() -> Self {
        #[cfg(feature = "hyprland")]
        {
            let xdg = env::var("XDG_CURRENT_DESKTOP")
                .unwrap_or_default()
                .to_lowercase();
            if env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() || xdg.contains("hyprland") {
                if let Ok(b) = HyprlandBackend::new().await {
                    info!("desktop backend: hyprland (auto-detected)");
                    return Self {
                        backend: Arc::new(b),
                    };
                }
            }
        }

        #[cfg(feature = "kde")]
        {
            let xdg = env::var("XDG_CURRENT_DESKTOP")
                .unwrap_or_default()
                .to_lowercase();
            let kde_session = env::var("KDE_FULL_SESSION").is_ok()
                || xdg.contains("kde")
                || xdg.contains("plasma");
            if kde_session {
                if let Ok(b) = KdeBackend::new().await {
                    info!("desktop backend: kde (auto-detected)");
                    return Self {
                        backend: Arc::new(b),
                    };
                }
            }
        }

        warn!("no supported compositor detected; desktop control simulated");
        Self::init_mock()
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

    /// Name of the bound backend ("hyprland", "kde", "mock").
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    pub async fn get_context_summary(&self) -> String {
        match self.backend.get_active_window().await {
            Ok(Some(win)) => format!(
                "Active App: {} | Window: \\\"{}\\\" | Workspace: {}",
                win.app_id, win.title, win.workspace_id
            ),
            Ok(None) => "Active App: None (Empty Desktop)".to_string(),
            Err(e) => format!("Desktop Context Unavailable: {}", e),
        }
    }

    pub async fn dispatch_action(
        &self,
        action: &str,
        arg: Option<&str>,
    ) -> Result<String, DesktopError> {
        match action {
            "workspace_switch" | "switch_workspace" => {
                let target: i32 = arg
                    .ok_or_else(|| {
                        DesktopError::UnsupportedOperation("missing workspace target".into())
                    })?
                    .parse()
                    .map_err(|_| {
                        DesktopError::UnsupportedOperation("invalid workspace number".into())
                    })?;
                self.backend.switch_workspace(target).await?;
                Ok(format!("Switched to workspace {}", target))
            }
            "window_close" | "close_window" => {
                self.backend.close_window(arg).await?;
                Ok("Closed window".to_string())
            }
            "window_focus" | "focus_window" => {
                let win_id = arg.ok_or_else(|| {
                    DesktopError::UnsupportedOperation("missing window id".into())
                })?;
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
                    .ok_or_else(|| {
                        DesktopError::UnsupportedOperation("missing workspace target".into())
                    })?
                    .parse()
                    .map_err(|_| {
                        DesktopError::UnsupportedOperation("invalid workspace number".into())
                    })?;
                self.backend.move_window_to_workspace(None, target).await?;
                Ok(format!("Moved active window to workspace {}", target))
            }
            "get_context" => Ok(self.get_context_summary().await),
            "list_workspaces" => {
                let workspaces = self.backend.list_workspaces().await?;
                let summary = workspaces
                    .iter()
                    .map(|w| {
                        format!(
                            "{}: {}{}",
                            w.id,
                            w.name,
                            if w.is_active { " (active)" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                Ok(format!("Workspaces: [{}]", summary))
            }
            unsupported => Err(DesktopError::UnsupportedOperation(format!(
                "Action '{}' is not recognized",
                unsupported
            ))),
        }
    }
}
