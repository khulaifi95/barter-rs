//! File system watcher for detecting new parquet files.
//!
//! Safety features:
//! - Ignores `.tmp` files (written atomically by collector)
//! - Verifies file mtime stability before processing
//! - Only processes files with `.parquet` extension

use crate::error::Result;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc as tokio_mpsc;
use tracing::{debug, info, warn};

/// Minimum file age in seconds before processing (mtime stability).
const DEFAULT_STABLE_MTIME_SECS: u64 = 2;

/// File watcher that emits paths of ready parquet files.
pub struct FileWatcher {
    _watcher: RecommendedWatcher,
    rx: Receiver<notify::Result<Event>>,
    stable_mtime_secs: u64,
}

impl FileWatcher {
    /// Create a new file watcher for the given directory.
    pub fn new(watch_dir: &Path, stable_mtime_secs: Option<u64>) -> Result<Self> {
        let (tx, rx) = channel();

        let mut watcher = RecommendedWatcher::new(
            move |res| {
                let _ = tx.send(res);
            },
            Config::default(),
        )?;

        watcher.watch(watch_dir, RecursiveMode::Recursive)?;

        info!("File watcher started for {:?}", watch_dir);

        Ok(Self {
            _watcher: watcher,
            rx,
            stable_mtime_secs: stable_mtime_secs.unwrap_or(DEFAULT_STABLE_MTIME_SECS),
        })
    }

    /// Wait for the next ready file, blocking.
    ///
    /// Returns `None` if the watcher is closed.
    pub fn next_ready_file(&self) -> Option<PathBuf> {
        loop {
            match self.rx.recv() {
                Ok(Ok(event)) => {
                    for path in event.paths {
                        if is_file_ready(&path, self.stable_mtime_secs) {
                            debug!("File ready: {:?}", path);
                            return Some(path);
                        }
                    }
                }
                Ok(Err(e)) => {
                    warn!("Watcher error: {}", e);
                }
                Err(_) => {
                    // Channel closed
                    return None;
                }
            }
        }
    }

    /// Try to receive the next ready file, non-blocking.
    pub fn try_next_ready_file(&self) -> Option<PathBuf> {
        while let Ok(result) = self.rx.try_recv() {
            if let Ok(event) = result {
                for path in event.paths {
                    if is_file_ready(&path, self.stable_mtime_secs) {
                        return Some(path);
                    }
                }
            }
        }
        None
    }
}

/// Async file watcher that sends paths through a tokio channel.
pub struct AsyncFileWatcher {
    /// Sender kept alive to prevent channel closure (receiver is returned to caller)
    #[allow(dead_code)]
    tx: tokio_mpsc::Sender<PathBuf>,
    _handle: std::thread::JoinHandle<()>,
}

