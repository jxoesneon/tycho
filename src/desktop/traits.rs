//! Core traits and domain models for unified desktop and compositor automation.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WindowContext {
    pub id: String,
    pub title: String,
    pub app_id: String,
    pub workspace_id: i32,
    pub is_floating: bool,
    pub is_fullscreen: bool,
    pub geometry: Option<WindowGeometry>,
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceContext {
    pub id: i32,
    pub name: String,
    pub is_active: bool,
    pub monitor: String,
    pub windows_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopEvent {
    ActiveWindowChanged(Option<WindowContext>),
    WorkspaceChanged(i32),
    WindowOpened(WindowContext),
    WindowClosed(String),
    MonitorChanged(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendCapabilities {
    pub supports_window_geometry: bool,
    pub supports_dynamic_workspaces: bool,
    pub supports_event_stream: bool,
    pub supports_nested_testing: bool,
    pub supports_window_floating: bool,
    pub supports_window_fullscreen: bool,
    pub supports_move_to_workspace: bool,
}

impl Default for BackendCapabilities {
    fn default() -> Self {
        Self {
            supports_window_geometry: true,
            supports_dynamic_workspaces: true,
            supports_event_stream: true,
            supports_nested_testing: false,
            supports_window_floating: true,
            supports_window_fullscreen: true,
            supports_move_to_workspace: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopError {
    ConnectionFailed(String),
    IpcError(String),
    UnsupportedOperation(String),
    WindowNotFound(String),
    WorkspaceNotFound(i32),
    Internal(String),
}

impl fmt::Display for DesktopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConnectionFailed(msg) => write!(f, "Desktop connection failed: {}", msg),
            Self::IpcError(msg) => write!(f, "Desktop IPC error: {}", msg),
            Self::UnsupportedOperation(msg) => write!(f, "Unsupported desktop operation: {}", msg),
            Self::WindowNotFound(id) => write!(f, "Window not found: {}", id),
            Self::WorkspaceNotFound(id) => write!(f, "Workspace not found: {}", id),
            Self::Internal(msg) => write!(f, "Internal desktop error: {}", msg),
        }
    }
}

impl std::error::Error for DesktopError {}

#[async_trait]
pub trait DesktopBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> &BackendCapabilities;

    async fn get_active_window(&self) -> Result<Option<WindowContext>, DesktopError>;
    async fn list_windows(&self) -> Result<Vec<WindowContext>, DesktopError>;
    async fn list_workspaces(&self) -> Result<Vec<WorkspaceContext>, DesktopError>;

    async fn switch_workspace(&self, target: i32) -> Result<(), DesktopError>;
    async fn focus_window(&self, window_id: &str) -> Result<(), DesktopError>;
    async fn close_window(&self, window_id: Option<&str>) -> Result<(), DesktopError>;
    async fn toggle_fullscreen(&self, window_id: Option<&str>) -> Result<(), DesktopError>;
    async fn toggle_floating(&self, window_id: Option<&str>) -> Result<(), DesktopError>;
    async fn move_window_to_workspace(
        &self,
        window_id: Option<&str>,
        target_workspace: i32,
    ) -> Result<(), DesktopError>;

    fn subscribe_events(&self) -> Result<broadcast::Receiver<DesktopEvent>, DesktopError>;
}
