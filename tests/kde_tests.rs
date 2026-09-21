//! KDE Plasma backend tests over a private `dbus-daemon` session bus with a
//! fake `org.kde.KWin` service — real D-Bus round trips, deterministic state.

use rust_voice_assistant::desktop::{DesktopBackend, DesktopEvent, KdeBackend};

mod common;

async fn backend() -> Option<(
    KdeBackend,
    common::SharedKwin,
    zbus::Connection,
    tokio::process::Child,
)> {
    let (addr, bus) = common::fake_bus().await?;
    let state = common::fake_kwin_state();
    let service = common::fake_kwin_bus(&addr, state.clone()).await;
    let conn = common::bus_client(&addr).await;
    let backend = KdeBackend::with_connection(conn).await.unwrap();
    Some((backend, state, service, bus))
}

#[tokio::test]
async fn test_kde_backend_basics() {
    let Some((backend, _state, _svc, _bus)) = backend().await else {
        return;
    };
    assert_eq!(backend.name(), "KDE Plasma (KWin / D-Bus)");
    assert!(backend.capabilities().supports_event_stream);

    let active = backend.get_active_window().await.unwrap().unwrap();
    assert_eq!(active.id, "kde-win-001");
    assert_eq!(active.title, "Konsole");

    let windows = backend.list_windows().await.unwrap();
    assert_eq!(windows.len(), 1);

    let workspaces = backend.list_workspaces().await.unwrap();
    // Exactly the desktops KWin reports — no fabricated entries.
    assert_eq!(workspaces.len(), 3);
    assert!(workspaces[0].is_active);
    assert_eq!(workspaces[0].name, "Main");
    assert_eq!(workspaces[2].name, "Media");
}

#[tokio::test]
async fn test_kde_backend_workspace_switching() {
    let Some((backend, state, _svc, _bus)) = backend().await else {
        return;
    };
    let mut rx = backend.subscribe_events().unwrap();

    backend.switch_workspace(2).await.unwrap();
    assert_eq!(rx.recv().await.unwrap(), DesktopEvent::WorkspaceChanged(2));
    assert_eq!(state.lock().unwrap().current_desktop, 2);

    let workspaces = backend.list_workspaces().await.unwrap();
    assert!(workspaces[1].is_active);
    assert!(!workspaces[0].is_active);

    let err = backend.switch_workspace(99).await.unwrap_err();
    assert!(err.to_string().contains("Workspace not found"));
}

#[tokio::test]
async fn test_kde_backend_window_focus() {
    let Some((backend, state, _svc, _bus)) = backend().await else {
        return;
    };
    let mut rx = backend.subscribe_events().unwrap();

    backend.focus_window("kde-win-001").await.unwrap();
    match rx.recv().await.unwrap() {
        DesktopEvent::ActiveWindowChanged(Some(w)) => assert_eq!(w.id, "kde-win-001"),
        other => panic!("unexpected event: {:?}", other),
    }
    assert_eq!(state.lock().unwrap().active.as_deref(), Some("kde-win-001"));

    let err = backend.focus_window("missing").await.unwrap_err();
    assert!(err.to_string().contains("Window not found"));
}

#[tokio::test]
async fn test_kde_backend_window_state_toggles() {
    let Some((backend, _state, _svc, _bus)) = backend().await else {
        return;
    };

    backend
        .toggle_fullscreen(Some("kde-win-001"))
        .await
        .unwrap();
    let win = backend.get_active_window().await.unwrap().unwrap();
    assert!(win.is_fullscreen);
    backend.toggle_fullscreen(None).await.unwrap();
    assert!(
        !backend
            .get_active_window()
            .await
            .unwrap()
            .unwrap()
            .is_fullscreen
    );
    assert!(backend.toggle_fullscreen(Some("missing")).await.is_err());

    // KWin scripting does not expose a floating toggle — honest error.
    assert!(backend.toggle_floating(None).await.is_err());
    assert!(backend.toggle_floating(Some("kde-win-001")).await.is_err());
}

