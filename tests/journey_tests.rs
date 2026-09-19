//! User workflow tests.

use rust_voice_assistant::audio::AudioPlaybackSink;
use rust_voice_assistant::config::TychoConfig;
use rust_voice_assistant::pipeline::TychoPipelineCoordinator;

#[tokio::test]
async fn test_journey_fast_path_and_barge_in() {
    let config = TychoConfig::default();
    let mut coordinator = TychoPipelineCoordinator::init(config).await.unwrap();

    let res = coordinator.process_query("switch to workspace 4").await.unwrap();
    assert_eq!(res, "Switched to workspace 4");

    let sink = AudioPlaybackSink::new();
    sink.interrupt();
    assert!(sink.is_interrupted());
}
