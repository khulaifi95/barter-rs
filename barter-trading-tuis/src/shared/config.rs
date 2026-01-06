//! Configuration Provider for Market State Engine
//!
//! Loads thresholds from `config/thresholds.toml` and provides the `ConfigProvider` trait
//! implementation. Supports per-ticker overrides with precedence: ticker-specific > default.
//!
//! # Usage
//! ```rust,ignore
//! use barter_trading_tuis::shared::config::Config;
//!
//! let config = Config::load()?;
//! let thresholds = config.thresholds();
//! let freshness = config.freshness(Signal::Price);
//! ```

use serde::Deserialize;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::time::Duration;

use crate::shared::market_state::{
    AuditThresholds, ConfigProvider, ConsensusThresholds, FreshnessThresholds, FuelThresholds,
    FundingOverrides, FundingThresholds, GammaThresholds, Signal, Thresholds, TickerOverrides,
    VolatilityOverrides, VolatilityThresholds,
};

// ============================================================================
// ERROR TYPES
// ============================================================================

/// Configuration loading errors
#[derive(Debug)]
pub enum ConfigError {
    /// Config file not found
    FileNotFound(String),
    /// IO error reading config file
    IoError(io::Error),
    /// TOML parsing error
    ParseError(toml::de::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::FileNotFound(path) => {
                write!(
                    f,
                    "Config file not found: {}. Please ensure config/thresholds.toml exists.",
                    path
                )
            }
            ConfigError::IoError(e) => write!(f, "IO error reading config: {}", e),
            ConfigError::ParseError(e) => write!(f, "TOML parse error: {}", e),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::IoError(e) => Some(e),
            ConfigError::ParseError(e) => Some(e),
            ConfigError::FileNotFound(_) => None,
        }
    }
}

impl From<io::Error> for ConfigError {
    fn from(e: io::Error) -> Self {
        ConfigError::IoError(e)
    }
}

impl From<toml::de::Error> for ConfigError {
    fn from(e: toml::de::Error) -> Self {
        ConfigError::ParseError(e)
    }
}

// ============================================================================
// CONFIG FILE STRUCTURE (matches TOML schema)
// ============================================================================

/// Full config file structure for deserialization
/// This matches the layout of config/thresholds.toml exactly
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigFile {
    pub volatility: VolatilityThresholds,
    pub consensus: ConsensusThresholds,
    pub funding: FundingThresholds,
    pub fuel: FuelThresholds,
    pub freshness_ms: FreshnessThresholds,
    pub gamma: GammaThresholds,
    pub audit: AuditThresholds,
    /// Per-ticker overrides (optional)
    #[serde(default)]
    pub ticker: HashMap<String, TickerOverrides>,
}

// ============================================================================
// CONFIG PROVIDER IMPLEMENTATION
// ============================================================================

/// Configuration provider that loads from TOML and supports per-ticker overrides
#[derive(Debug, Clone)]
pub struct Config {
    /// Base thresholds (defaults from config file)
    base: Thresholds,
    /// Per-ticker overrides
    ticker_overrides: HashMap<String, TickerOverrides>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            base: Thresholds {
                volatility: VolatilityThresholds {
                    percentile_extreme: 95.0,
                    percentile_high: 80.0,
                    zscore_shock: 3.5,
                },
                consensus: ConsensusThresholds {
                    min_agreement_pct: 66,
                },
                funding: FundingThresholds {
                    spike_threshold: 0.0002,
                    extreme_long: 0.0005,
                    extreme_short: -0.0001,
                },
                fuel: FuelThresholds {
                    rvol_strong_min: 2.0,
                    rvol_normal_min: 0.8,
                    rvol_thin_min: 0.5,
                    oi_momentum_fail_usd: -15_000_000.0,
                    oi_momentum_caution_usd: -5_000_000.0,
                    oi_flat_usd: 3_000_000.0,
                    oi_new_money_min_usd: 5_000_000.0,
                    liq_caution_usd_per_min: 150_000.0,
                    liq_fail_usd_per_min: 2_000_000.0,
                },
                freshness_ms: FreshnessThresholds {
                    price: 1000,
                    cvd: 2000,
                    l2_book: 2000,
                    whale: 10000,
                    funding: 300000,
                    gamma: 600000,
                    trad_markets: 120000,
                },
                gamma: GammaThresholds {
                    flip_buffer_pct: 0.5,
                },
                audit: AuditThresholds {
                    channel_size: 10000,
                    log_dir: "logs/audit".to_string(),
                    rotation_mb: 500,
                    retention_days: 30,
                },
            },
            ticker_overrides: {
                let mut map = HashMap::new();
                // Add SOL override per spec
                map.insert(
                    "SOL".to_string(),
                    TickerOverrides {
                        volatility: Some(VolatilityOverrides {
                            percentile_extreme: Some(90.0),
                            percentile_high: None,
                            zscore_shock: None,
                        }),
                        funding: None,
                    },
                );
                map
            },
        }
    }
}

