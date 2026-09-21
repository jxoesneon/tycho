//! Environment-dependent backend detection tests.
//!
//! All env mutation lives in a single test so nothing races on shared
//! process env. The Hyprland backend is exercised over real Unix-socket IPC
//! (a fake compositor serves canned replies) and the KDE backend over a
//! private `dbus-daemon` session bus.

use rust_voice_assistant::desktop::{DesktopBackend, DesktopManager, HyprlandBackend};

mod common;

#[tokio::test]
async fn test_hyprland_backend_and_auto_detection() {
    std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
    std::env::remove_var("XDG_CURRENT_DESKTOP");
    std::env::remove_var("KDE_FULL_SESSION");
    let runtime = std::env::temp_dir().join(format!("tycho-rt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&runtime);
    std::fs::create_dir_all(&runtime).unwrap();
    std::env::set_var("XDG_RUNTIME_DIR", &runtime);

    let missing = HyprlandBackend::new().await;
    assert!(missing.is_err());

    // Signature set but socket file absent → honest connection failure.
    std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", "test-sig-123");
    let missing = HyprlandBackend::new().await;
    assert!(missing.is_err());
    let (cmd_sock, evt_sock) = common::fake_hyprland_paths(&runtime, "test-sig-123");
    let _cmd = common::hypr_cmd_server(&cmd_sock);
    let _evt = common::hypr_event_server(
        &evt_sock,
        vec![
            "workspace>>7".to_string(),
            "closewindow>>0xabc".to_string(),
            "focusedmon>>DP-1".to_string(),
            "openwindow>>0x999,2,kitty,New Window".to_string(),
            "unmapped>>x".to_string(),
            "malformed-line".to_string(),
        ],
    );

    let backend = HyprlandBackend::new().await.unwrap();
    assert_eq!(backend.name(), "Hyprland (Wayland socket IPC)");
    assert!(backend.capabilities().supports_nested_testing);
    assert!(backend
        .command_socket_path()
        .ends_with("hypr/test-sig-123/.socket.sock"));
    assert!(backend
        .event_socket_path()
        .ends_with("hypr/test-sig-123/.socket2.sock"));

    // XDG_RUNTIME_DIR fallback to /tmp still derives the socket layout.
    let tmp_dir = std::path::Path::new("/tmp/hypr/test-sig-123");
    std::fs::create_dir_all(tmp_dir).unwrap();
    std::fs::write(tmp_dir.join(".socket.sock"), b"").unwrap();
    std::env::remove_var("XDG_RUNTIME_DIR");
    let fallback = HyprlandBackend::new().await.unwrap();
    assert!(fallback
        .command_socket_path()
        .starts_with("/tmp/hypr/test-sig-123"));
    let _ = std::fs::remove_dir_all(tmp_dir);
    std::env::set_var("XDG_RUNTIME_DIR", &runtime);

    // Real socket round trips against the fake compositor.
    let active = backend.get_active_window().await.unwrap().unwrap();
    assert_eq!(active.id, "0xdeadbeef");
    assert_eq!(active.app_id, "kitty");
    assert_eq!(active.workspace_id, 2);
    assert_eq!(active.pid, Some(4242));
    let windows = backend.list_windows().await.unwrap();
    assert_eq!(windows.len(), 2);
    assert!(windows[1].is_floating);
    let workspaces = backend.list_workspaces().await.unwrap();
    assert_eq!(workspaces.len(), 2);
    assert!(workspaces[1].is_active);

    // Event stream from .socket2.sock is mapped into DesktopEvents.
    let mut rx = backend.subscribe_events().unwrap();
    use rust_voice_assistant::desktop::DesktopEvent;
    assert_eq!(rx.recv().await.unwrap(), DesktopEvent::WorkspaceChanged(7));
    assert_eq!(
        rx.recv().await.unwrap(),
        DesktopEvent::WindowClosed("0xabc".to_string())
    );
    assert_eq!(
        rx.recv().await.unwrap(),
        DesktopEvent::MonitorChanged("DP-1".to_string())
    );
    match rx.recv().await.unwrap() {
        DesktopEvent::WindowOpened(w) => {
            assert_eq!(w.id, "0x999");
            assert_eq!(w.app_id, "kitty");
        }
        other => panic!("unexpected event: {:?}", other),
    }

    backend.switch_workspace(2).await.unwrap();
    assert_eq!(rx.recv().await.unwrap(), DesktopEvent::WorkspaceChanged(2));

    backend.focus_window("0xdeadbeef").await.unwrap();
    assert!(backend.focus_window("nonexistent").await.is_err());
    backend.close_window(None).await.unwrap();
    assert!(backend.close_window(Some("nonexistent")).await.is_err());
    backend.close_window(Some("0xdeadbeef")).await.unwrap();
    backend.toggle_fullscreen(None).await.unwrap();
    backend.toggle_floating(Some("0xdeadbeef")).await.unwrap();
    backend.move_window_to_workspace(None, 2).await.unwrap();
    backend
        .move_window_to_workspace(Some("0xdeadbeef"), 3)
        .await
        .unwrap();

    let auto = DesktopManager::init_auto().await;
    assert_eq!(auto.backend().name(), "Hyprland (Wayland socket IPC)");

    // KDE detection on a private bus with a fake org.kde.KWin service.
    let bus = common::fake_bus().await;
    std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
    if let Some((addr, _daemon)) = bus {
        let state = common::fake_kwin_state();
        let _service = common::fake_kwin_bus(&addr, state).await;
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &addr);

        std::env::set_var("XDG_CURRENT_DESKTOP", "KDE");
        let auto = DesktopManager::init_auto().await;
        assert_eq!(auto.backend().name(), "KDE Plasma (KWin / D-Bus)");

        std::env::remove_var("XDG_CURRENT_DESKTOP");
        std::env::set_var("KDE_FULL_SESSION", "1");
        let auto = DesktopManager::init_auto().await;
        assert_eq!(auto.backend().name(), "KDE Plasma (KWin / D-Bus)");

        std::env::remove_var("KDE_FULL_SESSION");
        std::env::set_var("XDG_CURRENT_DESKTOP", "sway");
        let auto = DesktopManager::init_auto().await;
        assert_eq!(auto.backend().name(), "Mock Compositor");

        std::env::set_var("XDG_CURRENT_DESKTOP", "Hyprland");
        let auto = DesktopManager::init_auto().await;
        assert_eq!(auto.backend().name(), "Mock Compositor");

        // KDE requested but the session bus is unreachable → Mock fallback.
        std::env::set_var(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/definitely-missing-bus-xyz",
        );
        std::env::set_var("XDG_CURRENT_DESKTOP", "KDE");
        let auto = DesktopManager::init_auto().await;
        assert_eq!(auto.backend().name(), "Mock Compositor");
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &addr);
        std::env::set_var("XDG_CURRENT_DESKTOP", "Hyprland");

        // KDE backend filesystem failures surface as honest errors: an
        // unwritable temp dir prevents bridge-script staging. The backend
        // is created under the bad TMPDIR so its bridge cache stays empty.
        let saved_tmp = std::env::var("TMPDIR").ok();
        std::env::set_var("TMPDIR", "/proc");
        let conn = common::bus_client(&addr).await;
        let kde = rust_voice_assistant::desktop::KdeBackend::with_connection(conn)
            .await
            .unwrap();
        assert!(kde.focus_window("kde-win-001").await.is_err());
        let ro_dir = runtime.join("ro-tmp");
        std::fs::create_dir_all(ro_dir.join("tycho-kwin")).unwrap();
        let mut perms = std::fs::metadata(ro_dir.join("tycho-kwin"))
            .unwrap()
            .permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o555);
        std::fs::set_permissions(ro_dir.join("tycho-kwin"), perms).unwrap();
        std::env::set_var("TMPDIR", &ro_dir);
        assert!(kde.focus_window("kde-win-001").await.is_err());
        match saved_tmp {
            Some(v) => std::env::set_var("TMPDIR", v),
            None => std::env::remove_var("TMPDIR"),
        }

        std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", "test-sig-123");
        let auto = DesktopManager::init_auto().await;
        assert_eq!(auto.backend().name(), "Hyprland (Wayland socket IPC)");
        std::env::remove_var("DBUS_SESSION_BUS_ADDRESS");
    }

    let _ = std::fs::remove_dir_all(&runtime);
}

#[tokio::test]
async fn test_expand_tilde_without_home() {
    use rust_voice_assistant::models::ModelManager;

    let saved_home = std::env::var("HOME").ok();
    std::env::remove_var("HOME");

    let config = rust_voice_assistant::config::ModelsConfig {
        cache_dir: std::path::PathBuf::from("~/tycho-nohome"),
        ..Default::default()
    };
    let manager = ModelManager::new(config);
    assert_eq!(
        manager.resolved_cache_dir(),
        std::path::PathBuf::from("~/tycho-nohome")
    );

    if let Some(home) = saved_home {
        std::env::set_var("HOME", home);
    }
}
