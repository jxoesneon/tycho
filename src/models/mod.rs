//! Model lifecycle management, HuggingFace Hub downloader, and local caching.

pub mod hub;
pub mod manager;

pub use hub::HuggingFaceModelSpec;
pub use manager::{ModelInventory, ModelManager};
