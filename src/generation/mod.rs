//! Conversational generation, persona conditioning, and prompt builders.

pub mod bootstrap;
pub mod client;
pub mod personas;
pub mod prompts;

pub use client::{ChatMessage, GenerationClient, GenerationResponse};
pub use personas::AssistantPersona;
pub use prompts::PromptBuilder;
