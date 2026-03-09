//! Checkpoint system for tracking processed files and enabling idempotent restarts.
//!
//! Features:
//! - Fast (mtime, size) fingerprinting for watch-mode deduplication
//! - SHA256 hashing available for batch-mode verification
//! - JSON persistence
//! - Skip already-processed files on restart
//! - Detect changed files for reprocessing

use crate::error::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use tracing::{debug, info, warn};

/// Main checkpoint structure persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Checkpoint format version
    pub version: String,
    /// Hash of the configuration used
    pub config_hash: String,
    /// Per-instrument checkpoint data
    pub instruments: HashMap<String, InstrumentCheckpoint>,
    /// Last save timestamp
    pub last_saved: DateTime<Utc>,
}

/// Checkpoint data for a single instrument.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentCheckpoint {
    /// Map of file path -> checkpoint info
    pub processed_files: HashMap<String, FileCheckpoint>,
    /// Current session state (for resuming mid-session)
    pub session_state: Option<SessionState>,
}

/// Checkpoint info for a single processed file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileCheckpoint {
    /// Legacy field kept for backward compatibility with existing checkpoint JSON.
    /// No longer computed — checks use (size, mtime) instead.
    #[serde(default)]
    pub file_hash: String,
    /// File size in bytes at processing time
    #[serde(default)]
    pub file_size: u64,
    /// File mtime as seconds since epoch
    #[serde(default)]
    pub file_mtime_secs: i64,
    /// Number of rows processed from this file
    pub rows_processed: u64,
    /// Last ts_event processed
    pub last_ts_event: i64,
    /// When this file was processed
    pub processed_at: DateTime<Utc>,
}

/// Session state for resuming mid-session processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub session_id: String,
    pub session_date: String,
    pub last_bracket_index: u8,
    pub running_volume: i64,
    pub running_poc: i64,
    pub running_vah: i64,
    pub running_val: i64,
}

impl Default for Checkpoint {
    fn default() -> Self {
        Self {
            version: "1.0.0".to_string(),
            config_hash: String::new(),
            instruments: HashMap::new(),
            last_saved: Utc::now(),
        }
    }
}

impl Checkpoint {
    /// Load checkpoint from a file, or create a new one if it doesn't exist.
    pub fn load_or_create(path: &Path, config_hash: &str) -> Result<Self> {
        if path.exists() {
            let file = File::open(path)?;
            let reader = BufReader::new(file);
            let mut checkpoint: Checkpoint = serde_json::from_reader(reader)?;

            // Check if config has changed
            if checkpoint.config_hash != config_hash {
                warn!(
                    "Config hash changed ({} -> {}), clearing checkpoint",
                    checkpoint.config_hash, config_hash
                );
                checkpoint = Self::new(config_hash);
            }

            info!(
                "Loaded checkpoint with {} instruments",
                checkpoint.instruments.len()
            );
            Ok(checkpoint)
        } else {
            info!("Creating new checkpoint");
            Ok(Self::new(config_hash))
        }
    }

    /// Create a new checkpoint with the given config hash.
    pub fn new(config_hash: &str) -> Self {
        Self {
            version: "1.0.0".to_string(),
            config_hash: config_hash.to_string(),
            instruments: HashMap::new(),
            last_saved: Utc::now(),
        }
    }

    /// Save checkpoint to a file atomically (using .tmp + rename).
    pub fn save(&mut self, path: &Path) -> Result<()> {
        self.last_saved = Utc::now();

        let tmp_path = path.with_extension("json.tmp");

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Write to temp file
        let file = File::create(&tmp_path)?;
        serde_json::to_writer_pretty(file, self)?;

        // Atomic rename
        std::fs::rename(&tmp_path, path)?;

        debug!("Saved checkpoint to {:?}", path);
        Ok(())
    }

