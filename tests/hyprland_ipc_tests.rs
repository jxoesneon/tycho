//! Hyprland Unix-socket IPC edge and error-path tests.
//!
//! These use `HyprlandBackend::with_sockets` pointed at fake listeners, so
//! they need no environment mutation and can run in parallel.

use rust_voice_assistant::desktop::{DesktopBackend, DesktopError, DesktopEvent, HyprlandBackend};
use std::sync::{Arc, Mutex};

mod common;

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tycho-hypr-{}-{}-{:?}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            % 1_000_000
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn backend_at(dir: &std::path::Path) -> (HyprlandBackend, std::path::PathBuf, std::path::PathBuf) {
    let cmd = dir.join(".socket.sock");
    let evt = dir.join(".socket2.sock");
    (
        HyprlandBackend::with_sockets(cmd.clone(), evt.clone()),
        cmd,
        evt,
    )
}

#[tokio::test]
async fn test_hyprland_connection_failure() {
    let dir = tmpdir("connfail");
    let (backend, _cmd, _evt) = backend_at(&dir);
    let err = backend.get_active_window().await.unwrap_err();
    assert!(matches!(err, DesktopError::ConnectionFailed(_)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_hyprland_malformed_and_shape_errors() {
    let dir = tmpdir("malformed");
    let (backend, cmd, _evt) = backend_at(&dir);
    let _server = common::unix_rpc_server(
        &cmd,
        Arc::new(|payload: &str| {
            if payload == "j/clients" {
                "{}".to_string()
            } else if payload == "j/workspaces" {
                "{\"not\":\"array\"}".to_string()
            } else {
                "not json at all".to_string()
            }
        }),
    );

    let err = backend.get_active_window().await.unwrap_err();
    assert!(matches!(err, DesktopError::IpcError(_)));

    let err = backend.list_windows().await.unwrap_err();
    assert!(err.to_string().contains("not an array"));

    let err = backend.list_workspaces().await.unwrap_err();
    assert!(err.to_string().contains("not an array"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_hyprland_dispatch_rejection() {
    let dir = tmpdir("dispatch");
    let (backend, cmd, _evt) = backend_at(&dir);
    let _server = common::hypr_cmd_server(&cmd);

    let err = backend.focus_window("nonexistent").await.unwrap_err();
    assert!(matches!(err, DesktopError::IpcError(_)));
    let err = backend.switch_workspace(0).await;
    // `workspace 0` dispatches fine at IPC level in the fake — assert ok.
    assert!(err.is_ok());
    let err = backend
        .move_window_to_workspace(Some("boom-window"), 2)
        .await
        .unwrap_err();
    assert!(matches!(err, DesktopError::IpcError(_)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_hyprland_dispatch_payloads_sent() {
    let dir = tmpdir("payloads");
    let (backend, cmd, _evt) = backend_at(&dir);
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let _server = common::unix_rpc_server(
        &cmd,
        Arc::new(move |payload: &str| {
            seen2.lock().unwrap().push(payload.to_string());
            "ok".to_string()
        }),
    );

    backend.switch_workspace(4).await.unwrap();
    backend.focus_window("0xabc").await.unwrap();
    backend.close_window(None).await.unwrap();
    backend.close_window(Some("0xabc")).await.unwrap();
    backend.toggle_fullscreen(Some("0xabc")).await.unwrap();
    backend.toggle_floating(None).await.unwrap();
    backend.move_window_to_workspace(None, 5).await.unwrap();
    backend
        .move_window_to_workspace(Some("0xabc"), 6)
        .await
        .unwrap();

    let sent = seen.lock().unwrap().clone();
    assert_eq!(
        sent,
        vec![
            "dispatch workspace 4",
            "dispatch focuswindow address:0xabc",
            "dispatch killactive",
            "dispatch closewindow address:0xabc",
            "dispatch focuswindow address:0xabc",
            "dispatch fullscreen",
            "dispatch togglefloating",
            "dispatch movetoworkspace 5",
            "dispatch movetoworkspace 6,address:0xabc",
        ]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_hyprland_lua_dispatch_protocol() {
    // Hyprland >= 0.55 rejects legacy text dispatch: the socket wraps
    // args as `hl.dispatch(<args>)` and Lua-parse-errors. The backend
    // must detect this, translate to `hl.dsp.*` form, retry, and pin
    // the syntax for subsequent calls.
    let dir = tmpdir("luadispatch");
    let (backend, cmd, _evt) = backend_at(&dir);
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let _server = common::unix_rpc_server(
        &cmd,
        Arc::new(move |payload: &str| {
            seen2.lock().unwrap().push(payload.to_string());
            if let Some(args) = payload.strip_prefix("dispatch ") {
                if args.starts_with("hl.dsp.") {
                    return "ok".to_string();
                }
                return format!("error: [string \"return hl.dispatch({args})\"]:1: syntax error");
            }
            "ok".to_string()
        }),
    );

    backend.switch_workspace(4).await.unwrap();
    // Syntax now pinned: the next dispatch skips the legacy attempt.
    backend.close_window(None).await.unwrap();
    backend.toggle_fullscreen(None).await.unwrap();
    backend
        .move_window_to_workspace(Some("0xabc"), 6)
        .await
        .unwrap();

    let sent = seen.lock().unwrap().clone();
    assert_eq!(
        sent,
        vec![
            "dispatch workspace 4",
            "dispatch hl.dsp.focus({ workspace = 4 })",
            "dispatch hl.dsp.window.close()",
            "dispatch hl.dsp.window.fullscreen({})",
            "dispatch hl.dsp.window.move({ workspace = 6, window = \"address:0xabc\" })",
        ]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_hyprland_parse_edges() {
    let dir = tmpdir("parse");
    let (backend, cmd, _evt) = backend_at(&dir);
    let _server = common::unix_rpc_server(
        &cmd,
        Arc::new(|payload: &str| match payload {
            "j/activewindow" => "{}".to_string(),
            "j/clients" => r#"[
                {"title":"no address"},
                {"address":"0x1","title":"T","class":"c","workspace":{"id":3},"fullscreen":1}
            ]"#
            .to_string(),
            "j/workspaces" => r#"[{"id":1,"name":"1","monitor":"m","windows":0}]"#.to_string(),
            "j/activeworkspace" => "not json".to_string(),
            _ => "ok".to_string(),
        }),
    );

    // Empty object → no address → None.
    assert!(backend.get_active_window().await.unwrap().is_none());

    let windows = backend.list_windows().await.unwrap();
    assert_eq!(windows.len(), 1);
    assert!(windows[0].is_fullscreen);
    assert!(windows[0].geometry.is_none());

    // Malformed activeworkspace reply degrades to "none active".
    let workspaces = backend.list_workspaces().await.unwrap();
    assert_eq!(workspaces.len(), 1);
    assert!(!workspaces[0].is_active);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_hyprland_event_socket_unavailable() {
    let dir = tmpdir("noevt");
    let (backend, cmd, _evt) = backend_at(&dir);
    let _server = common::hypr_cmd_server(&cmd);

    let mut rx = backend.subscribe_events().unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(rx.try_recv().is_err());

    // Second subscribe does not spawn a duplicate reader.
    let mut rx2 = backend.subscribe_events().unwrap();
    backend.switch_workspace(1).await.unwrap();
    assert_eq!(rx2.recv().await.unwrap(), DesktopEvent::WorkspaceChanged(1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_hyprland_event_line_edge_cases() {
    let dir = tmpdir("evt");
    let (backend, _cmd, evt) = backend_at(&dir);
    let _server = common::hypr_event_server(
        &evt,
        vec![
            "workspace>>notanum".to_string(),
            "openwindow>>0x1".to_string(),
            "activewindowv2>>0x999".to_string(),
        ],
    );
    let mut rx = backend.subscribe_events().unwrap();
    let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event, DesktopEvent::ActiveWindowChanged(None));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_hyprland_peer_reset_errors() {
    let dir = tmpdir("reset");
    let (backend, cmd, _evt) = backend_at(&dir);
    let _server = common::unix_drop_server(&cmd);

    // Query and dispatch paths: the compositor accepts the connection then
    // drops it mid-request, surfacing real socket errors.
    for _ in 0..4 {
        assert!(backend.get_active_window().await.is_err());
        assert!(backend.switch_workspace(3).await.is_err());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_hyprland_malformed_event_fields() {
    let dir = tmpdir("evtfields");
    let (backend, _cmd, evt) = backend_at(&dir);
    let _server = common::hypr_event_server(
        &evt,
        vec![
            // Missing workspace field → parse bail before WindowOpened.
            "openwindow>>0x1,2".to_string(),
            // Address only → workspace field missing → parse bail.
            "openwindow>>0x2".to_string(),
            // Address + workspace + class, no title → valid partial event.
            "openwindow>>0x3,4,kitty".to_string(),
        ],
    );
    let mut rx = backend.subscribe_events().unwrap();
    match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap()
    {
        DesktopEvent::WindowOpened(w) => {
            assert_eq!(w.id, "0x3");
            assert_eq!(w.workspace_id, 4);
            assert_eq!(w.title, "");
        }
        other => panic!("unexpected event: {:?}", other),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_hyprland_malformed_entry_fields() {
    let dir = tmpdir("entryfields");
    let (backend, cmd, _evt) = backend_at(&dir);
    let _server = common::unix_rpc_server(
        &cmd,
        Arc::new(|payload: &str| match payload {
            // Entries lacking a string "address" are dropped.
            "j/clients" => r#"[{"address":42},{"title":"no address"}]"#.to_string(),
            // Entries lacking a numeric "id" are dropped.
            "j/workspaces" => r#"[{"id":"one"},{"name":"no id"}]"#.to_string(),
            "j/activeworkspace" => "{}".to_string(),
            _ => "ok".to_string(),
        }),
    );

    assert!(backend.list_windows().await.unwrap().is_empty());
    assert!(backend.list_workspaces().await.unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_live_hyprland_probe_read_only() {
    // Read-only probe against a real Hyprland session when one is running;
    // exercises the true wire format without mutating the session.
    let Ok(his) = std::env::var("HYPRLAND_INSTANCE_SIGNATURE") else {
        return;
    };
    let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") else {
        return;
    };
    let cmd = std::path::Path::new(&xdg)
        .join("hypr")
        .join(&his)
        .join(".socket.sock");
    let evt = cmd.with_file_name(".socket2.sock");
    if !cmd.exists() {
        return;
    }
    let backend = HyprlandBackend::new().await.unwrap();
    assert_eq!(backend.command_socket_path(), cmd);
    assert_eq!(backend.event_socket_path(), evt);
    let _active = backend.get_active_window().await.unwrap();
    let _windows = backend.list_windows().await.unwrap();
    let workspaces = backend.list_workspaces().await.unwrap();
    assert!(!workspaces.is_empty());
    assert!(workspaces.iter().any(|w| w.is_active));
}
