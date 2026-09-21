//! Sliding-window dialogue memory and persistent JSONL turn store.

pub mod context;
pub mod mempalace;

pub use context::{ConversationHistory, ConversationTurn};
pub use mempalace::MemPalaceClient;
