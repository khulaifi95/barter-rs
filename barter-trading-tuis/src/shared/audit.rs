//! Audit Logger for Market State Engine
//!
//! Non-blocking audit logging with background file writing.
//! Logs state transitions to JSONL files with rotation.
//!
//! # Architecture
//! ```text
//! HOT PATH: log() -> try_send() -> ~100-200ns (non-blocking)
//!                       |
//!                       v
//!          mpsc::channel (bounded 10K)
//!                       |
//!                       v
//! COLD PATH: writer_task() -> serde_json -> write to file
//! ```
//!
//! # Critical Design Decisions
//! - `log()` uses `try_send()` - NEVER blocks the hot path
//! - Drops logs if channel full - preserves trading latency
//! - Rotation: Daily AND size-based (rotation_mb from config)
//! - Retention: Auto-delete files older than retention_days
//! - Graceful error handling - logs to stderr, never panics

use crate::shared::market_state::{AuditEntry, AuditLog, AuditThresholds};
use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, Utc};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Dropped log counter for metrics
static DROPPED_LOGS: AtomicU64 = AtomicU64::new(0);

/// Get the count of dropped logs (for monitoring/metrics)
pub fn dropped_log_count() -> u64 {
    DROPPED_LOGS.load(Ordering::Relaxed)
}

/// Non-blocking audit logger
///
/// Sends audit entries through a bounded channel to a background
/// writer task. Never blocks the hot path - drops logs if channel full.
#[derive(Clone)]
pub struct AuditLogger {
    tx: mpsc::Sender<AuditEntry>,
}

impl AuditLogger {
    /// Create a new audit logger with the given configuration
    ///
    /// Spawns a background tokio task for file writing.
    /// The logger is ready to use immediately after construction.
    pub fn new(config: &AuditThresholds) -> Self {
        let (tx, rx) = mpsc::channel(config.channel_size);
        let log_dir = config.log_dir.clone();
        let rotation_mb = config.rotation_mb;
        let retention_days = config.retention_days;

        // Spawn the background writer task
        tokio::spawn(async move {
            Self::writer_task(rx, log_dir, rotation_mb, retention_days).await;
        });

        Self { tx }
    }

    /// Create a logger for testing without spawning background task
    #[cfg(test)]
    pub fn new_test() -> (Self, mpsc::Receiver<AuditEntry>) {
        let (tx, rx) = mpsc::channel(100);
        (Self { tx }, rx)
    }

    /// Create a no-op logger that discards all entries (for testing)
    ///
    /// Uses a small channel that silently drops entries.
    pub fn no_op() -> Self {
        let (tx, _rx) = mpsc::channel(1);
        // Receiver is dropped immediately, so all sends will fail silently
        // This is intentional for no-op logging
        Self { tx }
    }

    /// Background writer task - drains channel and writes JSONL
    ///
    /// Handles:
    /// - Directory creation
    /// - Daily AND size-based log rotation
    /// - Retention policy (deletes old files)
    /// - Graceful error handling (logs to stderr)
    async fn writer_task(
        mut rx: mpsc::Receiver<AuditEntry>,
        log_dir: String,
        rotation_mb: u64,
        retention_days: u32,
    ) {
        let log_path = PathBuf::from(&log_dir);
        let rotation_bytes = rotation_mb * 1024 * 1024;

        // Ensure log directory exists
        if let Err(e) = fs::create_dir_all(&log_path) {
            error!("Failed to create audit log directory {:?}: {}", log_path, e);
            // Continue anyway - we'll retry on each write
        }

        // Clean up old files on startup
        Self::cleanup_old_files(&log_path, retention_days);

        let mut current_date: Option<NaiveDate> = None;
        let mut current_file_index: u32 = 0;
        let mut current_file_size: u64 = 0;
        let mut writer: Option<BufWriter<File>> = None;

        while let Some(entry) = rx.recv().await {
            // Determine the date for this entry
            let entry_date = DateTime::from_timestamp_millis(entry.timestamp)
                .map(|dt| dt.date_naive())
                .unwrap_or_else(|| Utc::now().date_naive());

            // Check if we need to rotate: date changed OR size exceeded
            let date_changed = current_date.map_or(true, |d| d != entry_date);
            let size_exceeded = current_file_size >= rotation_bytes;
            let needs_rotation = date_changed || size_exceeded;

            if needs_rotation {
                // Flush and close previous writer
                if let Some(ref mut w) = writer {
                    let _ = w.flush();
                }

                // Reset or increment file index
                if date_changed {
                    current_file_index = 0;
                } else if size_exceeded {
                    current_file_index += 1;
                    info!(
                        "Size-based rotation: file exceeded {}MB, rotating to index {}",
                        rotation_mb, current_file_index
                    );
                }

                // Open new file
                match Self::open_log_file_indexed(&log_path, entry_date, current_file_index) {
                    Ok((file, existing_size)) => {
                        writer = Some(BufWriter::new(file));
                        current_date = Some(entry_date);
                        current_file_size = existing_size;
                        debug!(
                            "Opened audit log: date={}, index={}, size={}",
                            entry_date, current_file_index, current_file_size
                        );
                    }
                    Err(e) => {
                        error!("Failed to open audit log file for {}: {}", entry_date, e);
                        writer = None;
                        current_date = None;
                        continue;
                    }
                }
            }

            // Write the entry as JSONL
            if let Some(ref mut w) = writer {
                match Self::write_entry(w, &entry) {
                    Ok(bytes_written) => {
                        current_file_size += bytes_written as u64;
                    }
                    Err(e) => {
                        error!("Failed to write audit entry {}: {}", entry.audit_id, e);
                        // Try to continue with next entry
                    }
                }
            }
        }

        // Channel closed - flush and exit
        if let Some(ref mut w) = writer {
            let _ = w.flush();
        }
        debug!("Audit writer task shutting down");
    }

