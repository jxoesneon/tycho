//! Router tests: intent parsing, laya/jev tiers, and unified routing.

use rust_voice_assistant::router::{
    JevSemanticRouter, LayaDecisionModel, ParsedDesktopCommand, RoutingTier, UnifiedRouter,
    CANONICAL_DESKTOP_OPTIONS,
};

#[test]
fn test_parse_workspace_intents() {
    let cmd = ParsedDesktopCommand::parse("switch to workspace 2").unwrap();
    assert_eq!(cmd.intent, "switch_workspace_2");
    assert_eq!(cmd.parameter.as_deref(), Some("2"));

    let cmd = ParsedDesktopCommand::parse("go to workspace 9 please").unwrap();
    assert_eq!(cmd.intent, "switch_workspace_9");

    let cmd = ParsedDesktopCommand::parse("workspace5").unwrap();
    assert_eq!(cmd.intent, "switch_workspace_5");

    let cmd = ParsedDesktopCommand::parse("switch to workspace 10").unwrap();
    assert_eq!(cmd.intent, "switch_workspace_10");
    assert_eq!(cmd.parameter.as_deref(), Some("10"));

    // STT output spells numbers and may split "workspace".
    let cmd = ParsedDesktopCommand::parse("switch to workspace two").unwrap();
    assert_eq!(cmd.intent, "switch_workspace_2");
    assert_eq!(cmd.parameter.as_deref(), Some("2"));

    let cmd = ParsedDesktopCommand::parse("go to work space three").unwrap();
    assert_eq!(cmd.intent, "switch_workspace_3");

    let cmd = ParsedDesktopCommand::parse("workspace four.").unwrap();
    assert_eq!(cmd.intent, "switch_workspace_4");

    assert!(ParsedDesktopCommand::parse("open workspace").is_none());
    assert!(ParsedDesktopCommand::parse("the workspace is cluttered").is_none());
}

#[test]
fn test_parse_focus_intent() {
    let cmd = ParsedDesktopCommand::parse("focus terminal window").unwrap();
    assert_eq!(cmd.intent, "focus_window");
    assert_eq!(cmd.parameter.as_deref(), Some("terminal window"));

    assert!(ParsedDesktopCommand::parse("please focus").is_none());
    assert!(ParsedDesktopCommand::parse("autofocus the lens").is_none());
}

#[test]
fn test_parse_window_and_media_intents() {
    let cmd = ParsedDesktopCommand::parse("close window").unwrap();
    assert_eq!(cmd.intent, "close_window");
    assert!(cmd.parameter.is_none());

    let cmd = ParsedDesktopCommand::parse("close this").unwrap();
    assert_eq!(cmd.intent, "close_window");

    let cmd = ParsedDesktopCommand::parse("go fullscreen").unwrap();
    assert_eq!(cmd.intent, "toggle_fullscreen");

    let cmd = ParsedDesktopCommand::parse("volume up").unwrap();
    assert_eq!(cmd.intent, "volume_up");

    let cmd = ParsedDesktopCommand::parse("volume down").unwrap();
    assert_eq!(cmd.intent, "volume_down");

    let cmd = ParsedDesktopCommand::parse("close the app now").unwrap();
    assert_eq!(cmd.intent, "close_window");

    let cmd = ParsedDesktopCommand::parse("mute").unwrap();
    assert_eq!(cmd.intent, "mute_audio");

    let cmd = ParsedDesktopCommand::parse("open browser").unwrap();
    assert_eq!(cmd.intent, "launch_browser");

    // Bare "close" counts as an explicit close request; "close" without a
    // closeable target does not.
    let cmd = ParsedDesktopCommand::parse("close").unwrap();
    assert_eq!(cmd.intent, "close_window");
    assert!(ParsedDesktopCommand::parse("close please").is_none());
}

