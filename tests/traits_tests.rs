//! Desktop trait model tests: capabilities, context types, events, serde.

use rust_voice_assistant::desktop::{
    BackendCapabilities, DesktopEvent, WindowContext, WindowGeometry, WorkspaceContext,
};

#[test]
fn test_backend_capabilities_default() {
    let caps = BackendCapabilities::default();
    assert!(caps.supports_window_geometry);
    assert!(caps.supports_dynamic_workspaces);
    assert!(caps.supports_event_stream);
    assert!(!caps.supports_nested_testing);
    assert!(caps.supports_window_floating);
    assert!(caps.supports_window_fullscreen);
    assert!(caps.supports_move_to_workspace);
}

#[test]
fn test_window_geometry_serde() {
    let geo = WindowGeometry {
        x: -10,
        y: 20,
        width: 800,
        height: 600,
    };
    let json = serde_json::to_string(&geo).unwrap();
    let back: WindowGeometry = serde_json::from_str(&json).unwrap();
    assert_eq!(back, geo);
    assert_eq!(
        WindowGeometry::default(),
        WindowGeometry {
            x: 0,
            y: 0,
            width: 0,
            height: 0
        }
    );
}

#[test]
fn test_window_context_serde() {
    let win = WindowContext {
        id: "w1".into(),
        title: "Terminal".into(),
        app_id: "kitty".into(),
        workspace_id: 2,
        is_floating: true,
        is_fullscreen: false,
        geometry: Some(WindowGeometry {
            x: 1,
            y: 2,
            width: 3,
            height: 4,
        }),
        pid: Some(99),
    };
    let json = serde_json::to_string(&win).unwrap();
    let back: WindowContext = serde_json::from_str(&json).unwrap();
    assert_eq!(back, win);
}

#[test]
fn test_workspace_context_serde() {
    let ws = WorkspaceContext {
        id: 3,
        name: "web".into(),
        is_active: false,
        monitor: "HDMI-1".into(),
        windows_count: 2,
    };
    let json = serde_json::to_string(&ws).unwrap();
    let back: WorkspaceContext = serde_json::from_str(&json).unwrap();
    assert_eq!(back, ws);
}

#[test]
fn test_desktop_event_variants() {
    let win = WindowContext {
        id: "w".into(),
        title: "t".into(),
        app_id: "a".into(),
        workspace_id: 1,
        is_floating: false,
        is_fullscreen: false,
        geometry: None,
        pid: None,
    };
    let events = vec![
        DesktopEvent::ActiveWindowChanged(Some(win.clone())),
        DesktopEvent::ActiveWindowChanged(None),
        DesktopEvent::WorkspaceChanged(4),
        DesktopEvent::WindowOpened(win),
        DesktopEvent::WindowClosed("w".into()),
        DesktopEvent::MonitorChanged("DP-2".into()),
    ];
    for e in events {
        let dbg = format!("{:?}", e);
        assert!(!dbg.is_empty());
        assert_eq!(e.clone(), e);
    }
}
