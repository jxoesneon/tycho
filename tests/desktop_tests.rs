//! Compositor backend tests.

use rust_voice_assistant::desktop::{DesktopBackend, MockBackend};

#[tokio::test]
async fn test_mock_backend_operations() {
    let backend = MockBackend::new();
    assert_eq!(backend.get_current_workspace().await.unwrap(), 1);

    backend.switch_workspace(3).await.unwrap();
    assert_eq!(backend.get_current_workspace().await.unwrap(), 3);

    backend.set_volume(80).await.unwrap();
    assert_eq!(backend.volume.load(std::sync::atomic::Ordering::SeqCst), 80);
}
