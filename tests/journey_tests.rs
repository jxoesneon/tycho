//! User workflow tests.

use rust_voice_assistant::audio::AudioPlaybackSink;
use rust_voice_assistant::desktop::DesktopManager;
use rust_voice_assistant::execution::DesktopExecutor;
use rust_voice_assistant::pipeline::TychoPipelineCoordinator;

mod common;

#[tokio::test]
async fn test_journey_fast_path_and_barge_in() {
    let (config, _cache) = common::seeded_cache_config("journey");
    let mut coordinator = TychoPipelineCoordinator::init(config).await.unwrap();
    // Keep compositor actions on the mock backend — the real backend would
    // dispatch live window-manager commands on hosts running Hyprland/KDE.
    coordinator.executor = DesktopExecutor::new(DesktopManager::init_mock());

    let res = coordinator
        .process_query("switch to workspace 4")
        .await
        .unwrap();
    assert_eq!(res, "Switched to workspace 4");

    let sink = AudioPlaybackSink::new();
    sink.interrupt();
    assert!(sink.is_interrupted());
}
