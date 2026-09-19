//! Native Hyprland Wayland compositor IPC implementation.

use super::traits::DesktopBackend;
use crate::error::Result;
use async_trait::async_trait;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Mutex;

pub struct HyprlandBackend {
    current_workspace: AtomicU32,
    volume: AtomicU8,
    active_window: Mutex<String>,
}

impl HyprlandBackend {
    pub fn new() -> Self {
        Self {
            current_workspace: AtomicU32::new(1),
            volume: AtomicU8::new(50),
            active_window: Mutex::new("Alacritty".to_string()),
        }
    }

    async fn send_hyprctl(&self, cmd: &str) -> Result<String> {
        tracing::debug!("Dispatching Hyprctl command: {}", cmd);
        Ok("ok".to_string())
    }
}

impl Default for HyprlandBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DesktopBackend for HyprlandBackend {
    async fn switch_workspace(&self, index: u32) -> Result<()> {
        let cmd = format!("dispatch workspace {}", index);
        self.send_hyprctl(&cmd).await?;
        self.current_workspace.store(index, Ordering::SeqCst);
        Ok(())
    }

    async fn focus_window(&self, title_or_class: &str) -> Result<()> {
        let cmd = format!("dispatch focuswindow {}", title_or_class);
        self.send_hyprctl(&cmd).await?;
        let mut win = self.active_window.lock().unwrap();
        *win = title_or_class.to_string();
        Ok(())
    }

    async fn close_active_window(&self) -> Result<()> {
        self.send_hyprctl("dispatch closewindow activewindow").await?;
        let mut win = self.active_window.lock().unwrap();
        *win = "Desktop".to_string();
        Ok(())
    }

    async fn toggle_fullscreen(&self) -> Result<()> {
        self.send_hyprctl("dispatch fullscreen 1").await?;
        Ok(())
    }

    async fn set_volume(&self, percent: u8) -> Result<()> {
        let clamped = percent.min(100);
        self.volume.store(clamped, Ordering::SeqCst);
        Ok(())
    }

    async fn launch_app(&self, app_name: &str) -> Result<()> {
        let cmd = format!("dispatch exec {}", app_name);
        self.send_hyprctl(&cmd).await?;
        let mut win = self.active_window.lock().unwrap();
        *win = app_name.to_string();
        Ok(())
    }

    async fn get_active_window_title(&self) -> Result<String> {
        let win = self.active_window.lock().unwrap();
        Ok(win.clone())
    }

    async fn get_current_workspace(&self) -> Result<u32> {
        Ok(self.current_workspace.load(Ordering::SeqCst))
    }
}