impl AsyncFileWatcher {
    /// Create a new async file watcher.
    pub fn new(
        watch_dir: PathBuf,
        stable_mtime_secs: Option<u64>,
    ) -> Result<(Self, tokio_mpsc::Receiver<PathBuf>)> {
        let (tx, rx) = tokio_mpsc::channel(100);
        let tx_clone = tx.clone();
        let stable_secs = stable_mtime_secs.unwrap_or(DEFAULT_STABLE_MTIME_SECS);

        let handle = std::thread::spawn(move || {
            let (notify_tx, notify_rx) = channel();

            let mut watcher = match RecommendedWatcher::new(
                move |res| {
                    let _ = notify_tx.send(res);
                },
                Config::default(),
            ) {
                Ok(w) => w,
                Err(e) => {
                    tracing::error!("Failed to create watcher: {}", e);
                    return;
                }
            };

            if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::Recursive) {
                tracing::error!("Failed to watch directory: {}", e);
                return;
            }

            info!("Async file watcher started for {:?}", watch_dir);

            loop {
                match notify_rx.recv() {
                    Ok(Ok(event)) => {
                        for path in event.paths {
                            if is_file_ready(&path, stable_secs)
                                && tx_clone.blocking_send(path).is_err()
                            {
                                // Receiver dropped
                                return;
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        warn!("Watcher error: {}", e);
                    }
                    Err(_) => {
                        // Channel closed
                        return;
                    }
                }
            }
        });

        Ok((
            Self {
                tx,
                _handle: handle,
            },
            rx,
        ))
    }
}

/// Check if a file is ready to process.
///
/// A file is ready if:
/// 1. It has a `.parquet` extension
/// 2. It does not end with `.parquet.tmp` (our temp file pattern)
/// 3. Its mtime is stable (older than `stable_mtime_secs`)
pub fn is_file_ready(path: &Path, stable_mtime_secs: u64) -> bool {
    // 1. Must have .parquet extension (not .tmp)
    if path.extension() != Some(OsStr::new("parquet")) {
        return false;
    }

    // 2. File name must not contain .tmp (e.g., "foo.parquet.tmp")
    if let Some(file_name) = path.file_name()
        && file_name.to_string_lossy().contains(".tmp")
    {
        return false;
    }

    // 3. Must be a file
    if !path.is_file() {
        return false;
    }

    // 4. Must have stable mtime
    if let Ok(metadata) = std::fs::metadata(path)
        && let Ok(modified) = metadata.modified()
    {
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default();
        if age < Duration::from_secs(stable_mtime_secs) {
            debug!("File {:?} too fresh ({:?}), skipping", path, age);
            return false;
        }
    }

    true
}

/// Scan a directory for ready parquet files.
///
/// Returns files sorted by modification time (oldest first).
pub fn scan_ready_files(dir: &Path, stable_mtime_secs: u64) -> Vec<PathBuf> {
    let mut files = Vec::new();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Recurse into subdirectories
                files.extend(scan_ready_files(&path, stable_mtime_secs));
            } else if is_file_ready(&path, stable_mtime_secs) {
                files.push(path);
            }
        }
    }

    // Sort by modification time
    files.sort_by(|a, b| {
        let a_time = std::fs::metadata(a)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let b_time = std::fs::metadata(b)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        a_time.cmp(&b_time)
    });

    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_is_file_ready_parquet() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.parquet");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"test content").unwrap();
        drop(f); // Close file

        // File is too fresh initially (5 second threshold)
        assert!(!is_file_ready(&path, 5));

        // With 0 second threshold, file should be ready
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            is_file_ready(&path, 0),
            "File should be ready with 0 threshold"
        );
    }

    #[test]
    fn test_watcher_ignores_tmp_files() {
        let dir = tempdir().unwrap();

        // Create a .tmp file
        let tmp_path = dir.path().join("test.parquet.tmp");
        File::create(&tmp_path).unwrap();

        // Should not be ready
        assert!(!is_file_ready(&tmp_path, 0));
    }

    #[test]
    fn test_watcher_ignores_non_parquet() {
        let dir = tempdir().unwrap();

        // Create a non-parquet file
        let txt_path = dir.path().join("test.txt");
        File::create(&txt_path).unwrap();

        assert!(!is_file_ready(&txt_path, 0));
    }

    #[test]
    fn test_scan_ready_files() {
        let dir = tempdir().unwrap();

        // Create some files
        let parquet_path = dir.path().join("test.parquet");
        let tmp_path = dir.path().join("test.parquet.tmp");
        let txt_path = dir.path().join("test.txt");

        let mut f1 = File::create(&parquet_path).unwrap();
        f1.write_all(b"parquet content").unwrap();
        drop(f1);
        let mut f2 = File::create(&tmp_path).unwrap();
        f2.write_all(b"tmp content").unwrap();
        drop(f2);
        let mut f3 = File::create(&txt_path).unwrap();
        f3.write_all(b"txt content").unwrap();
        drop(f3);

        // Wait for file system to settle
        std::thread::sleep(std::time::Duration::from_millis(50));

        let files = scan_ready_files(dir.path(), 0);

        // Should only find the .parquet file
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("test.parquet"));
    }

    #[test]
    fn test_partial_file_ignored() {
        let dir = tempdir().unwrap();

        // Create partial file
        let tmp_path = dir.path().join("test.parquet.tmp");
        let mut f = File::create(&tmp_path).unwrap();
        f.write_all(b"partial data").unwrap();
        drop(f);

        // Create complete file
        let complete_path = dir.path().join("complete.parquet");
        let mut f2 = File::create(&complete_path).unwrap();
        f2.write_all(b"complete data").unwrap();
        drop(f2);

        // Wait for file system to settle
        std::thread::sleep(std::time::Duration::from_millis(50));

        let files = scan_ready_files(dir.path(), 0);

        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("complete.parquet"));
    }
}
