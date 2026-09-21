//! Generation layer tests: client, personas, and prompt builder.

use rust_voice_assistant::generation::{
    AssistantPersona, ChatMessage, GenerationClient, PromptBuilder,
};
use rust_voice_assistant::Error;

mod common;

#[tokio::test]
async fn test_generation_client() {
    let endpoint = common::openai_server("Acknowledged");
    let client = GenerationClient::new("local-chat", endpoint.clone(), Some("key".into()));
    assert_eq!(client.model, "local-chat");
    assert_eq!(client.endpoint, endpoint);
    assert_eq!(client.api_key.as_deref(), Some("key"));

    let resp = client
        .generate("hello", "system prompt here")
        .await
        .unwrap();
    assert!(resp.content.contains("Acknowledged"));
    assert_eq!(resp.model, "local-chat");

    let err = client.generate("", "sys").await.unwrap_err();
    assert!(matches!(err, Error::Inference(_)));
}

#[tokio::test]
async fn test_generation_client_connection_failure() {
    let client = GenerationClient::new("m", "http://127.0.0.1:1", None);
    let err = client.generate("hello", "sys").await.unwrap_err();
    assert!(matches!(err, Error::Inference(_)));
    assert!(err.to_string().contains("request"));
}

#[tokio::test]
async fn test_generation_client_http_error() {
    let endpoint = common::http_server(b"model not found", "404 Not Found");
    let client = GenerationClient::new("m", endpoint, None);
    let err = client.generate("hello", "sys").await.unwrap_err();
    assert!(matches!(err, Error::Inference(_)));
    assert!(err.to_string().contains("404"));
}

#[tokio::test]
async fn test_generation_client_malformed_json() {
    let endpoint = common::http_server(b"not json at all", "200 OK");
    let client = GenerationClient::new("m", endpoint, None);
    let err = client.generate("hello", "sys").await.unwrap_err();
    assert!(matches!(err, Error::Inference(_)));
    assert!(err.to_string().contains("malformed"));
}

#[tokio::test]
async fn test_generation_client_missing_choices() {
    let endpoint = common::http_server(br#"{"choices":[]}"#, "200 OK");
    let client = GenerationClient::new("m", endpoint, None);
    let err = client.generate("hello", "sys").await.unwrap_err();
    assert!(matches!(err, Error::Inference(_)));
    assert!(err.to_string().contains("choices"));
}

#[test]
fn test_chat_message_and_response_serde() {
    let msg = ChatMessage {
        role: "user".into(),
        content: "hi".into(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: ChatMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(back.role, "user");
    assert_eq!(back.content, "hi");

    let resp: rust_voice_assistant::generation::GenerationResponse =
        serde_json::from_str(r#"{"content":"c","model":"m"}"#).unwrap();
    assert_eq!(resp.content, "c");
}

#[test]
fn test_persona_metadata() {
    assert_eq!(AssistantPersona::Sentinel.name(), "Sentinel");
    assert_eq!(AssistantPersona::Scholar.name(), "Scholar");
    assert_eq!(AssistantPersona::Consigliere.name(), "Consigliere");
    assert_eq!(AssistantPersona::Companion.name(), "Companion");

    assert_eq!(AssistantPersona::Sentinel.max_tokens(), 64);
    assert_eq!(AssistantPersona::Scholar.max_tokens(), 1024);
    assert_eq!(AssistantPersona::Consigliere.max_tokens(), 256);
    assert_eq!(AssistantPersona::Companion.max_tokens(), 384);

    assert_eq!(AssistantPersona::default(), AssistantPersona::Consigliere);
}

#[test]
fn test_persona_system_instructions() {
    let sentinel = AssistantPersona::Sentinel.system_instruction("ctx");
    assert!(sentinel.contains("Role: Sentinel"));
    assert!(sentinel.contains("ctx"));

    let scholar = AssistantPersona::Scholar.system_instruction("ctx");
    assert!(scholar.contains("Role: Scholar"));

    let consigliere = AssistantPersona::Consigliere.system_instruction("ctx");
    assert!(consigliere.contains("Role: Consigliere"));

    let companion = AssistantPersona::Companion.system_instruction("ctx");
    assert!(companion.contains("Role: Companion"));
}

#[test]
fn test_persona_serde() {
    let p: AssistantPersona = serde_json::from_str(r#""scholar""#).unwrap();
    assert_eq!(p, AssistantPersona::Scholar);
    assert_eq!(
        serde_json::to_string(&AssistantPersona::Companion).unwrap(),
        "\"companion\""
    );
}

#[test]
fn test_prompt_builder() {
    let (sys, user) = PromptBuilder::build(AssistantPersona::Scholar, "ws-1 active", "what is up");
    assert!(sys.contains("Role: Scholar"));
    assert!(sys.contains("ws-1 active"));
    assert!(user.contains("ws-1 active"));
    assert!(user.contains("what is up"));
}
