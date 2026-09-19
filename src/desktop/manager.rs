//! Desktop backend selection and compositor detection.

use super::traits::DesktopBackend;
use super::mock::MockBackend;
use crate::error::{Error, Result};
use std::sync::Arc;

pub struct DesktopManager {
    backend: Arc<dyn DesktopBackend>,
    backend_name: String,
}

impl DesktopManager {
    pub fn new(backend: Arc<dyn DesktopBackend>, name: impl Into<String>) -> Self {
        Self {
            backend,
            backend_name: name.into(),
        }
    }

    pub async fn init_auto() -> Result<Self> {
        if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
            #[cfg(feature = "hyprland")]
            {
                let backend = Arc::new(super::hyprland::HyprlandBackend::new());
                return Ok(Self::new(backend, "Hyprland (Native IPC)"));
            }
        }

        if std::env::var("KDE_FULL_SESSION").is_ok() {
            #[cfg(feature = "kde")]
            {
                let backend = Arc::new(super::kde::KdeBackend::new());
                return Ok(Self::new(backend, "KDE Plasma (KWin D-Bus)"));
            }
        }

        let mock = Arc::new(MockBackend::new());
        Ok(Self::new(mock, "Mock Desktop (Environment Neutral)"))
    }

    pub fn backend(&self) -> Arc<dyn DesktopBackend> {
        self.backend.clone()
    }

    pub fn backend_name(&self) -> &str {
        &self.backend_name
    }
}
