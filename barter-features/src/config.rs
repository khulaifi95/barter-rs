//! Configuration loading and validation.

use crate::error::Result;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Main configuration structure.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub general: GeneralConfig,
    #[serde(default)]
    pub filter: FilterConfig,
    #[serde(default)]
    pub precision: PrecisionConfig,
    #[serde(default)]
    pub session: SessionConfig,
    #[serde(default)]
    pub tpo: TpoConfig,
    #[serde(default)]
    pub large_trades: LargeTradesConfig,
    #[serde(default)]
    pub profile: ProfileConfig,
    #[serde(default)]
    pub gaps: GapsConfig,
    #[serde(default)]
    pub watcher: WatcherConfig,
    #[serde(default)]
    pub checkpoint: CheckpointConfig,
    #[serde(default)]
    pub writer: WriterConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    pub input_dir: PathBuf,
    pub output_dir: PathBuf,
    #[serde(default = "default_checkpoint_dir")]
    pub checkpoint_dir: PathBuf,
    #[serde(default = "default_mode")]
    pub mode: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FilterConfig {
    #[serde(default)]
    pub instruments: Vec<String>,
}

fn default_schema_version() -> String {
    "1.2.0".to_string()
}

fn default_checkpoint_dir() -> PathBuf {
    PathBuf::from("/data/_checkpoints")
}

