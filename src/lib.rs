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

pub use config::{ModelsConfig, ModelSpecConfig, TychoConfig};
pub use desktop::{DesktopBackend, DesktopManager, HyprlandBackend, KdeBackend, MockBackend};
pub use error::{Error, Result};
pub use execution::{DesktopExecutionResult, DesktopExecutor, SystemCommandResult, SystemExecutor};
pub use generation::AssistantPersona;
pub use models::{HuggingFaceModelSpec, ModelInventory, ModelManager};
pub use pipeline::{PipelineEvent, TychoPipelineCoordinator};
pub use router::{ParsedDesktopCommand, RoutingTier, UnifiedRouter, CANONICAL_DESKTOP_OPTIONS};
