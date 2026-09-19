//! Error types and standard Result alias for Tycho.

use std::fmt;
use std::io;
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Json(serde_json::Error),
    Toml(toml::de::Error),
    Desktop(String),
    Audio(String),
    ModelDownload { url: String, message: String },
    ModelNotFound(PathBuf),
    ModelCorrupted { path: PathBuf, expected_bytes: u64, actual_bytes: u64 },
    Routing(String),
    Transcription(String),
    Synthesis(String),
    Inference(String),
    Config(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {}", err),
            Self::Json(err) => write!(f, "JSON serialization error: {}", err),
            Self::Toml(err) => write!(f, "TOML configuration error: {}", err),
            Self::Desktop(msg) => write!(f, "desktop error: {}", msg),
            Self::Audio(msg) => write!(f, "audio error: {}", msg),
            Self::ModelDownload { url, message } => write!(f, "model download from {} failed: {}", url, message),
            Self::ModelNotFound(path) => write!(f, "model file missing: {:?}", path),
            Self::ModelCorrupted { path, expected_bytes, actual_bytes } => {
                write!(f, "corrupt model at {:?} (expected {} bytes, got {})", path, expected_bytes, actual_bytes)
            }
            Self::Routing(msg) => write!(f, "command routing error: {}", msg),
            Self::Transcription(msg) => write!(f, "transcription error: {}", msg),
            Self::Synthesis(msg) => write!(f, "speech synthesis error: {}", msg),
            Self::Inference(msg) => write!(f, "model inference error: {}", msg),
            Self::Config(msg) => write!(f, "configuration error: {}", msg),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Json(err) => Some(err),
            Self::Toml(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Self::Json(err)
    }
}

impl From<toml::de::Error> for Error {
    fn from(err: toml::de::Error) -> Self {
        Self::Toml(err)
    }
}