fn default_mode() -> String {
    "watch".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrecisionConfig {
    #[serde(default = "default_precision_mode")]
    pub mode: String,
}

fn default_precision_mode() -> String {
    "auto".to_string()
}

impl Default for PrecisionConfig {
    fn default() -> Self {
        Self {
            mode: default_precision_mode(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionConfig {
    #[serde(default)]
    pub start_hour: u8,
    #[serde(default = "default_duration_hours")]
    pub duration_hours: u8,
    #[serde(default = "default_timezone")]
    pub timezone: String,
}

fn default_duration_hours() -> u8 {
    24
}

fn default_timezone() -> String {
    "UTC".to_string()
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            start_hour: 0,
            duration_hours: default_duration_hours(),
            timezone: default_timezone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TpoConfig {
    #[serde(default = "default_bracket_minutes")]
    pub bracket_minutes: u32,
    #[serde(default = "default_price_bucket_usd")]
    pub price_bucket_usd: f64,
    #[serde(default = "default_ib_brackets")]
    pub initial_balance_brackets: u8,
    #[serde(default = "default_value_area_pct")]
    pub value_area_pct: f64,
    #[serde(default = "default_va_algorithm")]
    pub value_area_algorithm: String,
}

fn default_bracket_minutes() -> u32 {
    30
}

fn default_price_bucket_usd() -> f64 {
    50.0
}

fn default_ib_brackets() -> u8 {
    2
}

fn default_value_area_pct() -> f64 {
    0.70
}

fn default_va_algorithm() -> String {
    "expanding".to_string()
}

impl Default for TpoConfig {
    fn default() -> Self {
        Self {
            bracket_minutes: default_bracket_minutes(),
            price_bucket_usd: default_price_bucket_usd(),
            initial_balance_brackets: default_ib_brackets(),
            value_area_pct: default_value_area_pct(),
            value_area_algorithm: default_va_algorithm(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LargeTradesConfig {
    #[serde(default = "default_threshold_mode")]
    pub threshold_mode: String,
    #[serde(default = "default_large_threshold")]
    pub large_threshold_usd: f64,
    #[serde(default = "default_whale_threshold")]
    pub whale_threshold_usd: f64,
    #[serde(default = "default_mega_threshold")]
    pub mega_threshold_usd: f64,
}

fn default_threshold_mode() -> String {
    "absolute".to_string()
}

fn default_large_threshold() -> f64 {
    2_000_000.0
}

fn default_whale_threshold() -> f64 {
    5_000_000.0
}

fn default_mega_threshold() -> f64 {
    10_000_000.0
}

impl Default for LargeTradesConfig {
    fn default() -> Self {
        Self {
            threshold_mode: default_threshold_mode(),
            large_threshold_usd: default_large_threshold(),
            whale_threshold_usd: default_whale_threshold(),
            mega_threshold_usd: default_mega_threshold(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProfileConfig {
    #[serde(default = "default_poc_shift")]
    pub poc_shift_threshold_usd: f64,
    #[serde(default = "default_va_shift")]
    pub va_shift_threshold_usd: f64,
    #[serde(default = "default_poc_divergence")]
    pub poc_divergence_threshold_usd: f64,
}

fn default_poc_shift() -> f64 {
    100.0
}

fn default_va_shift() -> f64 {
    150.0
}

fn default_poc_divergence() -> f64 {
    200.0
}

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            poc_shift_threshold_usd: default_poc_shift(),
            va_shift_threshold_usd: default_va_shift(),
            poc_divergence_threshold_usd: default_poc_divergence(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct GapsConfig {
    #[serde(default = "default_gap_handling")]
    pub handling: String,
    #[serde(default = "default_max_gap")]
    pub max_gap_minutes: u32,
    #[serde(default = "default_tolerance")]
    pub tolerance_secs: u32,
}

fn default_gap_handling() -> String {
    "emit_event".to_string()
}

fn default_max_gap() -> u32 {
    60
}

fn default_tolerance() -> u32 {
    2
}

impl Default for GapsConfig {
    fn default() -> Self {
        Self {
            handling: default_gap_handling(),
            max_gap_minutes: default_max_gap(),
            tolerance_secs: default_tolerance(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct WatcherConfig {
    #[serde(default = "default_stable_mtime")]
    pub stable_mtime_secs: u64,
}

fn default_stable_mtime() -> u64 {
    2
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            stable_mtime_secs: default_stable_mtime(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CheckpointConfig {
    #[serde(default = "default_checkpoint_enabled")]
    pub enabled: bool,
    #[serde(default = "default_save_interval")]
    pub save_interval_secs: u64,
    #[serde(default = "default_verify_hashes")]
    pub verify_hashes: bool,
    #[serde(default = "default_checkpoint_file")]
    pub file: String,
}

fn default_checkpoint_enabled() -> bool {
    true
}

fn default_save_interval() -> u64 {
    30
}

fn default_verify_hashes() -> bool {
    true
}

fn default_checkpoint_file() -> String {
    "features_state.json".to_string()
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            enabled: default_checkpoint_enabled(),
            save_interval_secs: default_save_interval(),
            verify_hashes: default_verify_hashes(),
            file: default_checkpoint_file(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct WriterConfig {
    #[serde(default = "default_compression")]
    pub compression: String,
    #[serde(default = "default_flush_interval")]
    pub flush_interval_secs: u64,
    #[serde(default = "default_partition_by_date")]
    pub partition_by_date: bool,
    #[serde(default = "default_include_metadata")]
    pub include_metadata: bool,
}

fn default_compression() -> String {
    "zstd".to_string()
}

fn default_flush_interval() -> u64 {
    60
}

fn default_partition_by_date() -> bool {
    true
}

fn default_include_metadata() -> bool {
    true
}

impl Default for WriterConfig {
    fn default() -> Self {
        Self {
            compression: default_compression(),
            flush_interval_secs: default_flush_interval(),
            partition_by_date: default_partition_by_date(),
            include_metadata: default_include_metadata(),
        }
    }
}

impl Config {
    /// Load configuration from a TOML file.
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    /// Load configuration from the default location or environment.
    pub fn load() -> Result<Self> {
        // Check for config file path in environment
        if let Ok(path) = std::env::var("BARTER_FEATURES_CONFIG") {
            return Self::from_file(Path::new(&path));
        }

        // Try default locations
        let default_paths = [
            PathBuf::from("config/default.toml"),
            PathBuf::from("/etc/barter-features/config.toml"),
        ];

        for path in &default_paths {
            if path.exists() {
                return Self::from_file(path);
            }
        }

        // Fall back to environment-based config
        Self::from_env()
    }

    /// Create configuration from environment variables.
    pub fn from_env() -> Result<Self> {
        let input_dir =
            std::env::var("BARTER_FEATURES_INPUT_DIR").unwrap_or_else(|_| "/data/raw".to_string());
        let output_dir = std::env::var("BARTER_FEATURES_OUTPUT_DIR")
            .unwrap_or_else(|_| "/data/features".to_string());
        let checkpoint_dir = std::env::var("BARTER_FEATURES_CHECKPOINT_DIR")
            .unwrap_or_else(|_| "/data/_checkpoints".to_string());
        let mode = std::env::var("BARTER_FEATURES_MODE").unwrap_or_else(|_| "watch".to_string());
        let instruments = std::env::var("BARTER_FEATURES_FILTER_INSTRUMENTS")
            .ok()
            .map(|raw| {
                raw.split(',')
                    .map(|v| v.trim())
                    .filter(|v| !v.is_empty())
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(Config {
            general: GeneralConfig {
                schema_version: default_schema_version(),
                input_dir: PathBuf::from(input_dir),
                output_dir: PathBuf::from(output_dir),
                checkpoint_dir: PathBuf::from(checkpoint_dir),
                mode,
            },
            filter: FilterConfig { instruments },
            precision: PrecisionConfig::default(),
            session: SessionConfig::default(),
            tpo: TpoConfig::default(),
            large_trades: LargeTradesConfig::default(),
            profile: ProfileConfig::default(),
            gaps: GapsConfig::default(),
            watcher: WatcherConfig::default(),
            checkpoint: CheckpointConfig::default(),
            writer: WriterConfig::default(),
        })
    }

    /// Compute a hash of the configuration for reproducibility tracking.
    pub fn config_hash(&self) -> String {
        let mut hasher = Sha256::new();
        // Hash key configuration values that affect output
        hasher.update(self.general.schema_version.as_bytes());
        let mut instruments = self.filter.instruments.clone();
        instruments.sort();
        for instrument in &instruments {
            hasher.update(instrument.as_bytes());
        }
        hasher.update(self.tpo.bracket_minutes.to_le_bytes());
        hasher.update(self.tpo.price_bucket_usd.to_le_bytes());
        hasher.update(self.tpo.value_area_pct.to_le_bytes());
        hasher.update(self.large_trades.large_threshold_usd.to_le_bytes());
        hasher.update(self.large_trades.whale_threshold_usd.to_le_bytes());
        hasher.update(self.large_trades.mega_threshold_usd.to_le_bytes());
        hasher.update(self.profile.poc_shift_threshold_usd.to_le_bytes());

        let result = hasher.finalize();
        // Truncate to 16 chars for readability
        hex::encode(&result[..8])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::from_env().unwrap();
        assert_eq!(config.general.schema_version, "1.2.0");
        assert_eq!(config.tpo.bracket_minutes, 30);
        assert_eq!(config.tpo.value_area_pct, 0.70);
        assert_eq!(config.large_trades.large_threshold_usd, 2_000_000.0);
    }

    #[test]
    fn test_config_hash_deterministic() {
        let config1 = Config::from_env().unwrap();
        let config2 = Config::from_env().unwrap();
        assert_eq!(config1.config_hash(), config2.config_hash());
    }

    #[test]
    fn test_config_hash_changes_with_values() {
        let mut config1 = Config::from_env().unwrap();
        let hash1 = config1.config_hash();

        config1.tpo.bracket_minutes = 15;
        let hash2 = config1.config_hash();

        assert_ne!(hash1, hash2);
    }
}
