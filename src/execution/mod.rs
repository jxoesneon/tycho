//! Execution engines for compositor commands and sandboxed system tasks.

pub mod desktop;
pub mod system;

pub use desktop::{DesktopExecutionResult, DesktopExecutor};
pub use system::{SystemCommandResult, SystemExecutor};
