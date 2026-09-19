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
}

impl UnifiedRouter {
    pub fn new(laya_repo: impl Into<String>, jev_key: Option<String>, fast_path_threshold: f32) -> Self {
        Self {
            laya: LayaDecisionModel::new(laya_repo),
            jev: JevSemanticRouter::new(jev_key),
            fast_path_threshold,
        }
    }

    pub async fn route(&self, text: &str) -> RoutingTier {
        if let Some(decision) = self.laya.select_option(text, CANONICAL_DESKTOP_OPTIONS).await {
            if decision.confidence >= self.fast_path_threshold && decision.best_option != "general_query" {
                let parsed = ParsedDesktopCommand::parse(text);
                return RoutingTier::System1FastPath {
                    intent: decision.best_option,
                    parameter: parsed.and_then(|p| p.parameter),
                    confidence: decision.confidence,
                };
            }
        }

        if let Ok(decision) = self.jev.route(text, CANONICAL_DESKTOP_OPTIONS).await {
            if decision.confidence >= self.fast_path_threshold && decision.best_option != "general_query" {
                let parsed = ParsedDesktopCommand::parse(text);
                return RoutingTier::System1FastPath {
                    intent: decision.best_option,
                    parameter: parsed.and_then(|p| p.parameter),
                    confidence: decision.confidence,
                };
            }
        }

        RoutingTier::System2Deliberative {
            query: text.to_string(),
        }
    }
}
