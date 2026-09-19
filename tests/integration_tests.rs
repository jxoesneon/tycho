//! Integration test suite for the voice assistant pipeline.

use rust_voice_assistant::audio::{VadState, VoiceActivityDetector};
use rust_voice_assistant::config::TychoConfig;
use rust_voice_assistant::pipeline::TychoPipelineCoordinator;

#[test]
fn test_config_defaults() {
    let config = TychoConfig::default();
    assert_eq!(config.audio.sample_rate, 16000);
    assert_eq!(config.vad.energy_threshold, 0.02);
    assert_eq!(config.router.fast_path_confidence_threshold, 0.82);
}

#[test]
fn test_vad_state_transitions() {
    let mut vad = VoiceActivityDetector::new(0.05, 2, 2);
    let silent = vec![0.001f32; 320];
    let loud = vec![0.15f32; 320];

    assert_eq!(vad.process_frame(&silent), VadState::Silence);
    vad.process_frame(&loud);
    assert_eq!(vad.process_frame(&loud), VadState::SpeechStart);
    assert_eq!(vad.process_frame(&loud), VadState::InSpeech);
    vad.process_frame(&silent);
    assert_eq!(vad.process_frame(&silent), VadState::SpeechEnd);
}

#[tokio::test]
async fn test_pipeline_coordinator_end_to_end() {
    let config = TychoConfig::default();
    let mut coordinator = TychoPipelineCoordinator::init(config).await.unwrap();

    let fast_res = coordinator.process_query("switch to workspace 2").await.unwrap();
    assert_eq!(fast_res, "Switched to workspace 2");

    let delib_res = coordinator.process_query("Explain gravitational waves").await.unwrap();
    assert!(delib_res.contains("Acknowledged"));
}