#[test]
fn test_parse_launch_and_unmatched() {
    let cmd = ParsedDesktopCommand::parse("launch terminal").unwrap();
    assert_eq!(cmd.intent, "launch_terminal");
    assert_eq!(cmd.parameter.as_deref(), Some("alacritty"));

    let cmd = ParsedDesktopCommand::parse("open terminal").unwrap();
    assert_eq!(cmd.intent, "launch_terminal");

    assert!(ParsedDesktopCommand::parse("explain quantum mechanics").is_none());
}

#[test]
fn test_canonical_options_contents() {
    assert!(CANONICAL_DESKTOP_OPTIONS.contains(&"switch_workspace_1"));
    assert!(CANONICAL_DESKTOP_OPTIONS.contains(&"general_query"));
    assert!(CANONICAL_DESKTOP_OPTIONS.contains(&"unmute_audio"));
    assert!(CANONICAL_DESKTOP_OPTIONS.contains(&"launch_app"));
    assert!(CANONICAL_DESKTOP_OPTIONS.contains(&"stop_assistant"));
    assert!(CANONICAL_DESKTOP_OPTIONS.contains(&"repeat_response"));
    assert_eq!(CANONICAL_DESKTOP_OPTIONS.len(), 19);
}

#[tokio::test]
async fn test_laya_decision_model() {
    let laya = LayaDecisionModel::new("convaiinnovations/laya");
    assert_eq!(laya.hf_repo, "convaiinnovations/laya");

    let decision = laya
        .select_option("switch to workspace 3", CANONICAL_DESKTOP_OPTIONS)
        .await
        .unwrap();
    assert_eq!(decision.best_option, "switch_workspace_3");
    assert!(decision.confidence >= 0.9);

    let decision = laya
        .select_option("close the app now", CANONICAL_DESKTOP_OPTIONS)
        .await
        .unwrap();
    assert_eq!(decision.best_option, "close_window");

    assert!(laya
        .select_option("tell me a joke", CANONICAL_DESKTOP_OPTIONS)
        .await
        .is_none());
}

mod common;

const DEAD_ENDPOINT: &str = "http://127.0.0.1:1";

#[tokio::test]
async fn test_jev_semantic_router() {
    // Content with an opening brace but no closing brace → rfind('}') err.
    let jev = JevSemanticRouter::new(
        Some("k".into()),
        common::openai_server("{ unterminated json"),
        "m",
    );
    let err = jev
        .route("switch to workspace 2", CANONICAL_DESKTOP_OPTIONS)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no JSON"));

    let jev = JevSemanticRouter::new(Some("k".into()), common::jev_server("volume_up", 0.93), "m");
    assert_eq!(jev.api_key.as_deref(), Some("k"));

    let decision = jev
        .route("volume up", CANONICAL_DESKTOP_OPTIONS)
        .await
        .unwrap();
    assert_eq!(decision.best_option, "volume_up");
    assert!(decision.confidence > 0.9);

    let jev = JevSemanticRouter::new(None, common::jev_server("general_query", 0.2), "m");
    let decision = jev
        .route("what is the weather", CANONICAL_DESKTOP_OPTIONS)
        .await
        .unwrap();
    assert_eq!(decision.best_option, "general_query");
}

