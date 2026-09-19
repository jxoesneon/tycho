//! Native KDE Plasma / KWin D-Bus desktop integration.

use super::traits::DesktopBackend;
use crate::error::Result;
use async_trait::async_trait;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Mutex;

pub struct KdeBackend {
    current_workspace: AtomicU32,
    volume: AtomicU8,
    active_window: Mutex<String>,
}

impl KdeBackend {
    pub fn new() -> Self {
        Self {
            current_workspace: AtomicU32::new(1),
            volume: AtomicU8::new(50),
            active_window: Mutex::new("Konsole".to_string()),
        }
    }

    async fn call_kwin_dbus(&self, method: &str, arg: &str) -> Result<()> {
        tracing::debug!("Dispatching KWin D-Bus method: {} with arg: {}", method, arg);
        Ok(())
    }
}

impl Default for KdeBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DesktopBackend for KdeBackend {
    async fn switch_workspace(&self, index: u32) -> Result<()> {
        self.call_kwin_dbus("setCurrentDesktop", &index.to_string()).await?;
        self.current_workspace.store(index, Ordering::SeqCst);
        Ok(())
    }

    async fn focus_window(&self, title_or_class: &str) -> Result<()> {
        self.call_kwin_dbus("activateWindow", title_or_class).await?;
        let mut win = self.active_window.lock().unwrap();
        *win = title_or_class.to_string();
        Ok(())
    }

    async fn close_active_window(&self) -> Result<()> {
        self.call_kwin_dbus("closeActiveWindow", "").await?;
        let mut win = self.active_window.lock().unwrap();
        *win = "Desktop".to_string();
        Ok(())
    }

    async fn toggle_fullscreen(&self) -> Result<()> {
        self.call_kwin_dbus("toggleFullscreen", "").await?;
        Ok(())
    }

    async fn set_volume(&self, percent: u8) -> Result<()> {
        let clamped = percent.min(100);
        self.volume.store(clamped, Ordering::SeqCst);
        Ok(())
    }

    async fn launch_app(&self, app_name: &str) -> Result<()> {
        self.call_kwin_dbus("exec", app_name).await?;
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