impl Config {
    /// Default config file path (relative to working directory)
    pub const DEFAULT_PATH: &'static str = "config/thresholds.toml";

    /// Load configuration from the default path
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(Self::DEFAULT_PATH)
    }

    /// Load configuration from a specific path
    pub fn load_from<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let path = path.as_ref();

        // Check if file exists with clear error message
        if !path.exists() {
            return Err(ConfigError::FileNotFound(path.display().to_string()));
        }

        let content = std::fs::read_to_string(path)?;
        Self::from_str(&content)
    }

    /// Parse configuration from a TOML string (useful for testing)
    pub fn from_str(toml_content: &str) -> Result<Self, ConfigError> {
        let file: ConfigFile = toml::from_str(toml_content)?;

        // Build base Thresholds from parsed file
        let base = Thresholds {
            volatility: file.volatility,
            consensus: file.consensus,
            funding: file.funding,
            fuel: file.fuel,
            freshness_ms: file.freshness_ms,
            gamma: file.gamma,
            audit: file.audit,
        };

        Ok(Self {
            base,
            ticker_overrides: file.ticker,
        })
    }

    /// Get thresholds with ticker-specific overrides applied
    /// Precedence: ticker-specific > default
    pub fn thresholds_for_ticker(&self, ticker: &str) -> Thresholds {
        let overrides = self.ticker_overrides.get(ticker);

        match overrides {
            None => self.base.clone(),
            Some(overrides) => {
                let mut thresholds = self.base.clone();

                // Apply volatility overrides
                if let Some(vol) = &overrides.volatility {
                    apply_volatility_overrides(&mut thresholds.volatility, vol);
                }

                // Apply funding overrides
                if let Some(fund) = &overrides.funding {
                    apply_funding_overrides(&mut thresholds.funding, fund);
                }

                thresholds
            }
        }
    }

    /// Check if a ticker has any overrides
    pub fn has_overrides(&self, ticker: &str) -> bool {
        self.ticker_overrides.contains_key(ticker)
    }

    /// Get list of tickers with overrides
    pub fn tickers_with_overrides(&self) -> Vec<&str> {
        self.ticker_overrides.keys().map(|s| s.as_str()).collect()
    }
}

/// Apply volatility overrides to thresholds
fn apply_volatility_overrides(base: &mut VolatilityThresholds, overrides: &VolatilityOverrides) {
    if let Some(v) = overrides.percentile_extreme {
        base.percentile_extreme = v;
    }
    if let Some(v) = overrides.percentile_high {
        base.percentile_high = v;
    }
    if let Some(v) = overrides.zscore_shock {
        base.zscore_shock = v;
    }
}

/// Apply funding overrides to thresholds
fn apply_funding_overrides(base: &mut FundingThresholds, overrides: &FundingOverrides) {
    if let Some(v) = overrides.spike_threshold {
        base.spike_threshold = v;
    }
    if let Some(v) = overrides.extreme_long {
        base.extreme_long = v;
    }
    if let Some(v) = overrides.extreme_short {
        base.extreme_short = v;
    }
}

