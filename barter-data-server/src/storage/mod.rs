//! Storage abstraction for Parquet files.
//!
//! Provides a unified interface for local and S3 storage backends.

pub mod local;
pub mod s3;

use async_trait::async_trait;
use thiserror::Error;

/// Errors that can occur during storage operations.
#[derive(Error, Debug)]
pub enum StorageError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("S3 error: {0}")]
    S3(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Not found: {0}")]
    NotFound(String),
}

/// Trait for storage backends (local filesystem, S3, etc.).
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Write data to a path.
    async fn write(&self, path: &str, data: &[u8]) -> Result<(), StorageError>;

    /// Check if a path exists.
    async fn exists(&self, path: &str) -> Result<bool, StorageError>;

    /// Read data from a path.
    async fn read(&self, path: &str) -> Result<Vec<u8>, StorageError>;

    /// List files in a directory/prefix.
    async fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError>;

    /// Delete a file.
    async fn delete(&self, path: &str) -> Result<(), StorageError>;
}

/// Storage configuration from environment.
#[derive(Debug, Clone)]
pub struct StorageConfig {
    /// Use S3 storage
    pub s3_enabled: bool,
    /// S3 bucket name
    pub s3_bucket: Option<String>,
    /// S3 region
    pub s3_region: Option<String>,
    /// Local base directory
    pub local_dir: String,
}

impl StorageConfig {
    /// Load configuration from environment variables.
    pub fn from_env() -> Self {
        let s3_enabled = std::env::var("S3_ENABLED")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false);

        let s3_bucket = std::env::var("S3_BUCKET").ok();
        let s3_region = std::env::var("S3_REGION").ok();

        let local_dir = std::env::var("PARQUET_OUTPUT_DIR")
            .unwrap_or_else(|_| "/data/parquet".to_string());

        Self {
            s3_enabled,
            s3_bucket,
            s3_region,
            local_dir,
        }
    }
}

pub use local::LocalStorage;
pub use s3::S3Storage;