    /// Check if a file has already been processed using fast (size, mtime) fingerprint.
    ///
    /// Falls back to path-only check for legacy checkpoints that lack size/mtime fields.
    /// Does NOT re-hash the file — this is the hot path called on every rescan tick.
    pub fn is_file_processed(&self, instrument_id: &str, file_path: &Path) -> bool {
        let file_key = file_path.to_string_lossy().to_string();

        if let Some(instr) = self.instruments.get(instrument_id)
            && let Some(checkpoint) = instr.processed_files.get(&file_key)
        {
            // Fast path: compare (size, mtime) — no I/O beyond stat()
            if checkpoint.file_size > 0
                && let Ok(meta) = std::fs::metadata(file_path)
            {
                let current_size = meta.len();
                let current_mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                return checkpoint.file_size == current_size
                    && checkpoint.file_mtime_secs == current_mtime;
            }
            // Legacy checkpoint without size/mtime — treat as processed
            return true;
        }
        false
    }

    /// Check if a file needs reprocessing (exists in checkpoint but fingerprint differs).
    pub fn needs_reprocessing(&self, instrument_id: &str, file_path: &Path) -> bool {
        let file_key = file_path.to_string_lossy().to_string();

        if let Some(instr) = self.instruments.get(instrument_id)
            && let Some(checkpoint) = instr.processed_files.get(&file_key)
        {
            if checkpoint.file_size > 0
                && let Ok(meta) = std::fs::metadata(file_path)
            {
                let current_size = meta.len();
                let current_mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                return checkpoint.file_size != current_size
                    || checkpoint.file_mtime_secs != current_mtime;
            }
            return false; // Legacy: assume unchanged
        }
        false
    }

    /// Mark a file as processed, recording (size, mtime) for fast future checks.
    pub fn mark_processed(
        &mut self,
        instrument_id: &str,
        file_path: &Path,
        rows_processed: u64,
        last_ts_event: i64,
    ) -> Result<()> {
        let file_key = file_path.to_string_lossy().to_string();

        // Read file metadata for fast fingerprinting
        let (file_size, file_mtime_secs) = std::fs::metadata(file_path)
            .map(|meta| {
                let size = meta.len();
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                (size, mtime)
            })
            .unwrap_or((0, 0));

        let instr = self
            .instruments
            .entry(instrument_id.to_string())
            .or_insert_with(|| InstrumentCheckpoint {
                processed_files: HashMap::new(),
                session_state: None,
            });

        instr.processed_files.insert(
            file_key,
            FileCheckpoint {
                file_hash: String::new(),
                file_size,
                file_mtime_secs,
                rows_processed,
                last_ts_event,
                processed_at: Utc::now(),
            },
        );

        Ok(())
    }

    /// Update session state for an instrument.
    pub fn update_session_state(&mut self, instrument_id: &str, state: SessionState) {
        let instr = self
            .instruments
            .entry(instrument_id.to_string())
            .or_insert_with(|| InstrumentCheckpoint {
                processed_files: HashMap::new(),
                session_state: None,
            });

        instr.session_state = Some(state);
    }

    /// Get session state for an instrument.
    pub fn get_session_state(&self, instrument_id: &str) -> Option<&SessionState> {
        self.instruments
            .get(instrument_id)
            .and_then(|i| i.session_state.as_ref())
    }

    /// Clear session state for an instrument (e.g., at session end).
    pub fn clear_session_state(&mut self, instrument_id: &str) {
        if let Some(instr) = self.instruments.get_mut(instrument_id) {
            instr.session_state = None;
        }
    }

    /// Get the number of processed files for an instrument.
    pub fn processed_file_count(&self, instrument_id: &str) -> usize {
        self.instruments
            .get(instrument_id)
            .map(|i| i.processed_files.len())
            .unwrap_or(0)
    }

    /// Get total processed files across all instruments.
    pub fn total_processed_files(&self) -> usize {
        self.instruments
            .values()
            .map(|i| i.processed_files.len())
            .sum()
    }
}

/// Compute SHA256 hash of a file.
pub fn compute_file_hash(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];

    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    let result = hasher.finalize();
    Ok(hex::encode(result))
}

