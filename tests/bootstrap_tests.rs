//! Local-backend bootstrap coverage: URL/parsing helpers, skip gates,
//! and the deterministic failure path through a shimmed ollama binary.
//!
//! All environment mutation lives in one test so parallel cases cannot
//! race on shared process state.

use rust_voice_assistant::config::GenerationConfig;
use rust_voice_assistant::generation::bootstrap::{
    ensure_local_backend, ensure_tts_engine, find_on_path, install_ollama, model_present,
    ollama_bin, server_base, BackendStatus, DEFAULT_ENDPOINT,
};

mod common;

#[test]
fn server_base_strips_path() {
    assert_eq!(
        server_base("http://127.0.0.1:11434/v1/chat/completions"),
        "http://127.0.0.1:11434"
    );
    assert_eq!(server_base("http://h:9"), "http://h:9");
    assert_eq!(server_base("plain-host"), "plain-host");
    assert_eq!(server_base("https://x.y/a/b"), "https://x.y");
}

#[test]
fn model_present_parses_tags() {
    let tags = r#"{"models":[{"name":"llama3.2:3b"},{"name":"qwen3:4b"}]}"#;
    assert!(model_present(tags, "llama3.2:3b"));
    assert!(model_present(tags, "qwen3:4b"));
    // Bare names match the implicit :latest tag.
    let latest = r#"{"models":[{"name":"mistral:latest"}]}"#;
    assert!(model_present(latest, "mistral"));
    assert!(model_present(latest, "mistral:latest"));
    assert!(!model_present(tags, "absent-model"));
    assert!(!model_present("not json", "x"));
    assert!(!model_present("{}", "x"));
    assert!(!model_present(r#"{"models":[]}"#, "x"));
}

#[test]
fn find_on_path_locates_real_binaries() {
    assert!(find_on_path("sh").is_some());
    assert!(find_on_path("tycho-definitely-not-a-binary-xyz").is_none());
}

#[tokio::test]
async fn bootstrap_skips_user_managed_endpoints() {
    // API key configured — never touch.
    let mut gen = GenerationConfig {
        api_key: Some("key".into()),
        ..GenerationConfig::default()
    };
    assert_eq!(
        ensure_local_backend(&gen, None).await,
        BackendStatus::Skipped("api key configured")
    );

    // Custom endpoint — never touch.
    gen.api_key = None;
    gen.endpoint = "http://127.0.0.1:9/v1/chat/completions".into();
    assert_eq!(
        ensure_local_backend(&gen, None).await,
        BackendStatus::Skipped("custom generation endpoint")
    );

    // Disabled in config.
    gen.endpoint = DEFAULT_ENDPOINT.into();
    gen.auto_setup = false;
    assert_eq!(
        ensure_local_backend(&gen, None).await,
        BackendStatus::Skipped("auto-setup disabled in config")
    );
}

#[tokio::test]
async fn bootstrap_end_to_end_via_shim() {
    let home = common::TempDir::new("bootstrap-home");
    let saved_home = std::env::var_os("HOME");
    let saved_bin = std::env::var_os("TYCHO_OLLAMA_BIN");
    let saved_timeout = std::env::var_os("TYCHO_SERVE_TIMEOUT_MS");
    let saved_rc = std::env::var_os("TYCHO_PULL_RC");
    let saved_noinstall = std::env::var_os("TYCHO_NO_INSTALL");
    let saved_url = std::env::var_os("TYCHO_OLLAMA_URL");
    let saved_piper_url = std::env::var_os("TYCHO_PIPER_URL");
    let saved_piper_bin = std::env::var_os("TYCHO_PIPER_BIN");

    std::env::set_var("HOME", &*home);
    std::env::set_var("TYCHO_SERVE_TIMEOUT_MS", "3000");
    // Never run real installers/downloads in tests.
    std::env::set_var("TYCHO_NO_INSTALL", "1");

    let gen = GenerationConfig {
        model: "tycho-nonexistent-model".to_string(),
        ..GenerationConfig::default()
    };

    // 1. Missing binary + non-interactive stdin (test harness) → honest
    //    unavailable rather than a sudo prompt. When a real server is
    //    already up the missing binary still fails at the pull step.
    std::env::set_var("TYCHO_OLLAMA_BIN", "/nonexistent/ollama");
    assert!(ollama_bin().is_none());
    let status = ensure_local_backend(&gen, None).await;
    assert!(
        matches!(status, BackendStatus::Unavailable(_)),
        "expected Unavailable, got {status:?}"
    );

    // 2. Shim that exits immediately on `serve` → readiness timeout.
    let shim = home.join("ollama-shim");
    std::fs::write(&shim, "#!/bin/sh\nexit 1\n").unwrap();
    make_executable(&shim);
    std::env::set_var("TYCHO_OLLAMA_BIN", &shim);
    assert_eq!(ollama_bin().as_deref(), Some(shim.as_path()));
    if !probe_default_up() {
        let status = ensure_local_backend(&gen, None).await;
        assert!(
            matches!(status, BackendStatus::Unavailable(_)),
            "expected Unavailable, got {status:?}"
        );
    }

    // 3. Serving shim: `serve` binds 127.0.0.1:11434 for 15s via a
    //    self-terminating JSON responder; `pull` exits $TYCHO_PULL_RC.
    //    Covers wait_ready success, the model loop, pull, and Ready.
    let responder = home.join("responder.py");
    std::fs::write(
        &responder,
        "from http.server import BaseHTTPRequestHandler, HTTPServer\n\
         class H(BaseHTTPRequestHandler):\n\
         \x20   def do_GET(self):\n\
         \x20       b = b'{\"models\":[]}'\n\
         \x20       self.send_response(200)\n\
         \x20       self.send_header(\"Content-Length\", str(len(b)))\n\
         \x20       self.end_headers()\n\
         \x20       self.wfile.write(b)\n\
         \x20   def log_message(self, *a):\n\
         \x20       pass\n\
         HTTPServer((\"127.0.0.1\", 11434), H).serve_forever()\n",
    )
    .unwrap();
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n  serve) exec timeout 15 python3 {} >/dev/null 2>&1 ;;\n  pull) exit \"${{TYCHO_PULL_RC:-0}}\" ;;\n  *) exit 0 ;;\nesac\n",
            responder.display()
        ),
    )
    .unwrap();
    make_executable(&shim);
    let status = ensure_local_backend(&gen, Some("tycho-nonexistent-extra")).await;
    assert_eq!(
        status,
        BackendStatus::Ready,
        "serving shim must reach Ready"
    );

    // 4. Server still up, but pull fails → Unavailable from pull_model.
    std::env::set_var("TYCHO_PULL_RC", "1");
    let status = ensure_local_backend(&gen, None).await;
    assert!(
        matches!(status, BackendStatus::Unavailable(_)),
        "expected Unavailable, got {status:?}"
    );

    // 5. Real install paths: fixture tarballs holding the shims —
    //    bin/ollama (.tar.zst) and piper/piper (.tar.gz) — fetched from
    //    a local static server through the actual download pipelines
    //    into $HOME/.local/share/tycho.
    let mut file_server = None;
    let have_tools = ["curl", "zstd", "tar", "python3"]
        .iter()
        .all(|t| find_on_path(t).is_some());
    if have_tools {
        let fixture = home.join("fixture");
        let fixture_bin = fixture.join("bin");
        std::fs::create_dir_all(&fixture_bin).unwrap();
        std::fs::write(fixture_bin.join("ollama"), std::fs::read(&shim).unwrap()).unwrap();
        make_executable(&fixture_bin.join("ollama"));
        let piper_fixture = home.join("piper_fixture");
        let piper_dir = piper_fixture.join("piper");
        std::fs::create_dir_all(&piper_dir).unwrap();
        std::fs::write(
            piper_dir.join("piper"),
            "#!/bin/sh\ncat >/dev/null\nexit 0\n",
        )
        .unwrap();
        make_executable(&piper_dir.join("piper"));
        let serve_dir = home.join("served");
        std::fs::create_dir_all(&serve_dir).unwrap();
        let packed = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "tar -cf - -C '{}' . | zstd -q > '{}' && tar -czf '{}' -C '{}' .",
                fixture.display(),
                serve_dir.join("ollama.tar.zst").display(),
                serve_dir.join("piper.tar.gz").display(),
                piper_fixture.display()
            ))
            .status()
            .unwrap();
        assert!(packed.success());

        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        file_server = Some(
            std::process::Command::new("python3")
                .args([
                    "-m",
                    "http.server",
                    &port.to_string(),
                    "--bind",
                    "127.0.0.1",
                ])
                .current_dir(&serve_dir)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("fixture file server must spawn"),
        );
        // Wait until the fixture server actually accepts connections.
        let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(100))
                .is_ok()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        std::env::remove_var("TYCHO_NO_INSTALL");
        std::env::remove_var("TYCHO_OLLAMA_BIN");
        std::env::set_var(
            "TYCHO_OLLAMA_URL",
            format!("http://127.0.0.1:{port}/ollama.tar.zst"),
        );

        install_ollama().expect("fixture install must succeed");
        let installed = home.join(".local/share/tycho/bin/ollama");
        assert!(installed.is_file(), "fixture binary extracted");
        // ollama_bin() must find the user-local install via the HOME
        // fallback — unless a real ollama already sits on PATH.
        if find_on_path("ollama").is_none() {
            assert_eq!(ollama_bin().as_deref(), Some(installed.as_path()));
        }

        // The extracted shim serves/pulls: full loop to Ready.
        std::env::set_var("TYCHO_PULL_RC", "0");
        let status = ensure_local_backend(&gen, None).await;
        assert_eq!(status, BackendStatus::Ready, "installed shim must serve");

        // 6. TTS engine install: no engine on PATH → ensure_tts_engine
        //    downloads the piper fixture and finds piper/piper.
        if find_on_path("espeak-ng").is_none() && find_on_path("flite").is_none() {
            std::env::remove_var("TYCHO_PIPER_BIN");
            std::env::set_var(
                "TYCHO_PIPER_URL",
                format!("http://127.0.0.1:{port}/piper.tar.gz"),
            );
            assert_eq!(
                ensure_tts_engine("auto", true),
                BackendStatus::Ready,
                "piper fixture install must reach Ready"
            );
            assert!(home.join(".local/share/tycho/piper/piper").is_file());
            // Explicit engines are verified, and missing ones either
            // provision or report an actionable error — never skipped.
            assert_eq!(
                ensure_tts_engine("espeak", true),
                if find_on_path("espeak").is_some() {
                    BackendStatus::Ready
                } else {
                    BackendStatus::Unavailable(
                        "espeak not installed — install it with your package manager or pick another engine"
                            .to_string(),
                    )
                }
            );
            std::env::set_var("TYCHO_NO_INSTALL", "1");
            let kokoro = ensure_tts_engine("kokoro", true);
            std::env::remove_var("TYCHO_NO_INSTALL");
            assert_eq!(
                kokoro,
                if rust_voice_assistant::tts::catalog::engine_ready("kokoro") {
                    BackendStatus::Ready
                } else {
                    BackendStatus::Unavailable(
                        "auto-install disabled via TYCHO_NO_INSTALL".to_string(),
                    )
                }
            );
            assert_eq!(
                ensure_tts_engine("auto", false),
                BackendStatus::Skipped("auto-setup disabled in config")
            );
        }
    }
    if let Some(mut child) = file_server {
        let _ = child.kill();
    }

    // Restore.
    for (key, saved) in [
        ("HOME", saved_home),
        ("TYCHO_OLLAMA_BIN", saved_bin),
        ("TYCHO_SERVE_TIMEOUT_MS", saved_timeout),
        ("TYCHO_PULL_RC", saved_rc),
        ("TYCHO_NO_INSTALL", saved_noinstall),
        ("TYCHO_OLLAMA_URL", saved_url),
        ("TYCHO_PIPER_URL", saved_piper_url),
        ("TYCHO_PIPER_BIN", saved_piper_bin),
    ] {
        match saved {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}

fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

/// Whether something already answers on the default port — decides
/// which branch the missing-binary test exercises.
fn probe_default_up() -> bool {
    std::net::TcpStream::connect_timeout(
        &"127.0.0.1:11434".parse().unwrap(),
        std::time::Duration::from_millis(200),
    )
    .is_ok()
}
