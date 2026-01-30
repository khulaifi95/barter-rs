//! AWS S3 storage backend with retry logic.

use super::{StorageBackend, StorageError};
use async_trait::async_trait;
use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::ByteStream;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, warn};

/// Maximum number of retry attempts for S3 operations.
const MAX_RETRIES: u32 = 3;
/// Base delay between retries (exponential backoff).
const RETRY_BASE_DELAY_MS: u64 = 100;

/// S3 storage backend.
pub struct S3Storage {
    client: Client,
    bucket: String,
}

impl S3Storage {
    /// Create a new S3 storage backend.
    pub async fn new(bucket: String, region: Option<String>) -> Result<Self, StorageError> {
        let config = if let Some(region) = region {
            aws_config::defaults(aws_config::BehaviorVersion::latest())
                .region(aws_sdk_s3::config::Region::new(region))
                .load()
                .await
        } else {
            aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await
        };

        let client = Client::new(&config);

        Ok(Self { client, bucket })
    }

    /// Create from environment variables.
    pub async fn from_env() -> Result<Self, StorageError> {
        let bucket = std::env::var("S3_BUCKET")
            .map_err(|_| StorageError::Config("S3_BUCKET not set".to_string()))?;
        let region = std::env::var("S3_REGION").ok();
        Self::new(bucket, region).await
    }

    /// Execute an operation with retry logic.
    async fn with_retry<T, F, Fut>(&self, operation: F) -> Result<T, StorageError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T, StorageError>>,
    {
        let mut attempts = 0;
        loop {
            attempts += 1;
            match operation().await {
                Ok(result) => return Ok(result),
                Err(e) if attempts < MAX_RETRIES => {
                    let delay = RETRY_BASE_DELAY_MS * 2u64.pow(attempts - 1);
                    warn!("S3 operation failed (attempt {}), retrying in {}ms: {}", attempts, delay, e);
                    sleep(Duration::from_millis(delay)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

#[async_trait]
impl StorageBackend for S3Storage {
    async fn write(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        let bucket = self.bucket.clone();
        let key = path.to_string();
        let data_vec = data.to_vec();

        self.with_retry(|| {
            let bucket = bucket.clone();
            let key = key.clone();
            let data_vec = data_vec.clone();
            async move {
                let body = ByteStream::from(data_vec.clone());
                self.client
                    .put_object()
                    .bucket(&bucket)
                    .key(&key)
                    .body(body)
                    .send()
                    .await
                    .map_err(|e| StorageError::S3(e.to_string()))?;
                debug!("Wrote {} bytes to s3://{}/{}", data_vec.len(), bucket, key);
                Ok(())
            }
        })
        .await
    }

    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        let result = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(path)
            .send()
            .await;

        match result {
            Ok(_) => Ok(true),
            Err(e) => {
                let service_error = e.into_service_error();
                if service_error.is_not_found() {
                    Ok(false)
                } else {
                    Err(StorageError::S3(service_error.to_string()))
                }
            }
        }
    }

    async fn read(&self, path: &str) -> Result<Vec<u8>, StorageError> {
        let result = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(path)
            .send()
            .await
            .map_err(|e| {
                let service_error = e.into_service_error();
                if service_error.is_no_such_key() {
                    StorageError::NotFound(path.to_string())
                } else {
                    StorageError::S3(service_error.to_string())
                }
            })?;

        let data = result
            .body
            .collect()
            .await
            .map_err(|e| StorageError::S3(e.to_string()))?
            .into_bytes()
            .to_vec();

        Ok(data)
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        let result = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(prefix)
            .send()
            .await
            .map_err(|e| StorageError::S3(e.into_service_error().to_string()))?;

        let keys = result
            .contents()
            .iter()
            .filter_map(|obj| obj.key().map(|k| k.to_string()))
            .collect();

        Ok(keys)
    }

    async fn delete(&self, path: &str) -> Result<(), StorageError> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(path)
            .send()
            .await
            .map_err(|e| StorageError::S3(e.into_service_error().to_string()))?;
        Ok(())
    }
}

// Note: S3 tests require actual AWS credentials and are typically run manually
// or in CI with appropriate setup.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_error_display() {
        let err = StorageError::S3("test error".to_string());
        assert!(err.to_string().contains("test error"));
    }

    #[test]
    fn test_storage_error_not_found() {
        let err = StorageError::NotFound("missing.txt".to_string());
        assert!(err.to_string().contains("missing.txt"));
    }
}
