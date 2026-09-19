//! Compositor action dispatcher and execution timing.

use crate::desktop::DesktopManager;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct DesktopExecutionResult {
    pub action_performed: String,
    pub execution_duration_micros: u64,
    pub output_message: String,
}

pub struct DesktopExecutor {
    manager: DesktopManager,
}

impl DesktopExecutor {
    pub fn new(manager: DesktopManager) -> Self {
        Self { manager }
    }

    pub async fn execute(&self, intent: &str, parameter: Option<&str>) -> Result<DesktopExecutionResult, String> {
        let start = Instant::now();
        let backend = self.manager.backend();

        let output_message = match intent {
            i if i.starts_with("switch_workspace_") => {
                let ws_idx: u32 = i.trim_start_matches("switch_workspace_").parse().unwrap_or(1);
                backend.switch_workspace(ws_idx).await.map_err(|e| e.to_string())?;
                format!("Switched to workspace {}", ws_idx)
            }
            "focus_window" => {
                let win = parameter.unwrap_or("Alacritty");
                backend.focus_window(win).await.map_err(|e| e.to_string())?;
                format!("Focused window {}", win)
            }
            "close_window" => {
                backend.close_active_window().await.map_err(|e| e.to_string())?;
                "Closed active window".to_string()
            }
            "toggle_fullscreen" => {
                backend.toggle_fullscreen().await.map_err(|e| e.to_string())?;
                "Toggled fullscreen".to_string()
            }
            "volume_up" => {
                backend.set_volume(70).await.map_err(|e| e.to_string())?;
                "Volume increased".to_string()
            }
            "volume_down" => {
                backend.set_volume(30).await.map_err(|e| e.to_string())?;
                "Volume decreased".to_string()
            }
            "launch_terminal" => {
                let app = parameter.unwrap_or("alacritty");
                backend.launch_app(app).await.map_err(|e| e.to_string())?;
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
        let ws = backend.get_current_workspace().await.unwrap_or(1);
        let win = backend.get_active_window_title().await.unwrap_or_else(|_| "Desktop".to_string());
        format!("Compositor: {}, Active Window: '{}', Current Workspace: {}", self.manager.backend_name(), win, ws)
    }
}
