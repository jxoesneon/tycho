//! Assistant personas defining tone, brevity, and behavioral styles.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantPersona {
    Sentinel,
    Scholar,
    Consigliere,
    Companion,
}

impl AssistantPersona {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Sentinel => "Sentinel",
            Self::Scholar => "Scholar",
            Self::Consigliere => "Consigliere",
            Self::Companion => "Companion",
        }
    }

    pub fn max_tokens(&self) -> u32 {
        match self {
            Self::Sentinel => 64,
            Self::Scholar => 1024,
            Self::Consigliere => 256,
            Self::Companion => 384,
        }
    }

    pub fn system_instruction(&self, desktop_context: &str) -> String {
        let base = match self {
            Self::Sentinel => {
                "Role: Sentinel. Mode: Tactical, immediate, ultra-terse. Focus strictly on executing actions. No pleasantries."
            }
            Self::Scholar => {
                "Role: Scholar. Mode: Analytical, deep, highly structured. Provide thorough explanations, trade-offs, and technical rationale."
            }
            Self::Consigliere => {
                "Role: Consigliere. Mode: Strategic executive partner. Provide high-signal concise advice, anticipate bottlenecks, and coordinate execution."
            }
            Self::Companion => {
                "Role: Companion. Mode: Approachable, adaptive, warm conversational partner. Maintain active dialogue and support daily flow."
            }
        };

        format!("{}\nActive Desktop Environment Context:\n{}", base, desktop_context)
    }
}

impl Default for AssistantPersona {
    fn default() -> Self {
        Self::Consigliere
    }
}
