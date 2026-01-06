use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Server-side market snapshot for derived metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketSnapshot {
    pub timestamp: i64,
    pub tickers: HashMap<String, SnapshotTicker>,
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
}
