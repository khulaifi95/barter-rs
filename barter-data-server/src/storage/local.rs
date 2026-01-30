//! Local filesystem storage backend.

use super::{StorageBackend, StorageError};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::debug;

/// Local filesystem storage.
#[allow(dead_code)]
pub struct LocalStorage {
    base_dir: PathBuf,
}

impl LocalStorage {
    /// Create a new local storage backend.
    #[allow(dead_code)]
    pub fn new<P: AsRef<Path>>(base_dir: P) -> Self {
        Self {
            base_dir: base_dir.as_ref().to_path_buf(),
        }
    }

    /// Get the full path for a relative path.
    #[allow(dead_code)]
    fn full_path(&self, path: &str) -> PathBuf {
        self.base_dir.join(path)
    }
}

#[async_trait]
impl StorageBackend for LocalStorage {
    async fn write(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        let full_path = self.full_path(path);

        // Ensure parent directory exists
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        fs::write(&full_path, data).await?;
        debug!("Wrote {} bytes to {:?}", data.len(), full_path);
        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        let full_path = self.full_path(path);
        Ok(full_path.exists())
    }

    async fn read(&self, path: &str) -> Result<Vec<u8>, StorageError> {
        let full_path = self.full_path(path);
        if !full_path.exists() {
            return Err(StorageError::NotFound(path.to_string()));
        }
        Ok(fs::read(&full_path).await?)
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        let full_path = self.full_path(prefix);
        if !full_path.exists() {
            return Ok(Vec::new());
        }

        let mut entries = Vec::new();
        let mut read_dir = fs::read_dir(&full_path).await?;

        while let Some(entry) = read_dir.next_entry().await? {
            if let Ok(name) = entry.file_name().into_string() {
                entries.push(name);
            }
        }

        Ok(entries)
    }

    async fn delete(&self, path: &str) -> Result<(), StorageError> {
        let full_path = self.full_path(path);
        if full_path.exists() {
            fs::remove_file(&full_path).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_write_and_read() {
        let temp_dir = TempDir::new().unwrap();
        let storage = LocalStorage::new(temp_dir.path());

        let data = b"test data";
        storage.write("test/file.txt", data).await.unwrap();

        let read_data = storage.read("test/file.txt").await.unwrap();
        assert_eq!(read_data, data);
    }

    #[tokio::test]
    async fn test_exists() {
        let temp_dir = TempDir::new().unwrap();
        let storage = LocalStorage::new(temp_dir.path());

        assert!(!storage.exists("nonexistent.txt").await.unwrap());

        storage.write("exists.txt", b"data").await.unwrap();
        assert!(storage.exists("exists.txt").await.unwrap());
    }

    #[tokio::test]
    async fn test_list() {
        let temp_dir = TempDir::new().unwrap();
        let storage = LocalStorage::new(temp_dir.path());

        storage.write("dir/file1.txt", b"1").await.unwrap();
        storage.write("dir/file2.txt", b"2").await.unwrap();

        let files = storage.list("dir").await.unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.contains(&"file1.txt".to_string()));
        assert!(files.contains(&"file2.txt".to_string()));
    }

    #[tokio::test]
    async fn test_delete() {
        let temp_dir = TempDir::new().unwrap();
        let storage = LocalStorage::new(temp_dir.path());

        storage.write("to_delete.txt", b"data").await.unwrap();
        assert!(storage.exists("to_delete.txt").await.unwrap());

        storage.delete("to_delete.txt").await.unwrap();
        assert!(!storage.exists("to_delete.txt").await.unwrap());
    }

    #[tokio::test]
    async fn test_read_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let storage = LocalStorage::new(temp_dir.path());

        let result = storage.read("nonexistent.txt").await;
        assert!(matches!(result, Err(StorageError::NotFound(_))));
    }
}
