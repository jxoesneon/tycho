//! HuggingFace Hub client and model resolver.

use serde::{Deserialize, Serialize};

/// Specification of an ONNX model or asset hosted on HuggingFace Hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HuggingFaceModelSpec {
    /// Repository ID on HuggingFace, e.g. "onnx-community/whisper-tiny.en"
    pub repo_id: String,
    /// Git revision, branch, or tag (defaults to "main")
    pub revision: String,
    /// Path of the file inside the HuggingFace repository, e.g. "onnx/model.onnx"
    pub filename: String,
    /// Local filename when stored in cache directory, e.g. "whisper-tiny.en.onnx"
    pub target_filename: String,
    /// Expected minimum size in bytes for basic sanity check
    pub expected_min_bytes: u64,
    /// Optional subfolder inside repository
    pub subfolder: Option<String>,
}

impl HuggingFaceModelSpec {
    pub fn new(
        repo_id: impl Into<String>,
        filename: impl Into<String>,
        target_filename: impl Into<String>,
    ) -> Self {
        Self {
            repo_id: repo_id.into(),
            revision: "main".to_string(),
            filename: filename.into(),
            target_filename: target_filename.into(),
            expected_min_bytes: 1024,
            subfolder: None,
        }
    }

    pub fn with_revision(mut self, revision: impl Into<String>) -> Self {
        self.revision = revision.into();
        self
    }

    pub fn with_min_bytes(mut self, min_bytes: u64) -> Self {
        self.expected_min_bytes = min_bytes;
        self
    }

    pub fn with_subfolder(mut self, subfolder: impl Into<String>) -> Self {
        self.subfolder = Some(subfolder.into());
        self
    }

    /// Resolves the direct download URL on HuggingFace Hub.
    /// URL format: https://huggingface.co/{repo_id}/resolve/{revision}/{filename}
    pub fn resolve_url(&self, endpoint: &str) -> String {
        let base = endpoint.trim_end_matches('/');
        if let Some(ref sub) = self.subfolder {
            format!(
                "{}/{}/resolve/{}/{}/{}",
                base,
                self.repo_id,
                self.revision,
                sub.trim_matches('/'),
                self.filename
            )
        } else {
            format!(
                "{}/{}/resolve/{}/{}",
                base, self.repo_id, self.revision, self.filename
            )
        }
    }
}
