//! Local vector memory store client interface.

use crate::error::Result;

pub struct MemPalaceClient {
    pub db_path: std::path::PathBuf,
}

impl MemPalaceClient {
    pub fn new(db_path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            db_path: db_path.into(),
        }
    }

    pub async fn store(&self, _text: &str) -> Result<()> {
        Ok(())
    }

    pub async fn query_recent(&self, _query: &str, _limit: usize) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}