#[tokio::test]
async fn test_kde_backend_move_and_close_window() {
    let Some((backend, _state, _svc, _bus)) = backend().await else {
        return;
    };
    let mut rx = backend.subscribe_events().unwrap();

    backend.move_window_to_workspace(None, 3).await.unwrap();
    let win = backend.get_active_window().await.unwrap().unwrap();
    assert_eq!(win.workspace_id, 3);
    assert!(backend
        .move_window_to_workspace(Some("missing"), 2)
        .await
        .is_err());

    backend.close_window(Some("kde-win-001")).await.unwrap();
    assert_eq!(
        rx.recv().await.unwrap(),
        DesktopEvent::ActiveWindowChanged(None)
    );
    assert_eq!(
        rx.recv().await.unwrap(),
        DesktopEvent::WindowClosed("kde-win-001".to_string())
    );
    assert!(backend.list_windows().await.unwrap().is_empty());
    assert!(backend.get_active_window().await.unwrap().is_none());
    assert!(backend.close_window(None).await.is_err());
    assert!(backend.close_window(Some("gone")).await.is_err());
}

#[tokio::test]
async fn test_kde_backend_no_active_window_errors() {
    let Some((backend, _state, _svc, _bus)) = backend().await else {
        return;
    };
    backend.close_window(None).await.unwrap();
    assert!(backend.get_active_window().await.unwrap().is_none());

    assert!(backend.toggle_fullscreen(None).await.is_err());
    assert!(backend.toggle_floating(None).await.is_err());
    assert!(backend.move_window_to_workspace(None, 2).await.is_err());
    assert!(backend.close_window(None).await.is_err());
}

#[tokio::test]
async fn test_kde_backend_degraded_refresh() {
    // Exotic service state: a window on an unknown desktop, a ghost active
    // id, a non-array desktops property — refresh must degrade gracefully.
    let Some((addr, _bus)) = common::fake_bus().await else {
        return;
    };
    let state = common::fake_kwin_state_exotic();
    let _svc = common::fake_kwin_bus(&addr, state.clone()).await;
    let backend = KdeBackend::with_connection(common::bus_client(&addr).await)
        .await
        .unwrap();

    let windows = backend.list_windows().await.unwrap();
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].workspace_id, 7);
    assert!(backend.get_active_window().await.unwrap().is_none());
    // bad_desktops → generated workspace names.
    let workspaces = backend.list_workspaces().await.unwrap();
    assert_eq!(workspaces[0].name, "Desktop 1");

    // `call` failing during refresh → window/active queries degrade.
    state.lock().unwrap().fail_call = true;
    let degraded = KdeBackend::with_connection(common::bus_client(&addr).await)
        .await
        .unwrap();
    assert!(degraded.list_windows().await.unwrap().is_empty());
    assert!(degraded.get_active_window().await.unwrap().is_none());

    // A failing `call` method surfaces honest IPC errors.
    assert!(backend.focus_window("kde-win-777").await.is_err());
    assert!(backend.close_window(Some("kde-win-777")).await.is_err());
    state.lock().unwrap().fail_call = false;

    // A non-string `call` reply is reported as an unexpected value.
    state.lock().unwrap().bad_reply = true;
    assert!(backend.focus_window("kde-win-777").await.is_err());
    state.lock().unwrap().bad_reply = false;

    // A failing `loadScript` reports scripting as unavailable (fresh
    // backend whose bridge was never loaded).
    state.lock().unwrap().fail_load = true;
    let unbridged = KdeBackend::with_connection(common::bus_client(&addr).await)
        .await
        .unwrap();
    let err = unbridged.focus_window("kde-win-777").await.unwrap_err();
    assert!(err.to_string().contains("scripting unavailable"));
}

#[tokio::test]
async fn test_kde_backend_service_disconnect_errors() {
    let Some((backend, _state, svc, _bus)) = backend().await else {
        return;
    };
    // Dropping the service connection releases org.kde.KWin; subsequent
    // calls surface honest IPC errors rather than fabricated success.
    drop(svc);
    assert!(backend.switch_workspace(2).await.is_err());
    assert!(backend.focus_window("kde-win-001").await.is_err());
}

