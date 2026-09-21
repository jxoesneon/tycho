//! DesktopManager orchestration tests (no environment mutation).

use async_trait::async_trait;
use rust_voice_assistant::desktop::{
    BackendCapabilities, DesktopBackend, DesktopError, DesktopEvent, DesktopManager, MockBackend,
    WindowContext, WorkspaceContext,
};
use std::sync::Arc;
use tokio::sync::broadcast;

struct NullBackend {
    caps: BackendCapabilities,
}

struct BrokenBackend {
    caps: BackendCapabilities,
}

macro_rules! stub_backend {
    ($name:ident, $active:expr) => {
        #[async_trait]
        impl DesktopBackend for $name {
            fn name(&self) -> &'static str {
                "Stub"
            }
            fn capabilities(&self) -> &BackendCapabilities {
                &self.caps
            }
            async fn get_active_window(&self) -> Result<Option<WindowContext>, DesktopError> {
                $active
            }
            async fn list_windows(&self) -> Result<Vec<WindowContext>, DesktopError> {
                Ok(Vec::new())
            }
            async fn list_workspaces(&self) -> Result<Vec<WorkspaceContext>, DesktopError> {
                Ok(Vec::new())
            }
            async fn switch_workspace(&self, _t: i32) -> Result<(), DesktopError> {
                Ok(())
            }
            async fn focus_window(&self, _w: &str) -> Result<(), DesktopError> {
                Ok(())
            }
            async fn close_window(&self, _w: Option<&str>) -> Result<(), DesktopError> {
                Ok(())
            }
            async fn toggle_fullscreen(&self, _w: Option<&str>) -> Result<(), DesktopError> {
                Ok(())
            }
            async fn toggle_floating(&self, _w: Option<&str>) -> Result<(), DesktopError> {
                Ok(())
            }
            async fn move_window_to_workspace(
                &self,
                _w: Option<&str>,
                _t: i32,
            ) -> Result<(), DesktopError> {
                Ok(())
            }
            fn subscribe_events(&self) -> Result<broadcast::Receiver<DesktopEvent>, DesktopError> {
                let (tx, rx) = broadcast::channel(4);
                drop(tx);
                Ok(rx)
            }
        }
    };
}

stub_backend!(NullBackend, Ok(None));
stub_backend!(
    BrokenBackend,
    Err(DesktopError::Internal("backend offline".into()))
);

/// Backend where every compositor call fails — exercises dispatch error
/// propagation for each action kind.
struct AllFailBackend {
    caps: BackendCapabilities,
}

#[async_trait]
impl DesktopBackend for AllFailBackend {
    fn name(&self) -> &'static str {
        "AllFail"
    }
    fn capabilities(&self) -> &BackendCapabilities {
        &self.caps
    }
    async fn get_active_window(&self) -> Result<Option<WindowContext>, DesktopError> {
        Err(DesktopError::Internal("down".into()))
    }
    async fn list_windows(&self) -> Result<Vec<WindowContext>, DesktopError> {
        Err(DesktopError::Internal("down".into()))
    }
    async fn list_workspaces(&self) -> Result<Vec<WorkspaceContext>, DesktopError> {
        Err(DesktopError::Internal("down".into()))
    }
    async fn switch_workspace(&self, _t: i32) -> Result<(), DesktopError> {
        Err(DesktopError::Internal("down".into()))
    }
    async fn focus_window(&self, _w: &str) -> Result<(), DesktopError> {
        Err(DesktopError::Internal("down".into()))
    }
    async fn close_window(&self, _w: Option<&str>) -> Result<(), DesktopError> {
        Err(DesktopError::Internal("down".into()))
    }
    async fn toggle_fullscreen(&self, _w: Option<&str>) -> Result<(), DesktopError> {
        Err(DesktopError::Internal("down".into()))
    }
    async fn toggle_floating(&self, _w: Option<&str>) -> Result<(), DesktopError> {
        Err(DesktopError::Internal("down".into()))
    }
    async fn move_window_to_workspace(
        &self,
        _w: Option<&str>,
        _t: i32,
    ) -> Result<(), DesktopError> {
        Err(DesktopError::Internal("down".into()))
    }
    fn subscribe_events(&self) -> Result<broadcast::Receiver<DesktopEvent>, DesktopError> {
        Err(DesktopError::Internal("down".into()))
    }
}

#[tokio::test]
async fn test_manager_dispatch_backend_error_propagation() {
    let manager = DesktopManager::with_backend(Arc::new(AllFailBackend {
        caps: BackendCapabilities::default(),
    }));
    for args in [
        ("switch_workspace", Some("2")),
        ("focus_window", Some("win-1")),
        ("close_window", Some("win-1")),
        ("toggle_fullscreen", None),
        ("toggle_floating", None),
        ("move_to_workspace", Some("3")),
        ("list_workspaces", None),
    ] {
        let err = manager.dispatch_action(args.0, args.1).await.unwrap_err();
        assert!(err.to_string().contains("down"), "{}: {}", args.0, err);
    }
}

