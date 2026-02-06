//! Error types for barter-features.

use std::path::PathBuf;
use thiserror::Error;

/// Main error type for the feature computation layer.
#[derive(Error, Debug)]
pub enum FeatureError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    #[error("Parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),

    #[error("Config error: {0}")]
    Config(String),

    #[error("Invalid precision: expected {expected} bytes, got {actual}")]
    InvalidPrecision { expected: usize, actual: usize },

    #[error("Missing column: {0}")]
    MissingColumn(String),

    #[error("Invalid data: {0}")]
    InvalidData(String),

    #[error("Checkpoint error: {0}")]
    Checkpoint(String),

    #[error("Watcher error: {0}")]
    Watcher(String),

    #[error("File not ready: {path:?} - {reason}")]
    FileNotReady { path: PathBuf, reason: String },

    #[error("Gap detected: {missing_bars} bars missing from {start} to {end}")]
    GapDetected {
        start: i64,
        end: i64,
        missing_bars: u32,
    },

    #[error("Session error: {0}")]
    Session(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("Notify error: {0}")]
    Notify(#[from] notify::Error),
}

/// Result type alias for feature operations.
pub type Result<T> = std::result::Result<T, FeatureError>;