#[tokio::test]
async fn test_jev_semantic_router_error_edges() {
    // Unreachable endpoint.
    let jev = JevSemanticRouter::new(None, DEAD_ENDPOINT, "m");
    assert!(jev.route("hi", CANONICAL_DESKTOP_OPTIONS).await.is_err());

    // Non-2xx status.
    let jev = JevSemanticRouter::new(
        None,
        common::http_server(b"err", "500 Internal Server Error"),
        "m",
    );
    assert!(jev.route("hi", CANONICAL_DESKTOP_OPTIONS).await.is_err());

    // Response missing choices array.
    let jev = JevSemanticRouter::new(
        None,
        common::http_server(br#"{"choices":[]}"#, "200 OK"),
        "m",
    );
    assert!(jev.route("hi", CANONICAL_DESKTOP_OPTIONS).await.is_err());

    // Content without any JSON object.
    let jev = JevSemanticRouter::new(None, common::openai_server("no json here"), "m");
    assert!(jev.route("hi", CANONICAL_DESKTOP_OPTIONS).await.is_err());

    // Content with braces that are not valid JSON.
    let jev = JevSemanticRouter::new(None, common::openai_server("{bad json}"), "m");
    assert!(jev.route("hi", CANONICAL_DESKTOP_OPTIONS).await.is_err());

    // 200 reply whose body is not JSON at all → malformed-body error.
    let jev = JevSemanticRouter::new(
        None,
        common::http_server(b"this is not json", "200 OK"),
        "m",
    );
    let err = jev
        .route("hi", CANONICAL_DESKTOP_OPTIONS)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("malformed response body"));

    // Unknown option falls back to general_query with capped confidence.
    let jev = JevSemanticRouter::new(None, common::jev_server("bogus_intent", 0.95), "m");
    let decision = jev.route("hi", CANONICAL_DESKTOP_OPTIONS).await.unwrap();
    assert_eq!(decision.best_option, "general_query");
    assert!(decision.confidence <= 0.5);
}

#[tokio::test]
async fn test_unified_router_fast_path() {
    let router = UnifiedRouter::new("repo", None, DEAD_ENDPOINT, "m", 0.82);
    match router.route("switch to workspace 2").await {
        RoutingTier::System1FastPath {
            intent,
            parameter,
            confidence,
        } => {
            assert_eq!(intent, "switch_workspace_2");
            assert_eq!(parameter.as_deref(), Some("2"));
            assert!(confidence >= 0.82);
        }
        other => panic!("expected fast path, got {:?}", other),
    }
}

#[tokio::test]
async fn test_unified_router_deliberative_paths() {
    let jev_endpoint = common::jev_server("general_query", 0.2);
    let router = UnifiedRouter::new("repo", None, &jev_endpoint, "m", 0.82);

    match router.route("explain gravitational waves").await {
        RoutingTier::System2Deliberative { query } => {
            assert_eq!(query, "explain gravitational waves")
        }
        other => panic!("expected deliberative, got {:?}", other),
    }

    match router.route("close the app now").await {
        RoutingTier::System1FastPath { intent, .. } => assert_eq!(intent, "close_window"),
        other => panic!("expected close_window fast path, got {:?}", other),
    }

    // A low-confidence JEV verdict on an unparseable phrase stays
    // deliberative even when the intent seems obvious — the threshold
    // only gates neural confidence, not the deterministic grammar.
    let strict = UnifiedRouter::new("repo", None, &jev_endpoint, "m", 0.99);
    match strict
        .route("could you move me to the fourth workspace")
        .await
    {
        RoutingTier::System2Deliberative { query } => {
            assert_eq!(query, "could you move me to the fourth workspace")
        }
        other => panic!(
            "expected deliberative under strict threshold, got {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn test_unified_router_jev_fast_path() {
    // Laya cannot parse "take me to space 4" (no "workspace" token), so the
    // query reaches JEV, whose semantic verdict fast-paths it.
    let router = UnifiedRouter::new(
        "repo",
        None,
        common::jev_server("switch_workspace_4", 0.95),
        "m",
        0.82,
    );
    match router.route("take me to space 4").await {
        RoutingTier::System1FastPath {
            intent, confidence, ..
        } => {
            assert_eq!(intent, "switch_workspace_4");
            assert!(confidence >= 0.82);
        }
        other => panic!("expected jev fast path, got {:?}", other),
    }
}

#[tokio::test]
async fn test_unified_router_jev_general_query_edge() {
    let permissive = UnifiedRouter::new(
        "repo",
        None,
        common::jev_server("general_query", 0.2),
        "m",
        0.5,
    );
    match permissive.route("tell me about the moon").await {
        RoutingTier::System2Deliberative { query } => {
            assert_eq!(query, "tell me about the moon")
        }
        other => panic!(
            "expected deliberative via jev general_query, got {:?}",
            other
        ),
    }
}