// ============================================================================
// ConfigProvider TRAIT IMPLEMENTATION
// ============================================================================

impl ConfigProvider for Config {
    fn thresholds(&self) -> &Thresholds {
        &self.base
    }

    fn freshness(&self, signal: Signal) -> Duration {
        let ms = match signal {
            Signal::Price => self.base.freshness_ms.price,
            Signal::Cvd => self.base.freshness_ms.cvd,
            Signal::L2Book => self.base.freshness_ms.l2_book,
            Signal::Whale => self.base.freshness_ms.whale,
            Signal::Funding => self.base.freshness_ms.funding,
            Signal::Gamma => self.base.freshness_ms.gamma,
            Signal::TradMarkets => self.base.freshness_ms.trad_markets,
        };
        Duration::from_millis(ms)
    }
}

// ============================================================================
// UNIT TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Sample TOML config for testing
    const TEST_CONFIG: &str = r#"
[volatility]
percentile_extreme = 95
percentile_high = 80
zscore_shock = 3.5

[consensus]
min_agreement_pct = 66

[funding]
spike_threshold = 0.0002
extreme_long = 0.0005
extreme_short = -0.0001

[fuel]
rvol_strong_min = 2.0
rvol_normal_min = 0.8
rvol_thin_min = 0.5
oi_momentum_fail_usd = -15000000
oi_momentum_caution_usd = -5000000
oi_flat_usd = 3000000
oi_new_money_min_usd = 5000000
liq_caution_usd_per_min = 150000
liq_fail_usd_per_min = 2000000

[freshness_ms]
price = 1000
cvd = 2000
l2_book = 2000
whale = 10000
funding = 300000
gamma = 600000
trad_markets = 120000

[gamma]
flip_buffer_pct = 0.5

[audit]
channel_size = 10000
log_dir = "logs/audit"
rotation_mb = 500
retention_days = 30

[ticker.ETH.funding]
extreme_long = 0.0008