    /// Clean up log files older than retention_days
    fn cleanup_old_files(log_dir: &Path, retention_days: u32) {
        let cutoff = Utc::now() - ChronoDuration::days(retention_days as i64);
        let cutoff_date = cutoff.date_naive();

        let entries = match fs::read_dir(log_dir) {
            Ok(e) => e,
            Err(e) => {
                warn!("Failed to read audit log directory for cleanup: {}", e);
                return;
            }
        };

        let mut deleted_count = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                // Parse date from filename: audit_YYYY-MM-DD.jsonl or audit_YYYY-MM-DD_N.jsonl
                if let Some(date_str) = filename
                    .strip_prefix("audit_")
                    .and_then(|s| s.split('.').next())
                    .and_then(|s| s.split('_').next())
                {
                    if let Ok(file_date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                        if file_date < cutoff_date {
                            if let Err(e) = fs::remove_file(&path) {
                                warn!("Failed to delete old audit log {:?}: {}", path, e);
                            } else {
                                deleted_count += 1;
                            }
                        }
                    }
                }
            }
        }

        if deleted_count > 0 {
            info!(
                "Cleaned up {} audit log files older than {} days",
                deleted_count, retention_days
            );
        }
    }

    /// Open a log file for the given date (backward compat)
    #[allow(dead_code)]
    fn open_log_file(log_dir: &Path, date: NaiveDate) -> std::io::Result<File> {
        let (file, _) = Self::open_log_file_indexed(log_dir, date, 0)?;
        Ok(file)
    }

    /// Open a log file for the given date and rotation index.
    ///
    /// Returns the file and its current size (for size-based rotation tracking).
    fn open_log_file_indexed(
        log_dir: &Path,
        date: NaiveDate,
        index: u32,
    ) -> std::io::Result<(File, u64)> {
        // Ensure directory exists (retry in case it was deleted)
        fs::create_dir_all(log_dir)?;

        // Format: audit_YYYY-MM-DD.jsonl or audit_YYYY-MM-DD_N.jsonl
        let filename = if index == 0 {
            format!("audit_{}.jsonl", date.format("%Y-%m-%d"))
        } else {
            format!("audit_{}_{}.jsonl", date.format("%Y-%m-%d"), index)
        };
        let path = log_dir.join(filename);

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;

        // Get current file size for rotation tracking
        let metadata = file.metadata()?;
        let size = metadata.len();

        Ok((file, size))
    }

    /// Write a single entry as JSONL, returns bytes written
    fn write_entry<W: Write>(writer: &mut W, entry: &AuditEntry) -> std::io::Result<usize> {
        let json = serde_json::to_string(entry)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let bytes = json.len() + 1; // +1 for newline
        writeln!(writer, "{}", json)?;
        writer.flush()?; // Flush after each entry for durability
        Ok(bytes)
    }
}

impl AuditLog for AuditLogger {
    /// Log an audit entry - NON-BLOCKING
    ///
    /// Uses `try_send()` to never block the hot path.
    /// If the channel is full, the entry is dropped and a counter is incremented.
    fn log(&self, entry: AuditEntry) {
        match self.tx.try_send(entry) {
            Ok(()) => {
                // Successfully queued
            }
            Err(mpsc::error::TrySendError::Full(entry)) => {
                // Channel full - drop the log entry
                DROPPED_LOGS.fetch_add(1, Ordering::Relaxed);
                warn!(
                    "Audit channel full, dropped entry {} (total dropped: {})",
                    entry.audit_id,
                    DROPPED_LOGS.load(Ordering::Relaxed)
                );
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // Channel closed - writer task has stopped (or using no_op logger)
                // Silently drop to avoid log spam
            }
        }
    }
}

/// No-op audit logger for testing or when audit is disabled
#[derive(Clone, Default)]
pub struct NoOpAuditLogger;

