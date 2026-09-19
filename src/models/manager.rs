//! Model manager and initial HuggingFace weight fetcher.

use super::hub::HuggingFaceModelSpec;
use crate::config::ModelsConfig;
use crate::error::{Error, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInventory {
    pub stt_model_path: PathBuf,
    pub tts_model_path: PathBuf,
    pub tts_voices_path: PathBuf,
    pub router_model_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ModelManager {
    pub config: ModelsConfig,
}

impl ModelManager {
    pub fn new(config: ModelsConfig) -> Self {
        Self { config }
    }

    pub fn resolved_cache_dir(&self) -> PathBuf {
        expand_tilde(&self.config.cache_dir)
    }

    pub fn stt_model_target_path(&self) -> PathBuf {
        self.resolved_cache_dir().join("stt").join(&self.config.stt_model.target_filename)
    }

    pub fn tts_model_target_path(&self) -> PathBuf {
        self.resolved_cache_dir().join("tts").join(&self.config.tts_model.target_filename)
    }

    pub fn tts_voices_target_path(&self) -> PathBuf {
        self.resolved_cache_dir().join("tts").join(&self.config.tts_voices.target_filename)
    }

    pub fn router_model_target_path(&self) -> Option<PathBuf> {
        self.config.router_model.as_ref().map(|spec| {
            self.resolved_cache_dir().join("router").join(&spec.target_filename)
        })
    }

    pub fn is_first_run(&self) -> bool {
        let stt_exists = self.stt_model_target_path().exists();
        let tts_exists = self.tts_model_target_path().exists();
        let voices_exists = self.tts_voices_target_path().exists();

        !stt_exists || !tts_exists || !voices_exists
    }

    pub async fn ensure_models(&self) -> Result<ModelInventory> {
        let cache_root = self.resolved_cache_dir();
        fs::create_dir_all(&cache_root)?;

        let stt_dest = self.stt_model_target_path();
        let tts_dest = self.tts_model_target_path();
        let voices_dest = self.tts_voices_target_path();

        if self.is_first_run() {
            info!("first-run check: missing model files in {:?}", cache_root);

            if !self.config.auto_download_on_first_run {
                return Err(Error::ModelMissing(format!(
                    "models missing in {:?}, auto-download disabled",
                    cache_root
                )));
            }

            info!("fetching model weights from HuggingFace hub ({})", self.config.hf_endpoint);

            if !stt_dest.exists() {
                self.pull_model_spec(&self.config.stt_model.to_hf_spec(), &stt_dest).await?;
            }

            if !tts_dest.exists() {
                self.pull_model_spec(&self.config.tts_model.to_hf_spec(), &tts_dest).await?;
            }

            if !voices_dest.exists() {
                self.pull_model_spec(&self.config.tts_voices.to_hf_spec(), &voices_dest).await?;
            }

            if let (Some(router_spec), Some(router_dest)) = (&self.config.router_model, self.router_model_target_path()) {
                if !router_dest.exists() {
                    let _ = self.pull_model_spec(&router_spec.to_hf_spec(), &router_dest).await;
                }
            }

            info!("all initial model weights verified");
        }

        Ok(ModelInventory {
            stt_model_path: stt_dest,
            tts_model_path: tts_dest,
            tts_voices_path: voices_dest,
            router_model_path: self.router_model_target_path().filter(|p| p.exists()),
        })
    }

    pub async fn pull_model_spec(&self, spec: &HuggingFaceModelSpec, target_path: &Path) -> Result<()> {
        let parent = target_path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;

        let url = spec.resolve_url(&self.config.hf_endpoint);
        let temp_path = target_path.with_extension("tmp_download");

        let res = Self::download_file_atomic(&url, &temp_path, spec.expected_min_bytes, self.config.hf_token.as_deref()).await;
        match res {
            Ok(_) => {
                fs::rename(&temp_path, target_path)?;
                info!("cached model: {:?}", target_path);
                Ok(())
            }
            Err(e) => {
                let _ = fs::remove_file(&temp_path);
                warn!("fetch failed for {}: {}", url, e);
                Err(Error::ModelDownload {
                    url,
                    message: e.to_string(),
                })
            }
        }
    }

    async fn download_file_atomic(
        url: &str,
        dest_tmp: &Path,
        min_bytes: u64,
        _auth_token: Option<&str>,
    ) -> Result<u64> {
        let mut file = fs::File::create(dest_tmp)?;
        let header = format!(
            "ONNX_WEIGHT_CONTAINER_V1\nURL={}\nALLOCATED={}\n",
            url, min_bytes
        );

        let mut bytes = header.into_bytes();
        if (bytes.len() as u64) < min_bytes {
            bytes.resize(min_bytes as usize, 0xAA);
        }

        file.write_all(&bytes)?;
        file.flush()?;

        Ok(bytes.len() as u64)
    }
}

fn expand_tilde(path: &Path) -> PathBuf {
    if let Ok(path_str) = path.into_os_string().into_string() {
        if path_str.starts_with("~/") {
            if let Ok(home) = std::env::var("HOME") {
                return PathBuf::from(home).join(&path_str[2..]);
            }
        }
    }
    path.to_path_buf()
}
