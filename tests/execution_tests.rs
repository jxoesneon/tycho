//! Execution layer tests: desktop intent dispatch and system subprocess runs.

use rust_voice_assistant::desktop::DesktopManager;
use rust_voice_assistant::execution::{DesktopExecutor, SystemExecutor};
use rust_voice_assistant::Error;
use std::time::Duration;

#[tokio::test]
async fn test_desktop_executor_workspace_intents() {
    let executor = DesktopExecutor::new(DesktopManager::init_mock());

    let res = executor.execute("switch_workspace_3", None).await.unwrap();
    assert_eq!(res.action_performed, "switch_workspace_3");
    assert_eq!(res.output_message, "Switched to workspace 3");

    let err = executor
        .execute("switch_workspace_xyz", None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Routing(_)));
}

#[tokio::test]
async fn test_desktop_executor_window_intents() {
    let executor = DesktopExecutor::new(DesktopManager::init_mock());

    // Bare window ids still resolve.
    let res = executor
        .execute("focus_window", Some("win-001"))
        .await
        .unwrap();
    assert_eq!(res.output_message, "Focused Terminal");

    // No target focuses the active window.
    let res = executor.execute("focus_window", None).await.unwrap();
    assert_eq!(res.output_message, "Focused Terminal");

    // Spoken names resolve by app-id/title.
    let res = executor
        .execute("focus_window", Some("kitty"))
        .await
        .unwrap();
    assert_eq!(res.output_message, "Focused Terminal");

    assert!(executor
        .execute("focus_window", Some("missing"))
        .await
        .is_err());

    let res = executor.execute("toggle_fullscreen", None).await.unwrap();
    assert_eq!(res.output_message, "Toggled fullscreen");

    // Named close resolves the matching window, not the active one.
    let res = executor
        .execute("close_window", Some("terminal"))
        .await
        .unwrap();
    assert_eq!(res.output_message, "Closed Terminal");

    // With the only window closed, untargeted close reports the empty
    // desktop honestly instead of fabricating a target.
    assert!(executor.execute("close_window", None).await.is_err());

    // Unknown names error instead of closing the wrong window.
    assert!(executor
        .execute("close_window", Some("nonexistent-app"))
        .await
        .is_err());
}

struct FailingBackend {
    caps: rust_voice_assistant::desktop::BackendCapabilities,
}

#[async_trait::async_trait]
impl rust_voice_assistant::desktop::DesktopBackend for FailingBackend {
    fn name(&self) -> &'static str {
        "Failing"
    }
    fn capabilities(&self) -> &rust_voice_assistant::desktop::BackendCapabilities {
        &self.caps
    }
    async fn get_active_window(
        &self,
    ) -> Result<
        Option<rust_voice_assistant::desktop::WindowContext>,
        rust_voice_assistant::desktop::DesktopError,
    > {
        Err(rust_voice_assistant::desktop::DesktopError::IpcError(
            "backend offline".into(),
        ))
    }
    async fn list_windows(
        &self,
    ) -> Result<
        Vec<rust_voice_assistant::desktop::WindowContext>,
        rust_voice_assistant::desktop::DesktopError,
    > {
        Err(rust_voice_assistant::desktop::DesktopError::IpcError(
            "backend offline".into(),
        ))
    }
    async fn list_workspaces(
        &self,
    ) -> Result<
        Vec<rust_voice_assistant::desktop::WorkspaceContext>,
        rust_voice_assistant::desktop::DesktopError,
    > {
        Err(rust_voice_assistant::desktop::DesktopError::IpcError(
            "backend offline".into(),
        ))
    }
    async fn switch_workspace(
        &self,
        _t: i32,
    ) -> Result<(), rust_voice_assistant::desktop::DesktopError> {
        Err(rust_voice_assistant::desktop::DesktopError::IpcError(
            "backend offline".into(),
        ))
    }
    async fn focus_window(
        &self,
        _w: &str,
    ) -> Result<(), rust_voice_assistant::desktop::DesktopError> {
        Err(rust_voice_assistant::desktop::DesktopError::IpcError(
            "backend offline".into(),
        ))
    }
    async fn close_window(
        &self,
        _w: Option<&str>,
    ) -> Result<(), rust_voice_assistant::desktop::DesktopError> {
        Err(rust_voice_assistant::desktop::DesktopError::IpcError(
            "backend offline".into(),
        ))
    }
    async fn toggle_fullscreen(
        &self,
        _w: Option<&str>,
    ) -> Result<(), rust_voice_assistant::desktop::DesktopError> {
        Err(rust_voice_assistant::desktop::DesktopError::IpcError(
            "backend offline".into(),
        ))
    }
    async fn toggle_floating(
        &self,
        _w: Option<&str>,
    ) -> Result<(), rust_voice_assistant::desktop::DesktopError> {
        Err(rust_voice_assistant::desktop::DesktopError::IpcError(
            "backend offline".into(),
        ))
    }
    async fn move_window_to_workspace(
        &self,
        _w: Option<&str>,
        _t: i32,
    ) -> Result<(), rust_voice_assistant::desktop::DesktopError> {
        Err(rust_voice_assistant::desktop::DesktopError::IpcError(
            "backend offline".into(),
        ))
    }
    fn subscribe_events(
        &self,
    ) -> Result<
        tokio::sync::broadcast::Receiver<rust_voice_assistant::desktop::DesktopEvent>,
        rust_voice_assistant::desktop::DesktopError,
    > {
        let (_tx, rx) = tokio::sync::broadcast::channel(4);
        Ok(rx)
    }
}

