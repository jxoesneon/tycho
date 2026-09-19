//! Compositor backend interface definition.

use crate::error::Result;
use async_trait::async_trait;

#[async_trait]
pub trait DesktopBackend: Send + Sync {
    async fn switch_workspace(&self, index: u32) -> Result<()>;
    async fn focus_window(&self, title_or_class: &str) -> Result<()>;
    async fn close_active_window(&self) -> Result<()>;
    async fn toggle_fullscreen(&self) -> Result<()>;
    async fn set_volume(&self, percent: u8) -> Result<()>;
    async fn launch_app(&self, app_name: &str) -> Result<()>;
    async fn get_active_window_title(&self) -> Result<String>;
    async fn get_current_workspace(&self) -> Result<u32>;
}
