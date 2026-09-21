//! Compositor action dispatcher and execution timing.

use crate::desktop::DesktopManager;
use crate::error::{Error, Result};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct DesktopExecutionResult {
    pub action_performed: String,
    pub execution_duration_micros: u64,
    pub output_message: String,
}

/// Resolves a spoken window name ("firefox", "the terminal") to a real
/// `WindowContext` by app-id/title substring match — longest title
/// wins on ties. Returns a routing error when nothing matches so the
/// pipeline can say so instead of acting on the wrong window.
async fn resolve_window(
    backend: &dyn crate::desktop::DesktopBackend,
    name: &str,
) -> Result<crate::desktop::WindowContext> {
    let needle = name.to_lowercase();
    let windows = backend
        .list_windows()
        .await
        .map_err(|e| Error::Routing(format!("window lookup failed: {e}")))?;
    windows
        .into_iter()
        .filter(|w| {
            w.id.to_lowercase() == needle
                || w.app_id.to_lowercase().contains(&needle)
                || w.title.to_lowercase().contains(&needle)
        })
        .max_by_key(|w| w.title.len())
        .ok_or_else(|| {
            Error::Routing(format!(
                "window not found: no open window matches '{}'",
                name
            ))
        })
}

pub struct DesktopExecutor {
    manager: DesktopManager,
}

impl DesktopExecutor {
    pub fn new(manager: DesktopManager) -> Self {
        Self { manager }
    }

    pub async fn execute(
        &self,
        intent: &str,
        parameter: Option<&str>,
    ) -> Result<DesktopExecutionResult> {
        let start = Instant::now();
        let backend = self.manager.backend();

        let output_message = match intent {
            i if i.starts_with("switch_workspace_") => {
                let ws_idx: i32 =
                    i.trim_start_matches("switch_workspace_")
                        .parse()
                        .map_err(|_| {
                            Error::Routing(format!(
                                "invalid workspace target in intent '{}'",
                                intent
                            ))
                        })?;
                backend.switch_workspace(ws_idx).await?;
                format!("Switched to workspace {}", ws_idx)
            }
            "focus_window" => {
                match parameter {
                    // Named target: resolve against real windows by
                    // app-id/title — never a fabricated window id.
                    Some(name) => {
                        let win = resolve_window(backend, name).await?;
                        backend.focus_window(&win.id).await?;
                        format!("Focused {}", win.title)
                    }
                    None => {
                        let win = backend.get_active_window().await?.ok_or_else(|| {
                            Error::Routing("no active window to focus".to_string())
                        })?;
                        backend.focus_window(&win.id).await?;
                        format!("Focused {}", win.title)
                    }
                }
            }
            "close_window" => {
                match parameter {
                    // "close the browser" must close the browser — not
                    // whatever window happens to be active.
                    Some(name) => {
                        let win = resolve_window(backend, name).await?;
                        backend.close_window(Some(&win.id)).await?;
                        format!("Closed {}", win.title)
                    }
                    None => {
                        backend.close_window(None).await?;
                        "Closed active window".to_string()
                    }
                }
            }
            "toggle_fullscreen" => {
                backend.toggle_fullscreen(parameter).await?;
                "Toggled fullscreen".to_string()
            }
            "volume_up" => {
                let n = parameter.and_then(|p| p.parse().ok()).unwrap_or(5);
                crate::execution::SystemExecutor::volume_up(n).await?;
                format!("Volume up {} percent", n)
            }
            "volume_down" => {
                let n = parameter.and_then(|p| p.parse().ok()).unwrap_or(5);
                crate::execution::SystemExecutor::volume_down(n).await?;
                format!("Volume down {} percent", n)
            }
            "volume_set" => {
                let n: u32 = parameter
                    .and_then(|p| p.parse().ok())
                    .ok_or_else(|| Error::Routing("volume_set needs a percentage".to_string()))?;
                crate::execution::SystemExecutor::volume_set(n).await?;
                format!("Volume set to {} percent", n.min(150))
            }
            "mute_audio" => {
                crate::execution::SystemExecutor::set_mute(true).await?;
                "Audio muted".to_string()
            }
            "unmute_audio" => {
                let was = crate::execution::SystemExecutor::is_muted()
                    .await
                    .unwrap_or(true);
                crate::execution::SystemExecutor::set_mute(false).await?;
                if was {
                    "Audio unmuted".to_string()
                } else {
                    "Audio was already unmuted".to_string()
                }
            }
            "launch_terminal" => {
                let app = parameter.unwrap_or("alacritty");
                crate::execution::SystemExecutor::launch(app).await?;
                format!("Launched application {}", app)
            }
            "launch_browser" => {
                let app = parameter.unwrap_or("firefox");
                crate::execution::SystemExecutor::launch(app).await?;
                format!("Launched application {}", app)
            }
            "launch_app" => {
                let app = parameter
                    .ok_or_else(|| Error::Routing("launch_app needs a program name".to_string()))?;
                crate::execution::SystemExecutor::launch(app).await?;
                format!("Launched application {}", app)
            }
            _ => format!("Executed {}", intent),
        };

        let elapsed = start.elapsed().as_micros() as u64;

        Ok(DesktopExecutionResult {
            action_performed: intent.to_string(),
            execution_duration_micros: elapsed,
            output_message,
        })
    }

    pub async fn context_summary(&self) -> String {
        let backend = self.manager.backend();
        let ws = backend
            .list_workspaces()
            .await
            .ok()
            .and_then(|list| list.into_iter().find(|w| w.is_active))
            .map(|w| w.id)
            .unwrap_or(1);
        let win = backend
            .get_active_window()
            .await
            .ok()
            .flatten()
            .map(|w| w.title)
            .unwrap_or_else(|| "Desktop".to_string());
        format!(
            "Compositor: {}, Active Window: '{}', Current Workspace: {}",
            backend.name(),
            win,
            ws
        )
    }
}
