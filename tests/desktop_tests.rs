//! Compositor backend tests.

use rust_voice_assistant::desktop::{DesktopBackend, DesktopEvent, MockBackend};

#[tokio::test]
async fn test_mock_backend_operations() {
    let backend = MockBackend::new();
    assert_eq!(backend.name(), "Mock Compositor");
    assert!(backend.capabilities().supports_nested_testing);

    let active = backend.get_active_window().await.unwrap().unwrap();
    assert_eq!(active.id, "win-001");
    assert_eq!(active.workspace_id, 1);

    let workspaces = backend.list_workspaces().await.unwrap();
    assert_eq!(workspaces.len(), 5);
    assert!(workspaces[0].is_active);

    backend.switch_workspace(3).await.unwrap();
    let workspaces = backend.list_workspaces().await.unwrap();
    assert!(workspaces[2].is_active);
    assert!(!workspaces[0].is_active);
    assert!(backend.get_active_window().await.unwrap().is_none());
}

#[tokio::test]
async fn test_mock_backend_window_lifecycle() {
    let backend = MockBackend::new();

    backend.focus_window("win-001").await.unwrap();
    let err = backend.focus_window("missing").await.unwrap_err();
    assert!(err.to_string().contains("Window not found"));

    backend.toggle_fullscreen(Some("win-001")).await.unwrap();
    let win = backend.get_active_window().await.unwrap().unwrap();
    assert!(win.is_fullscreen);
    backend.toggle_fullscreen(None).await.unwrap();
    let win = backend.get_active_window().await.unwrap().unwrap();
    assert!(!win.is_fullscreen);

    backend.toggle_floating(None).await.unwrap();
    let win = backend.get_active_window().await.unwrap().unwrap();
    assert!(win.is_floating);
    assert!(backend.toggle_floating(Some("missing")).await.is_err());

    backend.move_window_to_workspace(None, 4).await.unwrap();
    let win = backend.get_active_window().await.unwrap().unwrap();
    assert_eq!(win.workspace_id, 4);
    assert!(backend
        .move_window_to_workspace(Some("missing"), 2)
        .await
        .is_err());

    let windows = backend.list_windows().await.unwrap();
    assert_eq!(windows.len(), 1);

    backend.close_window(Some("win-001")).await.unwrap();
    assert!(backend.list_windows().await.unwrap().is_empty());
    assert!(backend.close_window(None).await.is_err());
    assert!(backend.close_window(Some("gone")).await.is_err());
}

#[tokio::test]
async fn test_mock_backend_no_active_window_errors() {
    let backend = MockBackend::new();
    backend.close_window(None).await.unwrap();
    assert!(backend.get_active_window().await.unwrap().is_none());

    assert!(backend.toggle_fullscreen(None).await.is_err());
    assert!(backend.toggle_floating(None).await.is_err());
    assert!(backend.move_window_to_workspace(None, 2).await.is_err());
}

#[tokio::test]
async fn test_mock_backend_switch_to_occupied_workspace() {
    let backend = MockBackend::new();
    let mut rx = backend.subscribe_events().unwrap();

    backend
        .move_window_to_workspace(Some("win-001"), 4)
        .await
        .unwrap();
    backend.switch_workspace(4).await.unwrap();

    assert_eq!(rx.recv().await.unwrap(), DesktopEvent::WorkspaceChanged(4));
    match rx.recv().await.unwrap() {
        DesktopEvent::ActiveWindowChanged(Some(w)) => assert_eq!(w.id, "win-001"),
        other => panic!("unexpected event: {:?}", other),
    }
    let win = backend.get_active_window().await.unwrap().unwrap();
    assert_eq!(win.workspace_id, 4);
}

#[tokio::test]
async fn test_mock_backend_switch_creates_missing_workspace() {
    let backend = MockBackend::new();
    backend.switch_workspace(9).await.unwrap();

    let workspaces = backend.list_workspaces().await.unwrap();
    let created = workspaces.iter().find(|w| w.id == 9).unwrap();
    assert!(created.is_active);
    assert_eq!(created.name, "workspace-9");
    let previous = workspaces.iter().find(|w| w.id == 1).unwrap();
    assert!(!previous.is_active);
}

#[tokio::test]
async fn test_mock_backend_focus_cross_workspace_and_default() {
    let backend = MockBackend::default();
    assert_eq!(backend.name(), "Mock Compositor");
    let mut rx = backend.subscribe_events().unwrap();

    backend
        .move_window_to_workspace(Some("win-001"), 3)
        .await
        .unwrap();
    backend.focus_window("win-001").await.unwrap();

    assert_eq!(rx.recv().await.unwrap(), DesktopEvent::WorkspaceChanged(3));
    match rx.recv().await.unwrap() {
        DesktopEvent::ActiveWindowChanged(Some(w)) => assert_eq!(w.id, "win-001"),
        other => panic!("unexpected event: {:?}", other),
    }
}

#[tokio::test]
async fn test_mock_backend_close_non_active_window() {
    let backend = MockBackend::new();
    backend
        .move_window_to_workspace(Some("win-001"), 2)
        .await
        .unwrap();
    backend.close_window(Some("win-001")).await.unwrap();

    let win_closed = backend.subscribe_events().unwrap();
    drop(win_closed);
    let workspaces = backend.list_workspaces().await.unwrap();
    assert_eq!(workspaces[1].windows_count, 0);
    assert!(backend.get_active_window().await.unwrap().is_none());
}

#[tokio::test]
async fn test_mock_backend_events() {
    let backend = MockBackend::new();
    let mut rx = backend.subscribe_events().unwrap();

    backend.switch_workspace(2).await.unwrap();
    let event = rx.recv().await.unwrap();
    assert_eq!(event, DesktopEvent::WorkspaceChanged(2));
}
