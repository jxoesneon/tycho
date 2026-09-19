//! Pipeline coordination and event definitions.

pub mod coordinator;
pub mod events;

pub use coordinator::TychoPipelineCoordinator;
pub use events::PipelineEvent;
