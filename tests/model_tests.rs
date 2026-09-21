//! Model manager and HuggingFace URL tests.

use rust_voice_assistant::config::{ModelSpecConfig, ModelsConfig};
use rust_voice_assistant::models::{HuggingFaceModelSpec, ModelManager};

mod common;

use common::model_weights as weights;

#[tokio::test]
async fn test_huggingface_url_resolution() {
    let spec = HuggingFaceModelSpec::new(
        "onnx-community/whisper-tiny.en",
        "onnx/model.onnx",
        "whisper-tiny.en.onnx",
    );

    let url = spec.resolve_url("https://huggingface.co");
    assert_eq!(
        url,
        "https://huggingface.co/onnx-community/whisper-tiny.en/resolve/main/onnx/model.onnx"
    );

    let sub_spec = HuggingFaceModelSpec::new("hexgrad/Kokoro-82M", "voices.bin", "voices.bin")
        .with_subfolder("voices")
        .with_revision("v0.19");

    let sub_url = sub_spec.resolve_url("https://huggingface.co/");
    assert_eq!(
        sub_url,
        "https://huggingface.co/hexgrad/Kokoro-82M/resolve/v0.19/voices/voices.bin"
    );
}

#[tokio::test]
async fn test_model_manager_first_run_bootstrap() {
    let temp_dir = std::env::temp_dir().join("tycho_test_cache");
    let _ = std::fs::remove_dir_all(&temp_dir);

    let config = ModelsConfig {
        cache_dir: temp_dir.clone(),
        auto_download_on_first_run: true,
        hf_endpoint: common::hf_server(weights()),
        hf_token: Some("test-hf-token".into()),
        ..common::test_models_config()
    };

    let manager = ModelManager::new(config);
    assert!(
        manager.is_first_run(),
        "First run must be true when cache directory is empty"
    );

    let inventory = manager
        .ensure_models()
        .await
        .expect("Model bootstrap should succeed");

    assert!(inventory.stt_model_path.exists());
    assert!(inventory.tts_model_path.exists());
    assert!(inventory.tts_voices_path.exists());

    assert!(
        !manager.is_first_run(),
        "Subsequent run should not trigger first-run flag"
    );

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_model_manager_second_run_skips_pull() {
    let temp_dir = std::env::temp_dir().join(format!("tycho_test_second_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);

    let config = ModelsConfig {
        cache_dir: temp_dir.clone(),
        auto_download_on_first_run: false,
        hf_endpoint: common::hf_server(weights()),
        ..common::test_models_config()
    };

    let manager = ModelManager::new(config.clone());
    let err = manager.ensure_models().await.unwrap_err();
    assert!(err.to_string().contains("auto-download disabled"));

    let mut seeded = config.clone();
    seeded.auto_download_on_first_run = true;
    ModelManager::new(seeded).ensure_models().await.unwrap();

    let manager = ModelManager::new(config);
    assert!(!manager.is_first_run());
    let inventory = manager.ensure_models().await.unwrap();
    assert!(inventory.router_model_path.is_some());

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_model_manager_paths_and_tilde() {
    let config = ModelsConfig {
        cache_dir: std::path::PathBuf::from("~/tycho-test-cache"),
        ..Default::default()
    };
    let manager = ModelManager::new(config);

    let resolved = manager.resolved_cache_dir();
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    assert_eq!(resolved, home.join("tycho-test-cache"));

    assert!(manager
        .stt_model_target_path()
        .ends_with("stt/ggml-tiny.en.bin"));
    assert!(manager
        .tts_model_target_path()
        .ends_with("tts/en_US-amy-medium.onnx"));
    assert!(manager
        .tts_voices_target_path()
        .ends_with("tts/en_US-amy-medium.onnx.json"));
    assert!(manager
        .router_model_target_path()
        .unwrap()
        .ends_with("router/laya_intent_classifier.onnx"));

    let abs = ModelsConfig {
        cache_dir: std::path::PathBuf::from("/tmp/tycho-abs-cache"),
        router_model: None,
        ..Default::default()
    };
    let abs_manager = ModelManager::new(abs);
    assert_eq!(
        abs_manager.resolved_cache_dir(),
        std::path::PathBuf::from("/tmp/tycho-abs-cache")
    );
    assert!(abs_manager.router_model_target_path().is_none());
}

#[tokio::test]
async fn test_model_manager_pull_failure() {
    let config = ModelsConfig::default();
    let manager = ModelManager::new(config);

    let spec = HuggingFaceModelSpec::new("org/repo", "f.onnx", "t.onnx");
    let target = std::path::PathBuf::from("/proc/tycho-impossible-dir/model.onnx");
    let err = manager.pull_model_spec(&spec, &target).await.unwrap_err();
    assert!(
        err.to_string().contains("model download")
            || matches!(err, rust_voice_assistant::Error::Io(_))
    );
}

#[tokio::test]
async fn test_model_manager_pull_download_error() {
    use std::os::unix::fs::PermissionsExt;

    let config = ModelsConfig {
        hf_endpoint: common::hf_server(weights()),
        ..Default::default()
    };
    let manager = ModelManager::new(config);
    let spec = HuggingFaceModelSpec::new("org/repo", "f.onnx", "t.onnx");

    let dir = std::env::temp_dir().join(format!("tycho-nowrite-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();

    let target = dir.join("model.onnx");
    let err = manager.pull_model_spec(&spec, &target).await.unwrap_err();
    assert!(matches!(
        err,
        rust_voice_assistant::Error::ModelDownload { .. }
    ));

    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_model_manager_pull_error_edges() {
    let pid = std::process::id();

    // ensure_models: cache_root creation fails when a path component is a file.
    let blocker = std::env::temp_dir().join(format!("tycho-blocker-{pid}"));
    std::fs::write(&blocker, b"file-not-dir").unwrap();
    let cfg = ModelsConfig {
        cache_dir: blocker.join("nested"),
        auto_download_on_first_run: true,
        ..Default::default()
    };
    assert!(ModelManager::new(cfg).ensure_models().await.is_err());
    let _ = std::fs::remove_file(&blocker);

    // stt pull failure propagates through ensure_models.
    let dir = std::env::temp_dir().join(format!("tycho-sttfail-{pid}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("stt"), b"file-not-dir").unwrap();
    let cfg = ModelsConfig {
        cache_dir: dir.clone(),
        auto_download_on_first_run: true,
        ..Default::default()
    };
    assert!(ModelManager::new(cfg).ensure_models().await.is_err());
    let _ = std::fs::remove_dir_all(&dir);

    // tts pull failure after stt is already seeded.
    let dir = std::env::temp_dir().join(format!("tycho-ttsfail-{pid}"));
    let _ = std::fs::remove_dir_all(&dir);
    let cfg = ModelsConfig {
        cache_dir: dir.clone(),
        auto_download_on_first_run: true,
        ..Default::default()
    };
    let probe = ModelManager::new(cfg.clone());
    std::fs::create_dir_all(probe.stt_model_target_path().parent().unwrap()).unwrap();
    std::fs::write(probe.stt_model_target_path(), b"seeded-stt").unwrap();
    std::fs::write(dir.join("tts"), b"file-not-dir").unwrap();
    assert!(probe.ensure_models().await.is_err());
    let _ = std::fs::remove_dir_all(&dir);

    // voices pull failure via a directory occupying the temp download path.
    let dir = std::env::temp_dir().join(format!("tycho-voicesfail-{pid}"));
    let _ = std::fs::remove_dir_all(&dir);
    let cfg = ModelsConfig {
        cache_dir: dir.clone(),
        auto_download_on_first_run: true,
        hf_endpoint: common::hf_server(weights()),
        ..Default::default()
    };
    let probe = ModelManager::new(cfg);
    std::fs::create_dir_all(probe.stt_model_target_path().parent().unwrap()).unwrap();
    std::fs::write(probe.stt_model_target_path(), b"seeded-stt").unwrap();
    std::fs::create_dir_all(probe.tts_model_target_path().parent().unwrap()).unwrap();
    std::fs::write(probe.tts_model_target_path(), b"seeded-tts").unwrap();
    let voices_tmp = probe
        .tts_voices_target_path()
        .with_extension("tmp_download");
    std::fs::create_dir_all(&voices_tmp).unwrap();
    assert!(probe.ensure_models().await.is_err());
    let _ = std::fs::remove_dir_all(&dir);

    let manager = ModelManager::new(ModelsConfig {
        hf_endpoint: common::hf_server(weights()),
        ..Default::default()
    });
    let spec = HuggingFaceModelSpec::new("org/repo", "f.onnx", "t.onnx");

    // rename failure: target path exists as a directory.
    let dir_target = std::env::temp_dir().join(format!("tycho-dirtarget-{pid}"));
    let _ = std::fs::remove_dir_all(&dir_target);
    std::fs::create_dir_all(&dir_target).unwrap();
    std::fs::write(dir_target.join("inner"), b"x").unwrap();
    assert!(manager.pull_model_spec(&spec, &dir_target).await.is_err());
    let _ = std::fs::remove_file(dir_target.with_extension("tmp_download"));
    let _ = std::fs::remove_dir_all(&dir_target);

    // root path has no parent: falls back to the current directory.
    assert!(manager
        .pull_model_spec(&spec, std::path::Path::new("/"))
        .await
        .is_err());
    let _ = std::fs::remove_file(std::path::Path::new("/").with_extension("tmp_download"));

    // expected_min_bytes smaller than the served body: download succeeds.
    let ok_dir = std::env::temp_dir().join(format!("tycho-smallmin-{pid}"));
    let _ = std::fs::remove_dir_all(&ok_dir);
    let small = HuggingFaceModelSpec::new("org/repo", "f.onnx", "t.onnx").with_min_bytes(1);
    manager
        .pull_model_spec(&small, &ok_dir.join("t.onnx"))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(ok_dir.join("t.onnx")).unwrap().len(),
        weights().len()
    );
    let _ = std::fs::remove_dir_all(&ok_dir);
}

#[tokio::test]
async fn test_model_manager_pull_http_errors() {
    let pid = std::process::id();
    let spec = HuggingFaceModelSpec::new("org/repo", "f.onnx", "t.onnx");
    let ok_dir = std::env::temp_dir().join(format!("tycho-httperr-{pid}"));
    let _ = std::fs::remove_dir_all(&ok_dir);

    // Connection refused: request error arm.
    let dead = ModelManager::new(ModelsConfig {
        hf_endpoint: "http://127.0.0.1:1".into(),
        ..Default::default()
    });
    let err = dead
        .pull_model_spec(&spec, &ok_dir.join("a.onnx"))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        rust_voice_assistant::Error::ModelDownload { .. }
    ));
    assert!(err.to_string().contains("request failed"));

    // Non-2xx status arm.
    let not_found = ModelManager::new(ModelsConfig {
        hf_endpoint: common::http_server(b"missing", "404 Not Found"),
        ..Default::default()
    });
    let err = not_found
        .pull_model_spec(&spec, &ok_dir.join("b.onnx"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("404"));

    // Truncated body: mid-stream chunk error arm.
    let truncated = ModelManager::new(ModelsConfig {
        hf_endpoint: common::truncated_server(b"partial", 999_999),
        ..Default::default()
    });
    let err = truncated
        .pull_model_spec(&spec, &ok_dir.join("c.onnx"))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        rust_voice_assistant::Error::ModelDownload { .. }
    ));

    // Body smaller than expected_min_bytes: integrity failure arm.
    let tiny = ModelManager::new(ModelsConfig {
        hf_endpoint: common::hf_server(b"tiny"),
        ..Default::default()
    });
    let err = tiny
        .pull_model_spec(&spec, &ok_dir.join("d.onnx"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("corrupt model"));

    // Partial downloads never leak temp files.
    for f in ["a.onnx", "b.onnx", "c.onnx", "d.onnx"] {
        assert!(!ok_dir.join(f).exists());
        assert!(!ok_dir.join(f).with_extension("tmp_download").exists());
    }
    let _ = std::fs::remove_dir_all(&ok_dir);
}

#[tokio::test]
async fn test_model_manager_partial_bootstrap() {
    let temp_dir = std::env::temp_dir().join(format!("tycho_test_partial_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);

    let config = ModelsConfig {
        cache_dir: temp_dir.clone(),
        auto_download_on_first_run: true,
        hf_endpoint: common::hf_server(weights()),
        ..Default::default()
    };

    let seeded = ModelManager::new(config.clone());
    std::fs::create_dir_all(seeded.stt_model_target_path().parent().unwrap()).unwrap();
    std::fs::write(seeded.stt_model_target_path(), b"existing-weights").unwrap();

    let manager = ModelManager::new(config);
    assert!(manager.is_first_run());
    let inventory = manager.ensure_models().await.unwrap();

    assert_eq!(
        std::fs::read(&inventory.stt_model_path).unwrap(),
        b"existing-weights"
    );
    assert!(inventory.tts_model_path.exists());
    assert!(inventory.tts_voices_path.exists());

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_model_manager_partial_bootstrap_existing_tts() {
    let temp_dir = std::env::temp_dir().join(format!("tycho_test_partial2_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);

    let config = ModelsConfig {
        cache_dir: temp_dir.clone(),
        auto_download_on_first_run: true,
        hf_endpoint: common::hf_server(weights()),
        ..common::test_models_config()
    };

    let probe = ModelManager::new(config.clone());
    std::fs::create_dir_all(probe.tts_model_target_path().parent().unwrap()).unwrap();
    std::fs::write(probe.tts_model_target_path(), b"existing-tts").unwrap();
    std::fs::write(probe.tts_voices_target_path(), b"existing-voices").unwrap();
    std::fs::create_dir_all(probe.router_model_target_path().unwrap().parent().unwrap()).unwrap();
    std::fs::write(
        probe.router_model_target_path().unwrap(),
        b"existing-router",
    )
    .unwrap();

    let inventory = probe.ensure_models().await.unwrap();
    assert_eq!(
        std::fs::read(&inventory.tts_model_path).unwrap(),
        b"existing-tts"
    );
    assert_eq!(
        std::fs::read(&inventory.tts_voices_path).unwrap(),
        b"existing-voices"
    );
    assert_eq!(
        std::fs::read(inventory.router_model_path.unwrap()).unwrap(),
        b"existing-router"
    );
    assert!(inventory.stt_model_path.exists());

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_model_manager_bootstrap_without_router_spec() {
    let temp_dir = std::env::temp_dir().join(format!("tycho_test_norouter_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);

    let config = ModelsConfig {
        cache_dir: temp_dir.clone(),
        router_model: None,
        hf_endpoint: common::hf_server(weights()),
        ..common::test_models_config()
    };

    let inventory = ModelManager::new(config).ensure_models().await.unwrap();
    assert!(inventory.stt_model_path.exists());
    assert!(inventory.router_model_path.is_none());

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_expand_tilde_non_utf8_path() {
    use std::os::unix::ffi::OsStrExt;

    let config = ModelsConfig {
        cache_dir: std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"~/\xff\xfe")),
        ..Default::default()
    };
    let manager = ModelManager::new(config);
    assert_eq!(
        manager.resolved_cache_dir(),
        std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"~/\xff\xfe"))
    );
}

#[test]
fn test_model_spec_config_to_hf_spec() {
    let base = ModelSpecConfig {
        repo_id: "o/r".into(),
        revision: "v1".into(),
        filename: "m.onnx".into(),
        target_filename: "t.onnx".into(),
        expected_min_bytes: 42,
        subfolder: Some("sub/dir".into()),
    };
    let spec = base.to_hf_spec();
    assert_eq!(spec.repo_id, "o/r");
    assert_eq!(spec.revision, "v1");
    assert_eq!(spec.expected_min_bytes, 42);
    assert_eq!(spec.subfolder.as_deref(), Some("sub/dir"));
    assert!(spec.resolve_url("https://hf.co").contains("/sub/dir/"));

    let plain = ModelSpecConfig {
        subfolder: None,
        ..base
    };
    assert!(plain.to_hf_spec().subfolder.is_none());
}
