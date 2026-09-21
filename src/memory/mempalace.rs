//! Persistent memory store: an append-only JSONL file at `db_path` that
//! records timestamped entries and reads them back newest-first.

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Serialize, Deserialize)]
struct MemoryRecord {
    ts: u64,
    text: String,
}

pub struct MemPalaceClient {
    pub db_path: PathBuf,
}

impl MemPalaceClient {
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        Self {
            db_path: db_path.into(),
        }
    }

    /// Appends `text` as a timestamped JSON record, creating the parent
    /// directory and file as needed. The store is rotated when it
    /// exceeds `MAX_RECORDS`: the oldest half is dropped so the file
    /// cannot grow without bound across sessions.
    pub async fn store(&self, text: &str) -> Result<()> {
        if let Some(parent) = self.db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let record = MemoryRecord {
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            text: text.to_string(),
        };
        let mut line = serde_json::to_string(&record)?;
        line.push('\n');
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.db_path)
            .await?;
        file.write_all(line.as_bytes()).await?;
        file.flush().await?;
        self.rotate_if_needed().await
    }

    /// Hard cap on stored records — voice-assistant memory is
    /// recency-dominated, and an unbounded append log grows forever.
    const MAX_RECORDS: usize = 1000;

    /// Rewrites the store keeping the newest `MAX_RECORDS / 2` entries
    /// once it exceeds `MAX_RECORDS`. Read-modify-write is safe here:
    /// the coordinator is the only writer.
    async fn rotate_if_needed(&self) -> Result<()> {
        let content = match tokio::fs::read_to_string(&self.db_path).await {
            Ok(c) => c,
            Err(_) => return Ok(()),
        };
        let lines: Vec<&str> = content.lines().collect();
        if lines.len() <= Self::MAX_RECORDS {
            return Ok(());
        }
        let keep = &lines[lines.len() - Self::MAX_RECORDS / 2..];
        tokio::fs::write(&self.db_path, keep.join("\n") + "\n").await?;
        Ok(())
    }

    /// Relevance-ranked recall: stored texts are scored by token
    /// overlap with `query` (each distinctive query word matched counts
    /// once), ties broken by recency. Returns up to `limit` hits in
    /// chronological order — the context that a plain recency window
    /// would miss ("what did I ask you about Rust last week").
    pub async fn query_relevant(&self, query: &str, limit: usize) -> Result<Vec<String>> {
        let content = match tokio::fs::read_to_string(&self.db_path).await {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let query_terms: std::collections::HashSet<String> = query
            .to_lowercase()
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
            .filter(|w| w.len() > 2)
            .collect();
        if query_terms.is_empty() {
            return Ok(Vec::new());
        }
        let mut scored: Vec<(usize, usize, String)> = content
            .lines()
            .enumerate()
            .filter_map(|(idx, line)| {
                let rec = serde_json::from_str::<MemoryRecord>(line).ok()?;
                let lower = rec.text.to_lowercase();
                let hits = query_terms
                    .iter()
                    .filter(|t| lower.contains(t.as_str()))
                    .count();
                (hits > 0).then_some((hits, idx, rec.text))
            })
            .collect();
        // Most relevant first; recency breaks ties.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        scored.truncate(limit);
        scored.sort_by_key(|s| s.1);
        Ok(scored.into_iter().map(|(_, _, t)| t).collect())
    }

    /// Returns the most recent `limit` stored texts containing `query`
    /// (an empty query matches everything), in chronological order. A
    /// missing store reads as empty rather than erroring.
    pub async fn query_recent(&self, query: &str, limit: usize) -> Result<Vec<String>> {
        let content = match tokio::fs::read_to_string(&self.db_path).await {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut texts: Vec<String> = content
            .lines()
            .filter_map(|line| {
                serde_json::from_str::<MemoryRecord>(line)
                    .ok()
                    .map(|r| r.text)
            })
            .filter(|text| query.is_empty() || text.contains(query))
            .collect();
        if texts.len() > limit {
            texts.drain(..texts.len() - limit);
        }
        Ok(texts)
    }
}
