//! Tycho: Voice control daemon for Linux compositors.

pub mod audio;
pub mod config;
pub mod desktop;
pub mod error;
pub mod execution;
pub mod generation;
pub mod memory;
pub mod models;
pub mod pipeline;
pub mod router;
pub mod stt;
pub mod tts;
#[cfg(feature = "orb")]
pub mod ui;
pub mod wake;

pub use config::{ModelSpecConfig, ModelsConfig, TychoConfig};
#[cfg(feature = "hyprland")]
pub use desktop::HyprlandBackend;
#[cfg(feature = "kde")]
pub use desktop::KdeBackend;
pub use desktop::{
    BackendCapabilities, DesktopBackend, DesktopError, DesktopEvent, DesktopManager, MockBackend,
    WindowContext, WindowGeometry, WorkspaceContext,
};
pub use error::{Error, Result};
pub use execution::{DesktopExecutionResult, DesktopExecutor, SystemCommandResult, SystemExecutor};
pub use generation::AssistantPersona;
pub use models::{HuggingFaceModelSpec, ModelInventory, ModelManager};
pub use pipeline::{PipelineEvent, TychoPipelineCoordinator};
pub use router::{ParsedDesktopCommand, RoutingTier, UnifiedRouter, CANONICAL_DESKTOP_OPTIONS};

/// Serializes tests that mutate process env vars (HOME,
/// TYCHO_NO_INSTALL) — lib tests share one process and race
/// without this lock.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
