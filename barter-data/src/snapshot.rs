use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn default_snapshot_version() -> u16 {
    1
}

/// Server-side market snapshot for derived metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketSnapshot {
    #[serde(default = "default_snapshot_version")]
    pub snapshot_version: u16,
    pub timestamp: i64,
    pub tickers: HashMap<String, SnapshotTicker>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SnapshotPerExchangeShort {
    pub cvd_30s: f64,
    pub total_30s: f64,
    pub trades_30s: usize,
}

/// Derived metrics for a single ticker at snapshot time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotTicker {
    pub price: f64,
    pub cvd_5m: f64,
    pub cvd_15m: f64,
    pub rvol_5m: f64,
    pub oi_delta_5m: f64,
    pub funding_rate: f64,
    pub funding_velocity: f64,
    pub liq_rate_usd_per_min: f64,
    pub vol_percentile: f64,
    pub vol_regime: String,
    #[serde(default)]
    pub vol_samples: u16,
    #[serde(default)]
    pub per_exchange_30s: HashMap<String, SnapshotPerExchangeShort>,
}
