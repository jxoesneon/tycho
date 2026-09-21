//! CLI binary coverage tests driving the compiled `tycho` executable.

use std::process::Command;

mod common;

fn tycho_cmd(home: &std::path::Path) -> Command {
    // Every subcommand that initializes the pipeline resolves models and may
    // reach the generation endpoint — pin both to local mock servers so the
    // tests are deterministic and offline-safe.
    let config_path = home.join("tycho.toml");
    std::fs::write(
        &config_path,
        format!(
            "[models]\nhf_endpoint = \"{}\"\ncache_dir = \"{}\"\n\n[models.stt_model]\nrepo_id = \"ggerganov/whisper.cpp\"\nrevision = \"main\"\nfilename = \"ggml-tiny.en.bin\"\ntarget_filename = \"ggml-tiny.en.bin\"\nexpected_min_bytes = 1024\n\n[models.router_model]\nrepo_id = \"convaiinnovations/laya\"\nrevision = \"main\"\nfilename = \"model.onnx\"\ntarget_filename = \"laya_intent_classifier.onnx\"\nexpected_min_bytes = 512\n\n[generation]\nendpoint = \"{}\"\nauto_setup = false\n",
            common::hf_server(common::model_weights()),
            home.join("models").display(),
            common::openai_server("Acknowledged"),
        ),
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tycho"));
    cmd.env("HOME", home)
        .env("TYCHO_CONFIG", &config_path)
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("XDG_CURRENT_DESKTOP")
        .env_remove("KDE_FULL_SESSION")
        .env_remove("XDG_RUNTIME_DIR")
        // Spawned binaries must not open the orb overlay during tests.
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        // And must never trigger real self-provisioning downloads.
        .env("TYCHO_NO_INSTALL", "1");
    cmd
}

fn fresh_home(tag: &str) -> common::TempDir {
    common::TempDir::new(format!("cli-test-{}", tag))
}

#[test]
fn test_cli_pull_models() {
    let home = fresh_home("pull");
    let out = tycho_cmd(&home).args(["pull-models"]).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("STT model:"));
    assert!(stdout.contains("TTS model:"));
    assert!(stdout.contains("TTS voices:"));
    assert!(stdout.contains("Router model:"));
}

