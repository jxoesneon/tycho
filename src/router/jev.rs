//! Semantic intent matching and fallback routing.

use super::desktop_intents::ParsedDesktopCommand;
use super::laya::LayaDecision;
use crate::error::Result;

pub struct JevSemanticRouter {
    pub api_key: Option<String>,
}

impl JevSemanticRouter {
    pub fn new(api_key: Option<String>) -> Self {
        Self { api_key }
    }

    pub async fn route(&self, query: &str, _options: &[&str]) -> Result<LayaDecision> {
        if let Some(cmd) = ParsedDesktopCommand::parse(query) {
            return Ok(LayaDecision {
                best_option: cmd.intent,
                confidence: 0.91,
            });
        }

        Ok(LayaDecision {
            best_option: "general_query".to_string(),
            confidence: 0.50,
        })
    }
}
