//! System prompt construction and desktop context injection.

use super::personas::AssistantPersona;

pub struct PromptBuilder;

impl PromptBuilder {
    pub fn build(persona: AssistantPersona, desktop_context: &str, user_query: &str) -> (String, String) {
        let system_instruction = persona.system_instruction(desktop_context);
        let user_prompt = format!("Current Desktop State: [{}]\nUser Request: {}", desktop_context, user_query);
        (system_instruction, user_prompt)
    }
}