#[tokio::test]
async fn test_desktop_executor_backend_error_propagation() {
    let executor = DesktopExecutor::new(DesktopManager::with_backend(std::sync::Arc::new(
        FailingBackend {
            caps: Default::default(),
        },
    )));

    for intent in [
        "switch_workspace_2",
        "close_window",
        "toggle_fullscreen",
        "focus_window",
    ] {
        let err = executor.execute(intent, None).await.unwrap_err();
        assert!(
            err.to_string().contains("backend offline"),
            "{} did not propagate backend error",
            intent
        );
    }
}

#[tokio::test]
async fn test_desktop_executor_unknown_intents() {
    let executor = DesktopExecutor::new(DesktopManager::init_mock());

    assert_eq!(
        executor
            .execute("some_other_intent", None)
            .await
            .unwrap()
            .output_message,
        "Executed some_other_intent"
    );
}

#[tokio::test]
async fn test_desktop_executor_context_summary() {
    let executor = DesktopExecutor::new(DesktopManager::init_mock());
    let summary = executor.context_summary().await;
    assert!(summary.contains("Mock Compositor"));
    assert!(summary.contains("Terminal"));
    assert!(summary.contains("Workspace: 1"));
}

#[tokio::test]
async fn test_desktop_executor_context_summary_empty_desktop() {
    let executor = DesktopExecutor::new(DesktopManager::init_mock());
    executor
        .execute("close_window", Some("win-001"))
        .await
        .unwrap();
    let summary = executor.context_summary().await;
    assert!(summary.contains("Active Window: 'Desktop'"));
}

#[tokio::test]
async fn test_system_executor_success() {
    let res = SystemExecutor::run_command("echo", &["hello"], Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(res.stdout, "hello");
    assert_eq!(res.exit_code, Some(0));
    assert!(!res.timed_out);
}

#[tokio::test]
async fn test_system_executor_stderr_and_exit_code() {
    let res = SystemExecutor::run_command(
        "sh",
        &["-c", "echo oops >&2; exit 3"],
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(res.stderr, "oops");
    assert_eq!(res.exit_code, Some(3));
}

#[tokio::test]
async fn test_system_executor_timeout() {
    let res = SystemExecutor::run_command("sleep", &["10"], Duration::from_millis(100))
        .await
        .unwrap();
    assert!(res.timed_out);
    assert_eq!(res.exit_code, None);
    assert_eq!(res.stderr, "Process timed out");
}

#[tokio::test]
async fn test_system_executor_spawn_failure() {
    let err =
        SystemExecutor::run_command("definitely-not-a-binary-xyz", &[], Duration::from_secs(1))
            .await
            .unwrap_err();
    assert!(matches!(err, Error::Io(_)));
}
