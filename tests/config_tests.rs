//! Configuration loader tests: file lookup, TOML parsing, and env overrides.

use rust_voice_assistant::{Error, TychoConfig};

#[test]
fn test_config_load_env_and_file_paths() {
    let pid = std::process::id();
    let home = std::env::temp_dir().join(format!("tycho-cfg-home-{pid}"));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();

    // Fallback arm: no TYCHO_CONFIG and no config file under HOME.
    std::env::remove_var("TYCHO_CONFIG");
    std::env::set_var("HOME", &home);
    let cfg = TychoConfig::load().unwrap();
    assert_eq!(cfg.audio.sample_rate, 16000);

    // Environment overrides take precedence over defaults.
    std::env::set_var("TYCHO_CACHE_DIR", "/tmp/tycho-cfg-cache");
    std::env::set_var("HF_TOKEN", "hf-secret");
    std::env::set_var("TYCHO_JEV_API_KEY", "jev-key");
    std::env::set_var("TYCHO_API_KEY", "gen-key");
    std::env::set_var("TYCHO_ENDPOINT", "http://127.0.0.1:9/v1");
    let cfg = TychoConfig::load().unwrap();
    assert_eq!(
        cfg.models.cache_dir,
        std::path::PathBuf::from("/tmp/tycho-cfg-cache")
    );
    assert_eq!(cfg.models.hf_token.as_deref(), Some("hf-secret"));
    assert_eq!(cfg.router.jev_api_key.as_deref(), Some("jev-key"));
    assert_eq!(cfg.generation.api_key.as_deref(), Some("gen-key"));
    assert_eq!(cfg.generation.endpoint, "http://127.0.0.1:9/v1");

    // TYCHO_CONFIG selects a file; partial TOML merges over defaults.
    let cfg_path = home.join("custom.toml");
    std::fs::write(&cfg_path, "[audio]\nsample_rate = 8000\n").unwrap();
    std::env::set_var("TYCHO_CONFIG", &cfg_path);
    let cfg = TychoConfig::load().unwrap();
    assert_eq!(cfg.audio.sample_rate, 8000);
    assert_eq!(cfg.generation.endpoint, "http://127.0.0.1:9/v1");

    // from_file: malformed TOML and missing file error arms.
    let bad = home.join("bad.toml");
    std::fs::write(&bad, "not = [valid").unwrap();
    assert!(matches!(TychoConfig::from_file(&bad), Err(Error::Toml(_))));
    assert!(matches!(
        TychoConfig::from_file(&home.join("missing.toml")),
        Err(Error::Io(_))
    ));

    for k in [
        "TYCHO_CONFIG",
        "TYCHO_CACHE_DIR",
        "HF_TOKEN",
        "TYCHO_JEV_API_KEY",
        "TYCHO_API_KEY",
        "TYCHO_ENDPOINT",
    ] {
        std::env::remove_var(k);
    }
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn test_save_ui_patch_persists_and_preserves() {
    // Uses explicit paths (save_ui_patch_to) so no env mutation races
    // with the other tests in this binary.
    let pid = std::process::id();
    let home = std::env::temp_dir().join(format!("tycho-cfg-ui-{pid}"));
    let _ = std::fs::remove_dir_all(&home);
    let dir = home.join("nested");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg_path = dir.join("tycho.toml");
    std::fs::write(
        &cfg_path,
        "[audio]\nsample_rate = 8000\n\n[ui]\norb = true\n",
    )
    .unwrap();

    // Partial patch preserves untouched sections and keys.
    rust_voice_assistant::config::save_ui_patch_to(&cfg_path, Some("manual"), None, None, None)
        .unwrap();
    let cfg = TychoConfig::from_file(&cfg_path).unwrap();
    assert_eq!(cfg.ui.mode, "manual");
    assert_eq!(cfg.audio.sample_rate, 8000);
    assert!(cfg.ui.orb);

    rust_voice_assistant::config::save_ui_patch_to(
        &cfg_path,
        None,
        Some("custom"),
        Some(120),
        Some(640),
    )
    .unwrap();
    let cfg = TychoConfig::from_file(&cfg_path).unwrap();
    assert_eq!(cfg.ui.mode, "manual");
    assert_eq!(cfg.ui.position, "custom");
    assert_eq!(cfg.ui.margin_x, 120);
    assert_eq!(cfg.ui.margin_y, 640);

    // Missing file: creates parent dirs and writes a fresh [ui] table.
    let fresh = home.join("fresh/tycho.toml");
    rust_voice_assistant::config::save_ui_patch_to(&fresh, Some("off"), None, None, None).unwrap();
    let cfg = TychoConfig::from_file(&fresh).unwrap();
    assert_eq!(cfg.ui.mode, "off");
    assert_eq!(cfg.ui.position, "auto");

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn test_save_config_patch_dotted_keys() {
    let pid = std::process::id();
    let home = std::env::temp_dir().join(format!("tycho-cfg-patch-{pid}"));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let cfg_path = home.join("tycho.toml");
    std::fs::write(
        &cfg_path,
        "[audio]\nsample_rate = 8000\n\n[vad]\nenergy_threshold = 0.02\n",
    )
    .unwrap();

    // Float write preserves type and sibling keys.
    rust_voice_assistant::config::save_config_patch_to(
        &cfg_path,
        "vad.energy_threshold",
        toml::Value::Float(0.05),
    )
    .unwrap();
    let cfg = TychoConfig::from_file(&cfg_path).unwrap();
    assert_eq!(cfg.vad.energy_threshold, 0.05);
    assert_eq!(cfg.audio.sample_rate, 8000);

    // Missing intermediate table is created.
    rust_voice_assistant::config::save_config_patch_to(
        &cfg_path,
        "router.fast_path_confidence_threshold",
        toml::Value::Float(0.9),
    )
    .unwrap();
    // Scalar at a table position is replaced by a table.
    rust_voice_assistant::config::save_config_patch_to(
        &cfg_path,
        "ui.mode",
        toml::Value::String("off".into()),
    )
    .unwrap();
    rust_voice_assistant::config::save_config_patch_to(
        &cfg_path,
        "ui.orb",
        toml::Value::Boolean(false),
    )
    .unwrap();
    let cfg = TychoConfig::from_file(&cfg_path).unwrap();
    assert_eq!(cfg.router.fast_path_confidence_threshold, 0.9);
    assert_eq!(cfg.ui.mode, "off");
    assert!(!cfg.ui.orb);

    // A scalar occupying a parent position is overwritten by a table.
    let weird = home.join("weird.toml");
    std::fs::write(&weird, "ui = 3\n").unwrap();
    rust_voice_assistant::config::save_config_patch_to(
        &weird,
        "ui.mode",
        toml::Value::String("auto".into()),
    )
    .unwrap();
    let cfg = TychoConfig::from_file(&weird).unwrap();
    assert_eq!(cfg.ui.mode, "auto");

    // Empty key segments are rejected honestly.
    assert!(rust_voice_assistant::config::save_config_patch_to(
        &cfg_path,
        "vad..x",
        toml::Value::Integer(1)
    )
    .is_err());

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn test_config_file_deserialization() {
    let pid = std::process::id();
    let path = std::env::temp_dir().join(format!("tycho-cfg-full-{pid}.toml"));
    std::fs::write(
        &path,
        "[router]\nfast_path_confidence_threshold = 0.5\n\n[generation]\nmodel = \"m1\"\n",
    )
    .unwrap();
    let cfg = TychoConfig::from_file(&path).unwrap();
    assert_eq!(cfg.router.fast_path_confidence_threshold, 0.5);
    assert_eq!(cfg.generation.model, "m1");
    assert_eq!(cfg.vad.energy_threshold, 0.02);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_wake_config_defaults_and_roundtrip() {
    // Disabled by default: plain VAD activation out of the box.
    let cfg = TychoConfig::default();
    assert_eq!(cfg.wake.model, "off");
    assert_eq!(cfg.wake.threshold, 0.5);
    // Silero speech gate on by default; 6s follow-up window; earcons on.
    assert_eq!(cfg.wake.vad_gate, 0.5);
    assert_eq!(cfg.wake.follow_up_seconds, 6);
    assert!(cfg.ui.earcons);

    // [wake] table deserializes and merges over defaults.
    let pid = std::process::id();
    let path = std::env::temp_dir().join(format!("tycho-cfg-wake-{pid}.toml"));
    std::fs::write(
        &path,
        "[wake]\nmodel = \"hey_jarvis\"\nthreshold = 0.7\nvad_gate = 0.0\nfollow_up_seconds = 0\n[ui]\nearcons = false\n",
    )
    .unwrap();
    let cfg = TychoConfig::from_file(&path).unwrap();
    assert_eq!(cfg.wake.model, "hey_jarvis");
    assert_eq!(cfg.wake.threshold, 0.7);
    assert_eq!(cfg.wake.vad_gate, 0.0);
    assert_eq!(cfg.wake.follow_up_seconds, 0);
    assert!(!cfg.ui.earcons);

    // Settings-panel writes persist via the dotted-key patch writer.
    rust_voice_assistant::config::save_config_patch_to(
        &path,
        "wake.model",
        toml::Value::String("alexa".into()),
    )
    .unwrap();
    rust_voice_assistant::config::save_config_patch_to(
        &path,
        "wake.threshold",
        toml::Value::Float(0.3),
    )
    .unwrap();
    let cfg = TychoConfig::from_file(&path).unwrap();
    assert_eq!(cfg.wake.model, "alexa");
    assert_eq!(cfg.wake.threshold, 0.3);
    let _ = std::fs::remove_file(&path);
}

/// Every key the settings window offers must round-trip: patch via
/// `settings_toml_value` + `save_config_patch_to`, then `from_file`
/// must parse the file. Guards the two key tables (typed mapping vs
/// settings snapshot) from drifting apart.
#[test]
fn test_settings_keys_round_trip() {
    use rust_voice_assistant::config::{save_config_patch_to, settings_toml_value};

    let cases: &[(&str, &str)] = &[
        ("ui.orb", "true"),
        ("ui.earcons", "false"),
        ("ui.mode", "manual"),
        ("ui.position", "bottom-left"),
        ("ui.chat_verbosity", "verbose"),
        ("desktop.backend", "hyprland"),
        ("desktop.auto_detect", "false"),
        ("models.auto_download_on_first_run", "false"),
        ("generation.auto_setup", "false"),
        ("generation.temperature", "0.4"),
        ("generation.max_tokens", "256"),
        ("generation.model", "llama3.2:3b"),
        ("generation.persona", "sentinel"),
        ("memory.enable_long_term", "true"),
        ("memory.max_history_turns", "10"),
        ("vad.energy_threshold", "0.02"),
        ("vad.min_silence_duration_ms", "600"),
        ("vad.min_speech_duration_ms", "120"),
        ("router.fast_path_confidence_threshold", "0.9"),
        ("tts.speed", "1.2"),
        ("tts.voice", "en_US-amy-medium"),
        ("tts.engine", "piper"),
        ("stt.language", "en"),
        ("wake.model", "hey_jarvis"),
        ("wake.threshold", "0.6"),
        ("wake.vad_gate", "0.5"),
        ("wake.follow_up_seconds", "4"),
    ];

    let pid = std::process::id();
    let path = std::env::temp_dir().join(format!("tycho-settings-rt-{pid}.toml"));
    let _ = std::fs::remove_file(&path);

    for (key, value) in cases {
        let v = settings_toml_value(key, value)
            .unwrap_or_else(|| panic!("{key} failed to map '{value}' to a TOML value"));
        save_config_patch_to(&path, key, v).unwrap();
        TychoConfig::from_file(&path)
            .unwrap_or_else(|e| panic!("config broke after writing {key}={value}: {e}"));
    }

    let cfg = TychoConfig::from_file(&path).unwrap();
    assert!(!cfg.ui.earcons);
    assert_eq!(cfg.ui.chat_verbosity, "verbose");
    assert_eq!(cfg.wake.follow_up_seconds, 4);
    assert!((cfg.wake.vad_gate - 0.5).abs() < 1e-6);
    assert_eq!(cfg.generation.max_tokens, 256);

    // Unparsable typed values are rejected instead of written.
    assert!(settings_toml_value("wake.vad_gate", "loud").is_none());

    let _ = std::fs::remove_file(&path);
}

/// The config file can carry API keys — writes must be owner-only.
#[cfg(unix)]
#[test]
fn test_config_file_permissions_owner_only() {
    use rust_voice_assistant::config::save_config_patch_to;
    use std::os::unix::fs::PermissionsExt;

    let pid = std::process::id();
    let path = std::env::temp_dir().join(format!("tycho-perms-{pid}.toml"));
    let _ = std::fs::remove_file(&path);

    save_config_patch_to(&path, "ui.orb", toml::Value::Boolean(true)).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "config file mode was {mode:o}");

    let _ = std::fs::remove_file(&path);
}
