//! Modular compositor and desktop automation backends.

pub mod hyprland;
pub mod kde;
pub mod manager;
pub mod mock;
pub mod traits;

pub use hyprland::HyprlandBackend;
pub use kde::KdeBackend;
pub use manager::DesktopManager;
pub use mock::MockBackend;
pub use traits::{
    BackendCapabilities, DesktopBackend, DesktopError, DesktopEvent, WindowContext, WindowGeometry, WorkspaceContext,
};