/// Compute SHA256 hash of bytes (for in-memory data).
pub fn compute_bytes_hash(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_checkpoint_save_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("checkpoint.json");
        let file_path = dir.path().join("test.parquet");

        // Create a real file to hash
        let mut f = File::create(&file_path).unwrap();
        f.write_all(b"test content").unwrap();

        let mut checkpoint = Checkpoint::new("test_hash");
        checkpoint
            .mark_processed("BTCUSDT", &file_path, 1000, 123456789)
            .unwrap();

        checkpoint.save(&path).unwrap();

        let loaded = Checkpoint::load_or_create(&path, "test_hash").unwrap();
        assert_eq!(loaded.config_hash, "test_hash");
        assert_eq!(loaded.processed_file_count("BTCUSDT"), 1);
    }

    #[test]
    fn test_checkpoint_config_change_clears() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("checkpoint.json");
        let file_path = dir.path().join("test.parquet");

        // Create a real file to hash
        let mut f = File::create(&file_path).unwrap();
        f.write_all(b"test content").unwrap();

        let mut checkpoint = Checkpoint::new("old_hash");
        checkpoint
            .mark_processed("BTCUSDT", &file_path, 1000, 123456789)
            .unwrap();
        checkpoint.save(&path).unwrap();

        // Load with different config hash
        let loaded = Checkpoint::load_or_create(&path, "new_hash").unwrap();
        assert_eq!(loaded.config_hash, "new_hash");
        assert_eq!(loaded.total_processed_files(), 0);
    }

    #[test]
    fn test_is_file_processed() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.parquet");
        let mut f = File::create(&file_path).unwrap();
        f.write_all(b"test content").unwrap();

        let mut checkpoint = Checkpoint::new("test_hash");

        // Not processed yet
        assert!(!checkpoint.is_file_processed("BTCUSDT", &file_path));

        // Mark as processed
        checkpoint
            .mark_processed("BTCUSDT", &file_path, 100, 123456)
            .unwrap();

        // Now it's processed
        assert!(checkpoint.is_file_processed("BTCUSDT", &file_path));
    }

    #[test]
    fn test_needs_reprocessing_on_change() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.parquet");
        let mut f = File::create(&file_path).unwrap();
        f.write_all(b"original content").unwrap();

        let mut checkpoint = Checkpoint::new("test_hash");
        checkpoint
            .mark_processed("BTCUSDT", &file_path, 100, 123456)
            .unwrap();

        // File unchanged - no reprocessing needed
        assert!(!checkpoint.needs_reprocessing("BTCUSDT", &file_path));

        // Modify file with different size so (size, mtime) check detects the change
        let mut f = File::create(&file_path).unwrap();
        f.write_all(b"modified content that is longer than the original")
            .unwrap();

        // Now needs reprocessing (different file size)
        assert!(checkpoint.needs_reprocessing("BTCUSDT", &file_path));
    }

    #[test]
    fn test_idempotency_no_duplicates() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.parquet");
        let mut f = File::create(&file_path).unwrap();
        f.write_all(b"test content").unwrap();

        let mut checkpoint = Checkpoint::new("test_hash");

        // Process file first time
        assert!(!checkpoint.is_file_processed("BTCUSDT", &file_path));
        checkpoint
            .mark_processed("BTCUSDT", &file_path, 100, 123456)
            .unwrap();

        // Second time - should be skipped
        assert!(checkpoint.is_file_processed("BTCUSDT", &file_path));
    }

    #[test]
    fn test_session_state() {
        let mut checkpoint = Checkpoint::new("test_hash");

        let state = SessionState {
            session_id: "2026-02-02T00:00:00Z".to_string(),
            session_date: "2026-02-02".to_string(),
            last_bracket_index: 5,
            running_volume: 1000000000,
            running_poc: 78000000000000,
            running_vah: 79000000000000,
            running_val: 77000000000000,
        };

        checkpoint.update_session_state("BTCUSDT", state.clone());

        let retrieved = checkpoint.get_session_state("BTCUSDT").unwrap();
        assert_eq!(retrieved.last_bracket_index, 5);

        checkpoint.clear_session_state("BTCUSDT");
        assert!(checkpoint.get_session_state("BTCUSDT").is_none());
    }

    #[test]
    fn test_file_hash_deterministic() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.parquet");
        let mut f = File::create(&file_path).unwrap();
        f.write_all(b"consistent content").unwrap();

        let hash1 = compute_file_hash(&file_path).unwrap();
        let hash2 = compute_file_hash(&file_path).unwrap();

        assert_eq!(hash1, hash2);
    }
}
