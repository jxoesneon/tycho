//! Reference mock implementation for testing and development.

use super::traits::DesktopBackend;
use crate::error::Result;
use async_trait::async_trait;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Mutex;

pub struct MockBackend {
    pub current_workspace: AtomicU32,
    pub volume: AtomicU8,
    pub active_window: Mutex<String>,
}

impl MockBackend {
    pub fn new() -> Self {
        Self {
            current_workspace: AtomicU32::new(1),
            volume: AtomicU8::new(50),
            active_window: Mutex::new("Terminal".to_string()),
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
    async fn switch_workspace(&self, index: u32) -> Result<()> {
        self.current_workspace.store(index, Ordering::SeqCst);
        Ok(())
    }

    async fn focus_window(&self, title_or_class: &str) -> Result<()> {
        let mut win = self.active_window.lock().unwrap();
        *win = title_or_class.to_string();
        Ok(())
    }

    async fn close_active_window(&self) -> Result<()> {
        let mut win = self.active_window.lock().unwrap();
        *win = "Desktop".to_string();
        Ok(())
    }

    async fn toggle_fullscreen(&self) -> Result<()> {
        Ok(())
    }

    async fn set_volume(&self, percent: u8) -> Result<()> {
        self.volume.store(percent.min(100), Ordering::SeqCst);
        Ok(())
    }

    async fn launch_app(&self, app_name: &str) -> Result<()> {
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