#[test]
fn test_cli_pull_models_force() {
    let home = fresh_home("force");
    let out = tycho_cmd(&home)
        .args(["pull-models", "--force"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn test_cli_query_fast_path() {
    let home = fresh_home("query");
    let out = tycho_cmd(&home)
        .args(["query", "switch to workspace 2"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Switched to workspace 2"));
}

#[test]
fn test_cli_query_with_config_flag() {
    let home = fresh_home("cfgflag");
    let mut cmd = tycho_cmd(&home);
    let config_path = home.join("tycho.toml");
    let out = cmd
        .env_remove("TYCHO_CONFIG")
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "query",
            "switch to workspace 3",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Switched to workspace 3"));
}

#[test]
fn test_cli_test_desktop() {
    let home = fresh_home("desktop");
    let out = tycho_cmd(&home).args(["test-desktop"]).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Desktop context:"));
}

#[test]
fn test_cli_error_paths() {
    // Missing --config file → from_file error, nonzero exit.
    let home = fresh_home("err-cfg");
    let out = tycho_cmd(&home)
        .args(["--config", "/definitely/missing/tycho.toml", "query", "hi"])
        .output()
        .unwrap();
    assert!(!out.status.success());

    // Malformed TYCHO_CONFIG file → load error, nonzero exit.
    let bad = home.join("bad.toml");
    std::fs::write(&bad, "this is = [not toml\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_tycho"))
        .env("HOME", &*home)
        .env("TYCHO_CONFIG", &bad)
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .args(["query", "hi"])
        .output()
        .unwrap();
    assert!(!out.status.success());

    // Dead model endpoint → ensure_models error propagates through
    // pull-models, query, and test-desktop.
    let config_path = home.join("dead.toml");
    std::fs::write(
        &config_path,
        format!(
            "[models]\nhf_endpoint = \"http://127.0.0.1:1\"\ncache_dir = \"{}\"\n",
            home.join("models").display()
        ),
    )
    .unwrap();
    // `daemon` is absent: it detaches immediately, so its init failures
    // surface in daemon.log rather than the invoking exit code.
    for args in [
        vec!["pull-models"],
        vec!["query", "hi there"],
        vec!["test-desktop"],
        vec!["run"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_tycho"))
            .env("HOME", &*home)
            .env("TYCHO_CONFIG", &config_path)
            .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
            .args(&args)
            .output()
            .unwrap();
        assert!(!out.status.success(), "{:?} should fail", args);
    }

    // Generation endpoint dead + deliberative query → process_query error.
    let config_path = home.join("deadgen.toml");
    std::fs::write(
        &config_path,
        format!(
            "[models]\nhf_endpoint = \"{}\"\ncache_dir = \"{}\"\n\n[models.stt_model]\nrepo_id = \"ggerganov/whisper.cpp\"\nrevision = \"main\"\nfilename = \"ggml-tiny.en.bin\"\ntarget_filename = \"ggml-tiny.en.bin\"\nexpected_min_bytes = 1024\n\n[generation]\nendpoint = \"http://127.0.0.1:1\"\nauto_setup = false\n",
            common::hf_server(common::model_weights()),
            home.join("models").display()
        ),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_tycho"))
        .env("HOME", &*home)
        .env("TYCHO_CONFIG", &config_path)
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .args(["query", "ponder the nature of existence"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn test_cli_run_daemon() {
    use std::io::Read;
    use std::process::Stdio;

    let home = fresh_home("run");

    // `run` stays in the foreground listen loop until SIGINT.
    let mut child = tycho_cmd(&home)
        .args(["run"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf);
        let _ = tx.send(buf);
    });
    std::thread::sleep(std::time::Duration::from_secs(2));
    let _ = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .output();
    let status = child.wait().unwrap();
    let logs = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap_or_default();
    // With a capture device the daemon reaches the listen loop; on
    // hosts without one it exits with an honest audio error.
    let listening = logs.contains("listening on audio inputs");
    let honest_failure = !status.success() && (logs.contains("audio") || logs.contains("device"));
    assert!(
        status.success() && listening || honest_failure,
        "tycho run unexpected outcome; logs: {}",
        logs
    );

    // `daemon` detaches: exits immediately, writes a PID file, and the
    // detached `tycho run` keeps living under a new session.
    let out = tycho_cmd(&home).args(["daemon"]).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("daemon started"), "stdout: {}", stdout);

    let pid_path = home.join(".local/state/tycho/tycho.pid");
    let pid = std::fs::read_to_string(&pid_path)
        .expect("pid file written")
        .trim()
        .to_string();
    std::thread::sleep(std::time::Duration::from_secs(2));
    let _ = Command::new("kill").args(["-INT", &pid]).output();

    // The detached daemon exits on SIGINT within a few seconds.
    let mut gone = false;
    for _ in 0..20 {
        let alive = Command::new("kill")
            .args(["-0", &pid])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !alive {
            gone = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    assert!(gone, "daemon pid {} still alive", pid);
}

#[test]
fn test_cli_daemon_error_edges() {
    // `--config` is forwarded to the detached child; the pid file records it.
    let home = fresh_home("daemon-cfg");
    let config_path = home.join("tycho.toml");
    let out = tycho_cmd(&home)
        .env_remove("TYCHO_CONFIG")
        .args(["--config", config_path.to_str().unwrap(), "daemon"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pid = std::fs::read_to_string(home.join(".local/state/tycho/tycho.pid"))
        .expect("pid file written")
        .trim()
        .to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));
    let _ = Command::new("kill").args(["-INT", &pid]).output();
    for _ in 0..20 {
        let alive = Command::new("kill")
            .args(["-0", &pid])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !alive {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    panic!("daemon pid {} still alive", pid);
}

#[test]
fn test_cli_daemon_io_failures() {
    // daemon.log as a directory -> log open fails honestly, no child spawned.
    let home = fresh_home("daemon-log");
    std::fs::create_dir_all(home.join(".local/state/tycho/daemon.log")).unwrap();
    let out = tycho_cmd(&home).args(["daemon"]).output().unwrap();
    assert!(!out.status.success());

    // tycho.pid as a directory -> the spawned child is killed rather than
    // orphaned, and the command fails.
    let home = fresh_home("daemon-pid");
    std::fs::create_dir_all(home.join(".local/state/tycho/tycho.pid")).unwrap();
    let out = tycho_cmd(&home).args(["daemon"]).output().unwrap();
    assert!(!out.status.success());

    // HOME unset -> honest error before any spawn.
    let out = Command::new(env!("CARGO_BIN_EXE_tycho"))
        .env_remove("HOME")
        .args(["daemon"])
        .output()
        .unwrap();
    assert!(!out.status.success());

    // HOME pointing at a regular file -> state dir creation fails.
    let home = fresh_home("daemon-file");
    let notdir = home.join("not-a-dir");
    std::fs::write(&notdir, b"x").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_tycho"))
        .env("HOME", &notdir)
        .args(["daemon"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}
