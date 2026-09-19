//! Non-autoregressive fast-path decision classifier.

use super::desktop_intents::ParsedDesktopCommand;

#[derive(Debug, Clone, PartialEq)]
pub struct LayaDecision {
    pub best_option: String,
    pub confidence: f32,
}

pub struct LayaDecisionModel {
    pub hf_repo: String,
}

impl LayaDecisionModel {
    pub fn new(hf_repo: impl Into<String>) -> Self {
        Self {
            hf_repo: hf_repo.into(),
        }
    }

    pub async fn select_option(&self, query: &str, _options: &[&str]) -> Option<LayaDecision> {
        if let Some(cmd) = ParsedDesktopCommand::parse(query) {
            return Some(LayaDecision {
                best_option: cmd.intent,
                confidence: 0.96,
            });
        }

        let q = query.to_lowercase();
        if q.contains("workspace") || q.contains("terminal") || q.contains("volume") || q.contains("close") {
            return Some(LayaDecision {
                best_option: "switch_workspace_1".to_string(),
                confidence: 0.88,
            });
        }

        None
    }
}
