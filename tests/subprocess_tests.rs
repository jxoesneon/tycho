//! Real subprocess execution tests for volume control and app launching.
//!
//! All PATH/env mutation lives in a single test to avoid races on shared
//! process env. Stub binaries under a temp dir are put on PATH and record
//! their argv to a log file, proving real exec without touching the host.

use rust_voice_assistant::desktop::DesktopManager;
use rust_voice_assistant::execution::{DesktopExecutor, SystemExecutor};
use std::io::Write;

fn write_stub(dir: &std::path::Path, name: &str, body: &str) {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    drop(f);
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&path, perms).unwrap();
}

#[tokio::test]
async fn test_real_volume_and_launch_subprocesses() {
    let dir = std::env::temp_dir().join(format!("tycho-shims-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("calls.log");
    std::env::set_var("TYCHO_STUB_LOG", &log);
    std::env::remove_var("TYCHO_WPCTL_RC");
    std::env::remove_var("TYCHO_PACTL_RC");

    let stub = r#"#!/bin/sh
echo "$(basename "$0") $*" >> "$TYCHO_STUB_LOG"
rc_var="TYCHO_$(basename "$0" | tr '[:lower:]' '[:upper:]')_RC"
eval "exit \${$rc_var:-0}"
"#;
    for bin in ["wpctl", "pactl", "alacritty", "kitty", "firefox"] {
        write_stub(&dir, bin, stub);
    }

    let old_path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{}", dir.display(), old_path));
    let executor = DesktopExecutor::new(DesktopManager::init_mock());

    // wpctl happy path.
    let res = executor.execute("volume_up", None).await.unwrap();
    assert_eq!(res.output_message, "Volume up 5 percent");
    executor.execute("volume_down", None).await.unwrap();
    executor.execute("mute_audio", None).await.unwrap();
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(calls.contains("wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+"));
    assert!(calls.contains("wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%-"));
    assert!(calls.contains("wpctl get-volume @DEFAULT_AUDIO_SINK@"));
    assert!(calls.contains("wpctl set-mute @DEFAULT_AUDIO_SINK@ 1"));

    // wpctl failure → pactl fallback.
    std::env::set_var("TYCHO_WPCTL_RC", "3");
    executor.execute("volume_up", None).await.unwrap();
    executor.execute("mute_audio", None).await.unwrap();
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(calls.contains("pactl set-sink-volume @DEFAULT_SINK@ 5%+"));
    assert!(calls.contains("pactl set-sink-mute @DEFAULT_SINK@ 1"));

    // Both tools failing → honest error on every volume intent.
    std::env::set_var("TYCHO_PACTL_RC", "2");
    assert!(executor.execute("volume_up", None).await.is_err());
    assert!(executor.execute("volume_down", None).await.is_err());
    assert!(executor.execute("mute_audio", None).await.is_err());
    std::env::remove_var("TYCHO_PACTL_RC");
    std::env::remove_var("TYCHO_WPCTL_RC");

    // Application launching spawns real binaries (detached — poll the log).
    executor.execute("launch_terminal", None).await.unwrap();
    executor
        .execute("launch_terminal", Some("kitty"))
        .await
        .unwrap();
    executor.execute("launch_browser", None).await.unwrap();
    let mut calls = String::new();
    for _ in 0..50 {
        calls = std::fs::read_to_string(&log).unwrap_or_default();
        if calls.contains("alacritty") && calls.contains("kitty") && calls.contains("firefox") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(calls.contains("alacritty"));
    assert!(calls.contains("kitty"));
    assert!(calls.contains("firefox"));

    // Missing binary → honest error.
    let err = executor
        .execute("launch_terminal", Some("definitely-missing-app-xyz"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("failed to launch"));
    let err = executor
        .execute("launch_browser", Some("definitely-missing-browser-xyz"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("failed to launch"));

    // No volume tooling at all → honest error.
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    std::env::set_var("PATH", &empty);
    assert!(SystemExecutor::volume_up(5).await.is_err());
    assert!(SystemExecutor::toggle_mute().await.is_err());
    assert!(SystemExecutor::launch("anything").await.is_err());

    std::env::set_var("PATH", old_path);
    std::env::remove_var("TYCHO_STUB_LOG");
    let _ = std::fs::remove_dir_all(&dir);
}