#[tokio::test]
async fn test_manager_with_backend_and_init_mock() {
    let manager = DesktopManager::with_backend(Arc::new(MockBackend::new()));
    assert_eq!(manager.backend().name(), "Mock Compositor");

    let mock_manager = DesktopManager::init_mock();
    assert_eq!(mock_manager.backend().name(), "Mock Compositor");
}

#[tokio::test]
async fn test_manager_context_summary_branches() {
    let populated = DesktopManager::init_mock();
    let summary = populated.get_context_summary().await;
    assert!(summary.contains("Active App: kitty"));
    assert!(summary.contains("Terminal"));

    let empty = DesktopManager::with_backend(Arc::new(NullBackend {
        caps: BackendCapabilities::default(),
    }));
    assert_eq!(
        empty.get_context_summary().await,
        "Active App: None (Empty Desktop)"
    );

    let broken = DesktopManager::with_backend(Arc::new(BrokenBackend {
        caps: BackendCapabilities::default(),
    }));
    let summary = broken.get_context_summary().await;
    assert!(summary.starts_with("Desktop Context Unavailable:"));
}

#[tokio::test]
async fn test_manager_dispatch_workspace_actions() {
    let manager = DesktopManager::init_mock();

    let res = manager
        .dispatch_action("workspace_switch", Some("2"))
        .await
        .unwrap();
    assert_eq!(res, "Switched to workspace 2");

    let res = manager
        .dispatch_action("switch_workspace", Some("4"))
        .await
        .unwrap();
    assert_eq!(res, "Switched to workspace 4");

    assert!(manager
        .dispatch_action("switch_workspace", None)
        .await
        .is_err());
    assert!(manager
        .dispatch_action("switch_workspace", Some("abc"))
        .await
        .is_err());
}

#[tokio::test]
async fn test_manager_dispatch_window_actions() {
    let manager = DesktopManager::init_mock();

    let res = manager
        .dispatch_action("window_focus", Some("win-001"))
        .await
        .unwrap();
    assert_eq!(res, "Focused window win-001");
    assert!(manager.dispatch_action("focus_window", None).await.is_err());

    let res = manager
        .dispatch_action("window_fullscreen", None)
        .await
        .unwrap();
    assert_eq!(res, "Toggled fullscreen");
    let res = manager
        .dispatch_action("toggle_fullscreen", Some("win-001"))
        .await
        .unwrap();
    assert_eq!(res, "Toggled fullscreen");

    let res = manager.dispatch_action("window_float", None).await.unwrap();
    assert_eq!(res, "Toggled floating mode");
    let res = manager
        .dispatch_action("toggle_floating", Some("win-001"))
        .await
        .unwrap();
    assert_eq!(res, "Toggled floating mode");

    let res = manager
        .dispatch_action("move_to_workspace", Some("3"))
        .await
        .unwrap();
    assert_eq!(res, "Moved active window to workspace 3");
    assert!(manager
        .dispatch_action("move_to_workspace", None)
        .await
        .is_err());
    assert!(manager
        .dispatch_action("move_to_workspace", Some("x"))
        .await
        .is_err());

    let res = manager
        .dispatch_action("window_close", Some("win-001"))
        .await
        .unwrap();
    assert_eq!(res, "Closed window");
    assert!(manager.dispatch_action("close_window", None).await.is_err());
}

#[tokio::test]
async fn test_manager_dispatch_info_actions() {
    let manager = DesktopManager::init_mock();

    let ctx = manager.dispatch_action("get_context", None).await.unwrap();
    assert!(ctx.contains("Active App:"));

    let res = manager
        .dispatch_action("list_workspaces", None)
        .await
        .unwrap();
    assert!(res.starts_with("Workspaces: ["));
    assert!(res.contains("(active)"));

    let err = manager
        .dispatch_action("definitely_not_an_action", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not recognized"));
}

#[tokio::test]
async fn test_manager_init_with_config() {
    // Explicit mock selection binds the in-memory backend.
    let m = DesktopManager::init_with_config("mock", true).await;
    assert_eq!(m.backend_name(), "Mock Compositor");

    // backend=auto + auto_detect=false also resolves to mock.
    let m = DesktopManager::init_with_config("auto", false).await;
    assert_eq!(m.backend_name(), "Mock Compositor");

    // auto + detect binds whatever the session offers, or mock —
    // either way the call must not fail.
    let m = DesktopManager::init_with_config("auto", true).await;
    assert!(!m.backend_name().is_empty());

    // A named backend that can't answer falls back to mock, not a crash.
    let m = DesktopManager::init_with_config("kde", true).await;
    assert!(!m.backend_name().is_empty());
}
