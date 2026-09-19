//! Model manager and HuggingFace URL tests.

use rust_voice_assistant::config::ModelsConfig;
use rust_voice_assistant::models::{HuggingFaceModelSpec, ModelManager};

#[tokio::test]
async fn test_huggingface_url_resolution() {
    let spec = HuggingFaceModelSpec::new(
        "onnx-community/whisper-tiny.en",
        "onnx/model.onnx",
        "whisper-tiny.en.onnx",
    );

    let url = spec.resolve_url("https://huggingface.co");
    assert_eq!(url, "https://huggingface.co/onnx-community/whisper-tiny.en/resolve/main/onnx/model.onnx");

    let sub_spec = HuggingFaceModelSpec::new("hexgrad/Kokoro-82M", "voices.bin", "voices.bin")
        .with_subfolder("voices")
        .with_revision("v0.19");

    let sub_url = sub_spec.resolve_url("https://huggingface.co/");
    assert_eq!(sub_url, "https://huggingface.co/hexgrad/Kokoro-82M/resolve/v0.19/voices/voices.bin");
}

#[tokio::test]
async fn test_model_manager_first_run_bootstrap() {
    let temp_dir = std::env::temp_dir().join("tycho_test_cache");
    let _ = std::fs::remove_dir_all(&temp_dir);

    let mut config = ModelsConfig::default();
    config.cache_dir = temp_dir.clone();
    config.auto_download_on_first_run = true;

    let manager = ModelManager::new(config);
    assert!(manager.is_first_run(), "First run must be true when cache directory is empty");

    let inventory = manager.ensure_models().await.expect("Model bootstrap should succeed");

    assert!(inventory.stt_model_path.exists());
    assert!(inventory.tts_model_path.exists());
    assert!(inventory.tts_voices_path.exists());

    assert!(!manager.is_first_run(), "Subsequent run should not trigger first-run flag");

    let _ = std::fs::remove_dir_all(&temp_dir);
}
