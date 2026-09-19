//! Desktop compositor backend integration and abstractions.

pub mod hyprland;
pub mod kde;
pub mod manager;
pub mod mock;
pub mod traits;

pub use hyprland::HyprlandBackend;
pub use kde::KdeBackend;
pub use manager::DesktopManager;
pub use mock::MockBackend;
pub use traits::DesktopBackend;