#[tokio::test]
async fn test_kde_backend_unowned_service_fails() {
    let Some((addr, _bus)) = common::fake_bus().await else {
        return;
    };
    let conn = common::bus_client(&addr).await;
    let err = match KdeBackend::with_connection(conn).await {
        Err(e) => e,
        Ok(_) => panic!("backend init succeeded without org.kde.KWin"),
    };
    assert!(err.to_string().contains("org.kde.KWin"));
}

#[tokio::test]
async fn test_kde_backend_reply_type_edges() {
    // Each variant must be selected before the fake service registers.
    let Some((addr, _bus)) = common::fake_bus().await else {
        return;
    };

    // currentDesktop replying `s` instead of `i` → IPC error, refresh
    // degrades to desktop 1.
    let state = common::fake_kwin_state();
    state.lock().unwrap().bad_current = true;
    let _svc = common::fake_kwin_bus(&addr, state.clone()).await;
    let backend = KdeBackend::with_connection(common::bus_client(&addr).await)
        .await
        .unwrap();
    let workspaces = backend.list_workspaces().await.unwrap();
    assert_eq!(workspaces[0].name, "Main");
    drop(_svc);

    // currentDesktop itself erroring → refresh degrades to desktop 1.
    let Some((addr, _bus)) = common::fake_bus().await else {
        return;
    };
    let state = common::fake_kwin_state();
    state.lock().unwrap().fail_current = true;
    let _svc = common::fake_kwin_bus(&addr, state.clone()).await;
    let backend = KdeBackend::with_connection(common::bus_client(&addr).await)
        .await
        .unwrap();
    assert!(!backend.list_workspaces().await.unwrap().is_empty());
    drop(_svc);

    // loadScript replying `s` instead of `i` → honest IPC error.
    let Some((addr, _bus)) = common::fake_bus().await else {
        return;
    };
    let state = common::fake_kwin_state();
    state.lock().unwrap().bad_load_type = true;
    let _svc = common::fake_kwin_bus(&addr, state.clone()).await;
    let backend = KdeBackend::with_connection(common::bus_client(&addr).await)
        .await
        .unwrap();
    assert!(backend.focus_window("kde-win-001").await.is_err());
    drop(_svc);

    // desktops as `as` → non-structure entries yield no names.
    let Some((addr, _bus)) = common::fake_bus().await else {
        return;
    };
    let state = common::fake_kwin_state();
    state.lock().unwrap().desktops_strings = true;
    let _svc = common::fake_kwin_bus(&addr, state.clone()).await;
    let backend = KdeBackend::with_connection(common::bus_client(&addr).await)
        .await
        .unwrap();
    let workspaces = backend.list_workspaces().await.unwrap();
    assert_eq!(workspaces[0].name, "Desktop 1");
    drop(_svc);

    // tychoWins returning invalid JSON → refresh degrades, no windows.
    let Some((addr, _bus)) = common::fake_bus().await else {
        return;
    };
    let state = common::fake_kwin_state();
    state.lock().unwrap().bad_wins = true;
    let _svc = common::fake_kwin_bus(&addr, state.clone()).await;
    let backend = KdeBackend::with_connection(common::bus_client(&addr).await)
        .await
        .unwrap();
    assert!(backend.list_windows().await.unwrap().is_empty());
    drop(_svc);
}

#[tokio::test]
async fn test_kde_backend_bus_death_probe_error() {
    let Some((addr, mut bus)) = common::fake_bus().await else {
        return;
    };
    let conn = common::bus_client(&addr).await;
    let _ = bus.kill().await;
    let err = match KdeBackend::with_connection(conn).await {
        Err(e) => e,
        Ok(_) => panic!("backend init succeeded on a dead bus"),
    };
    assert!(err.to_string().contains("bus probe"));
}

#[tokio::test]
async fn test_kde_backend_new_without_session() {
    // On hosts without an owned org.kde.KWin (non-KDE sessions, headless),
    // `new` must fail honestly rather than fabricate a backend.
    if std::env::var_os("KDE_FULL_SESSION").is_none() {
        assert!(KdeBackend::new().await.is_err());
    }
}
