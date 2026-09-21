//! Modular compositor and desktop automation backends.
//!
//! Each compositor backend is feature-gated: `hyprland` enables the
//! Unix-socket IPC backend, `kde` the D-Bus/KWin backend (pulling in
//! `zbus`). With neither, `DesktopManager::init_auto` binds the mock.

#[cfg(feature = "hyprland")]
pub mod hyprland;
#[cfg(feature = "kde")]
pub mod kde;
pub mod manager;
pub mod mock;
pub mod traits;

#[cfg(feature = "hyprland")]
pub use hyprland::HyprlandBackend;
#[cfg(feature = "kde")]
pub use kde::KdeBackend;
pub use manager::DesktopManager;
pub use mock::MockBackend;
pub use traits::{
    BackendCapabilities, DesktopBackend, DesktopError, DesktopEvent, WindowContext, WindowGeometry,
    WorkspaceContext,
};
