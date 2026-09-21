//! Cognitive router arbitrating fast-path commands vs conversational queries.

pub mod desktop_intents;
pub mod jev;
pub mod laya;

pub use desktop_intents::{ParsedDesktopCommand, CANONICAL_DESKTOP_OPTIONS};
pub use jev::JevSemanticRouter;
pub use laya::{LayaDecision, LayaDecisionModel};

#[derive(Debug, Clone, PartialEq)]
pub enum RoutingTier {
    System1FastPath {
        intent: String,
        parameter: Option<String>,
        confidence: f32,
    },
    System2Deliberative {
        query: String,
    },
}

pub struct UnifiedRouter {
    pub laya: LayaDecisionModel,
    pub jev: JevSemanticRouter,
    pub fast_path_threshold: f32,
    /// Whether the local Laya heuristic tier runs between the strict
    /// grammar and the Jev LLM (`router.fallback_to_laya` in config).
    pub laya_enabled: bool,
}

impl UnifiedRouter {
    pub fn new(
        laya_repo: impl Into<String>,
        jev_key: Option<String>,
        jev_endpoint: impl Into<String>,
        jev_model: impl Into<String>,
        fast_path_threshold: f32,
    ) -> Self {
        Self {
            laya: LayaDecisionModel::new(laya_repo),
            jev: JevSemanticRouter::new(jev_key, jev_endpoint, jev_model),
            fast_path_threshold,
            laya_enabled: true,
        }
    }

    pub async fn route(&self, text: &str) -> RoutingTier {
        // Deterministic grammar first — an exact command phrase skips
        // both neural routers and their model round-trips entirely.
        if let Some(cmd) = ParsedDesktopCommand::parse(text) {
            return RoutingTier::System1FastPath {
                intent: cmd.intent,
                parameter: cmd.parameter,
                confidence: 1.0,
            };
        }

        if let Some(decision) = if self.laya_enabled {
            self.laya
                .select_option(text, CANONICAL_DESKTOP_OPTIONS)
                .await
        } else {
            None
        } {
            if decision.confidence >= self.fast_path_threshold
                && decision.best_option != "general_query"
            {
                let parameter = match ParsedDesktopCommand::parse(text).and_then(|p| p.parameter) {
                    Some(p) => Some(p),
                    None => self.laya.extract_parameter(text).await,
                };
                return RoutingTier::System1FastPath {
                    intent: decision.best_option,
                    parameter,
                    confidence: decision.confidence,
                };
            }
        }

        if let Ok(decision) = self.jev.route(text, CANONICAL_DESKTOP_OPTIONS).await {
            if decision.confidence >= self.fast_path_threshold
                && decision.best_option != "general_query"
            {
                let parameter = ParsedDesktopCommand::parse(text)
                    .and_then(|p| p.parameter)
                    .or_else(|| desktop_intents::neural_parameter(&decision.best_option, text));
                return RoutingTier::System1FastPath {
                    intent: decision.best_option,
                    parameter,
                    confidence: decision.confidence,
                };
            }
        }

        RoutingTier::System2Deliberative {
            query: text.to_string(),
        }
    }
}