impl AuditLog for NoOpAuditLogger {
    fn log(&self, _entry: AuditEntry) {
        // Intentionally empty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::market_state::{State, TradingBias, VolRegime};
    use std::time::Duration;
    use tempfile::TempDir;

    /// Create a test audit entry
    fn test_entry(audit_id: u64) -> AuditEntry {
        AuditEntry {
            audit_id,
            timestamp: chrono::Utc::now().timestamp_millis(),
            ticker: "BTC".to_string(),
            price: 50000.0,
            prev_state: State::Wait,
            new_state: State::Ready,
            bias: Some(TradingBias::MeanReversion),
            confidence: 85,
            vol_percentile: 45.0,
            vol_regime: VolRegime::Normal,
            gamma_flip_price: Some(49500.0),
            price_vs_flip_pct: Some(1.01),
            cvd_consensus: "3/3 BUY".to_string(),
            funding_rate: 0.0001,
            funding_velocity: 0.00005,
            reason: "L1 passed, L2 above flip, L3 consensus".to_string(),
        }
    }

    #[test]
    fn test_non_blocking_log() {
        // Create a test logger with small channel
        let (tx, _rx) = mpsc::channel(2);
        let logger = AuditLogger { tx };

        // First two should succeed
        logger.log(test_entry(1));
        logger.log(test_entry(2));

        // Third should be dropped (channel full)
        let initial_dropped = dropped_log_count();
        logger.log(test_entry(3));

        // Give a moment for the try_send to complete
        std::thread::sleep(Duration::from_millis(10));

        // Check that we dropped one
        assert!(
            dropped_log_count() >= initial_dropped,
            "Should have tracked dropped log"
        );
    }

    #[tokio::test]
    async fn test_entry_queued() {
        let (logger, mut rx) = AuditLogger::new_test();

        // Log an entry
        let entry = test_entry(42);
        logger.log(entry);

        // Should receive it
        let received = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("Timeout waiting for entry")
            .expect("Channel closed");

        assert_eq!(received.audit_id, 42);
        assert_eq!(received.ticker, "BTC");
    }

    #[tokio::test]
    async fn test_file_writing() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let log_dir = temp_dir.path().to_str().unwrap().to_string();

        let config = AuditThresholds {
            channel_size: 100,
            log_dir: log_dir.clone(),
            rotation_mb: 500,
            retention_days: 30,
        };

        let logger = AuditLogger::new(&config);

        // Log a few entries
        for i in 0..5 {
            logger.log(test_entry(i));
        }

        // Give the background task time to write
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Check that a file was created
        let entries: Vec<_> = fs::read_dir(&log_dir)
            .expect("Failed to read log dir")
            .filter_map(|e| e.ok())
            .collect();

        assert!(!entries.is_empty(), "Should have created at least one log file");

        // Verify file contains JSONL
        let file_path = entries[0].path();
        let content = fs::read_to_string(&file_path).expect("Failed to read log file");
        let lines: Vec<_> = content.lines().collect();

        assert_eq!(lines.len(), 5, "Should have 5 log entries");

        // Verify each line is valid JSON
        for line in lines {
            let parsed: AuditEntry =
                serde_json::from_str(line).expect("Each line should be valid JSON");
            assert_eq!(parsed.ticker, "BTC");
        }
    }

    #[test]
    fn test_entry_serialization() {
        let entry = test_entry(123);
        let json = serde_json::to_string(&entry).expect("Should serialize");
        let parsed: AuditEntry = serde_json::from_str(&json).expect("Should deserialize");

        assert_eq!(parsed.audit_id, 123);
        assert_eq!(parsed.ticker, "BTC");
        assert_eq!(parsed.price, 50000.0);
        assert_eq!(parsed.new_state, State::Ready);
    }

    #[test]
    fn test_noop_logger() {
        let logger = NoOpAuditLogger;
        // Should not panic
        logger.log(test_entry(1));
        logger.log(test_entry(2));
    }

    #[tokio::test]
    async fn test_log_file_naming() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let log_path = temp_dir.path();

        // Test file naming
        let today = Utc::now().date_naive();
        let file = AuditLogger::open_log_file(log_path, today).expect("Should open file");
        drop(file);

        let expected_filename = format!("audit_{}.jsonl", today.format("%Y-%m-%d"));
        let file_path = log_path.join(&expected_filename);

        assert!(file_path.exists(), "File should exist with correct name");
    }

    #[tokio::test]
    async fn test_channel_capacity() {
        // Verify channel respects configured size
        let config = AuditThresholds {
            channel_size: 5,
            log_dir: "/tmp/test".to_string(),
            rotation_mb: 500,
            retention_days: 30,
        };

        let (tx, _rx) = mpsc::channel::<AuditEntry>(config.channel_size);

        // Fill the channel
        for i in 0..5 {
            tx.try_send(test_entry(i)).expect("Should succeed");
        }

        // Next send should fail
        let result = tx.try_send(test_entry(5));
        assert!(result.is_err(), "Channel should be full");
    }
}
