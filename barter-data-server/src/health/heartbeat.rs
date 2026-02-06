//! Heartbeat file writer for health monitoring.
//!
//! Writes a JSON file that external monitoring tools can check to verify
//! the collector is running and producing data.

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::fs;
use tokio::sync::RwLock;
use tokio::time::{Duration, interval};
use tracing::{debug, error, info};

/// Configuration for heartbeat writer.
#[derive(Debug, Clone)]
pub struct HeartbeatConfig {
    /// Path to heartbeat JSON file
    pub file_path: PathBuf,
    /// Interval between heartbeat writes in seconds
    pub interval_secs: u64,
}

impl HeartbeatConfig {
    /// Load configuration from environment variables.
    pub fn from_env() -> Self {
        let file_path = std::env::var("HEARTBEAT_FILE")
            .unwrap_or_else(|_| "/tmp/collector-heartbeat.json".to_string())
            .into();
        let interval_secs = std::env::var("HEARTBEAT_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);

        Self {
            file_path,
            interval_secs,
        }
    }
}

/// Heartbeat JSON structure.
#[derive(Debug, Serialize, serde::Deserialize)]
pub struct HeartbeatData {
    /// Last successful write timestamp
    pub last_write: DateTime<Utc>,
    /// List of symbols being tracked
    pub symbols: Vec<String>,
    /// Last bar timestamp per symbol
    pub last_bars: HashMap<String, DateTime<Utc>>,
    /// Total bars written since startup
    pub bars_written_total: u64,
    /// Total trades processed since startup
    pub trades_processed_total: u64,
    /// Uptime in seconds
    pub uptime_secs: u64,
    /// Collector version
    pub version: String,
}

/// Heartbeat state that can be updated from other tasks.
#[derive(Debug, Default)]
pub struct HeartbeatState {
    /// Symbols being tracked
    pub symbols: Vec<String>,
    /// Last bar time per symbol
    pub last_bars: HashMap<String, DateTime<Utc>>,
}

/// Heartbeat writer that periodically writes status to a file.
pub struct HeartbeatWriter {
    config: HeartbeatConfig,
    state: Arc<RwLock<HeartbeatState>>,
    bars_written: Arc<AtomicU64>,
    trades_processed: Arc<AtomicU64>,
    start_time: std::time::Instant,
}

impl HeartbeatWriter {
    /// Create a new heartbeat writer.
    pub fn new(
        config: HeartbeatConfig,
        bars_written: Arc<AtomicU64>,
        trades_processed: Arc<AtomicU64>,
    ) -> Self {
        Self {
            config,
            state: Arc::new(RwLock::new(HeartbeatState::default())),
            bars_written,
            trades_processed,
            start_time: std::time::Instant::now(),
        }
    }

    /// Get a handle to update state from other tasks.
    pub fn state_handle(&self) -> Arc<RwLock<HeartbeatState>> {
        Arc::clone(&self.state)
    }

    /// Write heartbeat to file.
    pub async fn write_heartbeat(&self) -> Result<(), std::io::Error> {
        let state = self.state.read().await;

        let data = HeartbeatData {
            last_write: Utc::now(),
            symbols: state.symbols.clone(),
            last_bars: state.last_bars.clone(),
            bars_written_total: self.bars_written.load(Ordering::Relaxed),
            trades_processed_total: self.trades_processed.load(Ordering::Relaxed),
            uptime_secs: self.start_time.elapsed().as_secs(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        };

        let json = serde_json::to_string_pretty(&data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        // Write to temp file first, then rename for atomicity
        let temp_path = self.config.file_path.with_extension("tmp");
        fs::write(&temp_path, &json).await?;
        fs::rename(&temp_path, &self.config.file_path).await?;

        debug!("Wrote heartbeat to {:?}", self.config.file_path);
        Ok(())
    }
}

/// Run the heartbeat writer as an async task.
pub async fn run_heartbeat_task(
    config: HeartbeatConfig,
    bars_written: Arc<AtomicU64>,
    trades_processed: Arc<AtomicU64>,
) -> Arc<RwLock<HeartbeatState>> {
    info!(
        "Heartbeat task starting: file={:?}, interval={}s",
        config.file_path, config.interval_secs
    );

    let interval_secs = config.interval_secs;
    let writer = HeartbeatWriter::new(config, bars_written, trades_processed);
    let state_handle = writer.state_handle();

    tokio::spawn(async move {
        let mut tick = interval(Duration::from_secs(interval_secs));

        loop {
            tick.tick().await;
            if let Err(e) = writer.write_heartbeat().await {
                error!("Failed to write heartbeat: {}", e);
            }
        }
    });

    state_handle
}

/// Update heartbeat state when a bar is written.
pub async fn update_heartbeat_bar(
    state: &Arc<RwLock<HeartbeatState>>,
    symbol: &str,
    timestamp: DateTime<Utc>,
) {
    let mut state = state.write().await;
    state.last_bars.insert(symbol.to_string(), timestamp);
    if !state.symbols.contains(&symbol.to_string()) {
        state.symbols.push(symbol.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_heartbeat_write() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("heartbeat.json");

        let config = HeartbeatConfig {
            file_path: file_path.clone(),
            interval_secs: 30,
        };

        let bars = Arc::new(AtomicU64::new(100));
        let trades = Arc::new(AtomicU64::new(5000));

        let writer = HeartbeatWriter::new(config, bars, trades);

        // Update state
        {
            let mut state = writer.state.write().await;
            state.symbols = vec!["BTCUSDT".to_string(), "ETHUSDT".to_string()];
            state.last_bars.insert("BTCUSDT".to_string(), Utc::now());
        }

        // Write heartbeat
        writer.write_heartbeat().await.unwrap();

        // Verify file exists and is valid JSON
        let content = fs::read_to_string(&file_path).await.unwrap();
        let data: HeartbeatData = serde_json::from_str(&content).unwrap();

        assert_eq!(data.symbols.len(), 2);
        assert_eq!(data.bars_written_total, 100);
        assert_eq!(data.trades_processed_total, 5000);
    }

    #[tokio::test]
    async fn test_update_heartbeat_bar() {
        let state = Arc::new(RwLock::new(HeartbeatState::default()));

        update_heartbeat_bar(&state, "BTCUSDT", Utc::now()).await;
        update_heartbeat_bar(&state, "ETHUSDT", Utc::now()).await;
        update_heartbeat_bar(&state, "BTCUSDT", Utc::now()).await; // Duplicate

        let state = state.read().await;
        assert_eq!(state.symbols.len(), 2);
        assert!(state.last_bars.contains_key("BTCUSDT"));
        assert!(state.last_bars.contains_key("ETHUSDT"));
    }
}