[ticker.SOL.volatility]
percentile_extreme = 90
"#;

    #[test]
    fn test_parse_config_from_str() {
        let config = Config::from_str(TEST_CONFIG).expect("Failed to parse config");

        // Check base thresholds
        assert_eq!(config.base.volatility.percentile_extreme, 95.0);
        assert_eq!(config.base.volatility.percentile_high, 80.0);
        assert_eq!(config.base.volatility.zscore_shock, 3.5);

        assert_eq!(config.base.consensus.min_agreement_pct, 66);

        assert_eq!(config.base.funding.spike_threshold, 0.0002);
        assert_eq!(config.base.funding.extreme_long, 0.0005);
        assert_eq!(config.base.funding.extreme_short, -0.0001);

        assert_eq!(config.base.freshness_ms.price, 1000);
        assert_eq!(config.base.freshness_ms.cvd, 2000);
        assert_eq!(config.base.freshness_ms.l2_book, 2000);
        assert_eq!(config.base.freshness_ms.whale, 10000);
        assert_eq!(config.base.freshness_ms.funding, 300000);
        assert_eq!(config.base.freshness_ms.gamma, 600000);
        assert_eq!(config.base.freshness_ms.trad_markets, 120000);

        assert_eq!(config.base.gamma.flip_buffer_pct, 0.5);

        assert_eq!(config.base.audit.channel_size, 10000);
        assert_eq!(config.base.audit.log_dir, "logs/audit");
        assert_eq!(config.base.audit.rotation_mb, 500);
        assert_eq!(config.base.audit.retention_days, 30);
    }

    #[test]
    fn test_ticker_overrides_exist() {
        let config = Config::from_str(TEST_CONFIG).expect("Failed to parse config");

        assert!(config.has_overrides("ETH"));
        assert!(config.has_overrides("SOL"));
        assert!(!config.has_overrides("BTC")); // No overrides in test config
        assert!(!config.has_overrides("DOGE"));
    }

    #[test]
    fn test_thresholds_for_btc_uses_defaults() {
        let config = Config::from_str(TEST_CONFIG).expect("Failed to parse config");

        let btc_thresholds = config.thresholds_for_ticker("BTC");

        // BTC should use all defaults
        assert_eq!(btc_thresholds.volatility.percentile_extreme, 95.0);
        assert_eq!(btc_thresholds.funding.extreme_long, 0.0005);
    }

    #[test]
    fn test_thresholds_for_eth_has_funding_override() {
        let config = Config::from_str(TEST_CONFIG).expect("Failed to parse config");

        let eth_thresholds = config.thresholds_for_ticker("ETH");

        // ETH has funding override
        assert_eq!(eth_thresholds.funding.extreme_long, 0.0008);

        // Other values should be defaults
        assert_eq!(eth_thresholds.volatility.percentile_extreme, 95.0);
        assert_eq!(eth_thresholds.funding.spike_threshold, 0.0002);
        assert_eq!(eth_thresholds.funding.extreme_short, -0.0001);
    }

    #[test]
    fn test_thresholds_for_sol_has_volatility_override() {
        let config = Config::from_str(TEST_CONFIG).expect("Failed to parse config");

        let sol_thresholds = config.thresholds_for_ticker("SOL");

        // SOL has volatility override
        assert_eq!(sol_thresholds.volatility.percentile_extreme, 90.0);

        // Other values should be defaults
        assert_eq!(sol_thresholds.volatility.percentile_high, 80.0);
        assert_eq!(sol_thresholds.volatility.zscore_shock, 3.5);
        assert_eq!(sol_thresholds.funding.extreme_long, 0.0005);
    }

    #[test]
    fn test_freshness_duration_mapping() {
        let config = Config::from_str(TEST_CONFIG).expect("Failed to parse config");

        assert_eq!(config.freshness(Signal::Price), Duration::from_millis(1000));
        assert_eq!(config.freshness(Signal::Cvd), Duration::from_millis(2000));
        assert_eq!(
            config.freshness(Signal::L2Book),
            Duration::from_millis(2000)
        );
        assert_eq!(
            config.freshness(Signal::Whale),
            Duration::from_millis(10000)
        );
        assert_eq!(
            config.freshness(Signal::Funding),
            Duration::from_millis(300000)
        );
        assert_eq!(
            config.freshness(Signal::Gamma),
            Duration::from_millis(600000)
        );
        assert_eq!(
            config.freshness(Signal::TradMarkets),
            Duration::from_millis(120000)
        );
    }

    #[test]
    fn test_config_provider_trait() {
        let config = Config::from_str(TEST_CONFIG).expect("Failed to parse config");

        // Test trait methods
        let thresholds: &Thresholds = config.thresholds();
        assert_eq!(thresholds.volatility.percentile_extreme, 95.0);

        let freshness = config.freshness(Signal::Price);
        assert_eq!(freshness, Duration::from_millis(1000));
    }

    #[test]
    fn test_tickers_with_overrides() {
        let config = Config::from_str(TEST_CONFIG).expect("Failed to parse config");

        let tickers = config.tickers_with_overrides();
        assert!(tickers.contains(&"ETH"));
        assert!(tickers.contains(&"SOL"));
        assert_eq!(tickers.len(), 2);
    }

    #[test]
    fn test_parse_error_on_invalid_toml() {
        let invalid_toml = "this is not valid toml [[[";
        let result = Config::from_str(invalid_toml);
        assert!(matches!(result, Err(ConfigError::ParseError(_))));
    }

    #[test]
    fn test_parse_error_on_missing_field() {
        let incomplete_toml = r#"
[volatility]
percentile_extreme = 95
"#;
        let result = Config::from_str(incomplete_toml);
        assert!(matches!(result, Err(ConfigError::ParseError(_))));
    }

    #[test]
    fn test_file_not_found_error() {
        let result = Config::load_from("/nonexistent/path/config.toml");
        assert!(matches!(result, Err(ConfigError::FileNotFound(_))));

        // Check error message
        if let Err(ConfigError::FileNotFound(path)) = result {
            assert!(path.contains("nonexistent"));
        }
    }

    #[test]
    fn test_error_display() {
        let err = ConfigError::FileNotFound("/path/to/config.toml".to_string());
        let msg = format!("{}", err);
        assert!(msg.contains("Config file not found"));
        assert!(msg.contains("/path/to/config.toml"));
    }

    #[test]
    fn test_multiple_overrides_for_same_ticker() {
        let config_with_multiple = r#"
[volatility]
percentile_extreme = 95
percentile_high = 80
zscore_shock = 3.5

[consensus]
min_agreement_pct = 66

[funding]
spike_threshold = 0.0002
extreme_long = 0.0005
extreme_short = -0.0001

[freshness_ms]
price = 1000
cvd = 2000
l2_book = 2000
whale = 10000
funding = 300000
gamma = 600000
trad_markets = 120000

[gamma]
flip_buffer_pct = 0.5

[audit]
channel_size = 10000
log_dir = "logs/audit"
rotation_mb = 500
retention_days = 30

[ticker.DOGE.volatility]
percentile_extreme = 85
percentile_high = 70

[ticker.DOGE.funding]
extreme_long = 0.001
spike_threshold = 0.0003
"#;

        let config = Config::from_str(config_with_multiple).expect("Failed to parse config");
        let doge_thresholds = config.thresholds_for_ticker("DOGE");

        // Check volatility overrides
        assert_eq!(doge_thresholds.volatility.percentile_extreme, 85.0);
        assert_eq!(doge_thresholds.volatility.percentile_high, 70.0);
        assert_eq!(doge_thresholds.volatility.zscore_shock, 3.5); // Default

        // Check funding overrides
        assert_eq!(doge_thresholds.funding.extreme_long, 0.001);
        assert_eq!(doge_thresholds.funding.spike_threshold, 0.0003);
        assert_eq!(doge_thresholds.funding.extreme_short, -0.0001); // Default
    }

    #[test]
    fn test_empty_ticker_overrides() {
        let config_no_overrides = r#"
[volatility]
percentile_extreme = 95
percentile_high = 80
zscore_shock = 3.5

[consensus]
min_agreement_pct = 66

[funding]
spike_threshold = 0.0002
extreme_long = 0.0005
extreme_short = -0.0001

[freshness_ms]
price = 1000
cvd = 2000
l2_book = 2000
whale = 10000
funding = 300000
gamma = 600000
trad_markets = 120000

[gamma]
flip_buffer_pct = 0.5

[audit]
channel_size = 10000
log_dir = "logs/audit"
rotation_mb = 500
retention_days = 30
"#;

        let config = Config::from_str(config_no_overrides).expect("Failed to parse config");
        assert!(config.tickers_with_overrides().is_empty());
        assert!(!config.has_overrides("BTC"));

        // Should still work with any ticker
        let btc_thresholds = config.thresholds_for_ticker("BTC");
        assert_eq!(btc_thresholds.volatility.percentile_extreme, 95.0);
    }

    /// Integration test that validates the real config file parses correctly.
    /// This test uses the actual config/thresholds.toml file content.
    #[test]
    fn test_real_config_file_content() {
        // This is the content from the actual config/thresholds.toml
        // We inline it here to ensure the test works regardless of working directory
        let real_config = r#"
# Market State Engine Configuration
# Single source of truth for all thresholds
# See docs/market_state.md Section 11 for documentation

# =============================================================================
# VOLATILITY THRESHOLDS (L1 Regime Filter)
# =============================================================================
[volatility]
percentile_extreme = 95    # L1 kill threshold - WAIT if >= this
percentile_high = 80       # Caution threshold - CAUTION if >= this
zscore_shock = 3.5         # Flash crash threshold - WAIT if abs(zscore) >= this

# =============================================================================
# CONSENSUS THRESHOLDS (L3 Flow Filter)
# =============================================================================
[consensus]
min_agreement_pct = 66     # 2/3 venues must agree for READY

# =============================================================================
# FUNDING THRESHOLDS (L3 Funding Filter)
# =============================================================================
[funding]
spike_threshold = 0.0002   # 0.02% velocity in 15m = spike -> CAUTION
extreme_long = 0.0005      # >0.05% funding = extreme long -> Warning
extreme_short = -0.0001    # <-0.01% funding = extreme short -> Warning

# =============================================================================
# FUEL THRESHOLDS (L3 Quality Filter)
# =============================================================================
[fuel]
rvol_strong_min = 2.0      # >= 2.0x = strong conviction
rvol_normal_min = 0.8      # >= 0.8x = normal activity
rvol_thin_min = 0.5        # >= 0.5x = thin volume
oi_momentum_fail_usd = -15000000   # <= -$15M = squeeze risk (momentum)
oi_momentum_caution_usd = -5000000 # <= -$5M = caution (momentum)
oi_flat_usd = 3000000              # <= $3M = flat/neutral
oi_new_money_min_usd = 5000000     # >= $5M = new money
liq_caution_usd_per_min = 150000   # >= $150K/min = caution (elevated)
liq_fail_usd_per_min = 2000000     # >= $2M/min = exhaustion

# =============================================================================
# FRESHNESS THRESHOLDS (Data Staleness Gates)
# All values in milliseconds
# =============================================================================
[freshness_ms]
price = 1000               # 1 second - WAIT if stale
cvd = 2000                 # 2 seconds - WAIT if stale
l2_book = 2000             # 2 seconds - use last known if stale
whale = 10000              # 10 seconds - ignore filter if stale
funding = 300000           # 5 minutes - use last known if stale
gamma = 600000             # 10 minutes - NO-GAMMA mode if stale
trad_markets = 120000      # 2 minutes - NO-TRAD mode if stale

# =============================================================================
# GAMMA THRESHOLDS (L2 Bias Filter)
# =============================================================================
[gamma]
flip_buffer_pct = 0.5      # Within 0.5% of flip = AT_FLIP (uncertain zone)

# =============================================================================
# AUDIT CONFIGURATION
# =============================================================================
[audit]
channel_size = 10000       # Bounded mpsc buffer - drop logs if full
log_dir = "logs/audit"     # Audit log directory
rotation_mb = 500          # Rotate file at 500MB
retention_days = 30        # Keep logs for 30 days

# =============================================================================
# PER-TICKER OVERRIDES
# Precedence: ticker-specific > default
# Only specify fields that differ from defaults
# =============================================================================

[ticker.BTC]
# BTC uses all defaults - no overrides needed

[ticker.ETH]
# ETH tends to have higher funding extremes
[ticker.ETH.funding]
extreme_long = 0.0008      # ETH tolerates higher positive funding

[ticker.SOL]
# SOL is more volatile, use stricter threshold
[ticker.SOL.volatility]
percentile_extreme = 90    # Trigger WAIT earlier for SOL
"#;

        let config = Config::from_str(real_config).expect("Failed to parse real config file");

        // Validate core thresholds match expected values
        assert_eq!(config.base.volatility.percentile_extreme, 95.0);
        assert_eq!(config.base.volatility.percentile_high, 80.0);
        assert_eq!(config.base.volatility.zscore_shock, 3.5);
        assert_eq!(config.base.consensus.min_agreement_pct, 66);
        assert_eq!(config.base.funding.spike_threshold, 0.0002);
        assert_eq!(config.base.gamma.flip_buffer_pct, 0.5);

        // Validate ETH override
        let eth = config.thresholds_for_ticker("ETH");
        assert_eq!(eth.funding.extreme_long, 0.0008);

        // Validate SOL override
        let sol = config.thresholds_for_ticker("SOL");
        assert_eq!(sol.volatility.percentile_extreme, 90.0);

        // Validate BTC uses defaults (empty override section)
        let btc = config.thresholds_for_ticker("BTC");
        assert_eq!(btc.volatility.percentile_extreme, 95.0);
        assert_eq!(btc.funding.extreme_long, 0.0005);
    }
}
