//! Snapshot Bridge - Converts AggregatedSnapshot to Orchestrator inputs
//!
//! This module bridges the existing Aggregator/Snapshot infrastructure
//! to the new Market State Engine without modifying existing code.
//!
//! # Design Principles
//! - ADDITIVE only - no modifications to existing structs
//! - Pure functions - no side effects
//! - Fallback values for missing data

use crate::shared::market_state::{
    Direction, FlowScore, FuelInput, FundingScore, TradMarketStatus, VolRegime, VolRegimeScore,
};
use crate::shared::orchestrator::{DataTimestamps, MarketDataInput};
use crate::shared::state::{AggregatedSnapshot, TickerSnapshot};
use crate::shared::types::SnapshotTicker;
use chrono::Utc;

/// Build MarketDataInput from a TickerSnapshot
///
/// This is the primary bridge function that converts existing snapshot data
/// to the format expected by StateOrchestrator.
pub fn build_market_data_input(
    snapshot: &TickerSnapshot,
    server_snapshot: Option<&SnapshotTicker>,
    trad_status: TradMarketStatus,
) -> MarketDataInput {
    let now_ms = Utc::now().timestamp_millis();

    MarketDataInput {
        spot_price: snapshot.binance_perp_last.unwrap_or(0.0),
        ticker: snapshot.ticker.clone(),
        vol_score: build_vol_score(snapshot, server_snapshot),
        options_context: None, // Not available in current infrastructure
        flow_score: build_flow_score(snapshot, server_snapshot),
        funding_score: build_funding_score(snapshot, server_snapshot),
        fuel_input: build_fuel_input(snapshot, server_snapshot),
        timestamps: build_timestamps(snapshot, now_ms),
        trad_status,
    }
}

/// Build VolRegimeScore from snapshot volatility data
fn build_vol_score(snapshot: &TickerSnapshot, server_snapshot: Option<&SnapshotTicker>) -> VolRegimeScore {
    // Use realized volatility if available
    let current_rv = snapshot.realized_vol_1h.unwrap_or(0.0);

    // Estimate percentile from trend (simplified - real implementation would use history)
    let (mut percentile, mut regime) = match snapshot.realized_vol_trend {
        crate::shared::state::VolTrend::Contracting => (30.0, VolRegime::Low),
        crate::shared::state::VolTrend::Stable => (50.0, VolRegime::Normal),
        crate::shared::state::VolTrend::Expanding => (75.0, VolRegime::High),
    };

    if let Some(server) = server_snapshot {
        if server.vol_percentile > 0.0 {
            percentile = server.vol_percentile;
        }
        if let Some(server_regime) = parse_vol_regime(&server.vol_regime) {
            regime = server_regime;
        }
    }

    // Check for shock based on ATR or 1m return Z-score
    let atr_shock = snapshot
        .atr_14_pct
        .map(|atr| atr > 2.0) // >2% ATR is unusual
        .unwrap_or(false);
    let zscore_shock = snapshot.zscore_1m.abs() >= 3.5;
    let is_shock = atr_shock || zscore_shock;

    VolRegimeScore {
        current_rv,
        percentile,
        regime,
        zscore_1m: snapshot.zscore_1m,
        is_shock,
        passed: !is_shock && percentile < 95.0,
    }
}

/// Build FlowScore from CVD data across exchanges
fn build_flow_score(snapshot: &TickerSnapshot, server_snapshot: Option<&SnapshotTicker>) -> FlowScore {
    let cvd_5m = &snapshot.cvd_per_exchange_5m;

    // Count venues with positive/negative CVD
    let mut long_count = 0u8;
    let mut short_count = 0u8;
    let mut total = 0u8;

    for (_exchange, &cvd) in cvd_5m.iter() {
        total += 1;
        if cvd > 0.0 {
            long_count += 1;
        } else if cvd < 0.0 {
            short_count += 1;
        }
    }

    // Determine consensus direction
    let (venues_agreeing, consensus_direction) = if long_count > short_count {
        (long_count, Direction::Long)
    } else if short_count > long_count {
        (short_count, Direction::Short)
    } else {
        (0, Direction::Neutral)
    };

    // Net CVD across all venues
    let cvd_net = server_snapshot
        .map(|server| server.cvd_5m)
        .unwrap_or(snapshot.cvd_5m_total);

    // Use flow signal for absorption detection
    // Distribution = selling into strength, Exhaustion = momentum fading
    // Both can indicate absorption-like behavior
    let absorption_detected = matches!(
        snapshot.flow_signal,
        crate::shared::state::FlowSignal::Distribution | crate::shared::state::FlowSignal::Exhaustion
    );

    // Calculate pass criteria (2/3 consensus)
    let consensus_pct = if total > 0 {
        (venues_agreeing as f64 / total as f64) * 100.0
    } else {
        0.0
    };
    let passed = consensus_pct >= 66.0;

    FlowScore {
        venues_agreeing,
        venues_total: total,
        consensus_direction,
        cvd_net,
        absorption_detected,
        passed,
    }
}

/// Build FundingScore from exchange funding rates
fn build_funding_score(snapshot: &TickerSnapshot, server_snapshot: Option<&SnapshotTicker>) -> FundingScore {
    let (current_rate, rate_15m_ago, velocity) = if let Some(server) = server_snapshot {
        let rate_15m_ago = server.funding_rate - server.funding_velocity;
        (server.funding_rate, rate_15m_ago, server.funding_velocity)
    } else {
        let rates = &snapshot.funding_rate_by_exchange;

        // Calculate average funding rate
        let (sum, count) = rates.iter().fold((0.0, 0), |(sum, count), (_, &rate)| {
            (sum + rate, count + 1)
        });

        let current_rate = if count > 0 { sum / count as f64 } else { 0.0 };
        (current_rate, snapshot.funding_rate_15m_ago, snapshot.funding_velocity_15m)
    };

    // Thresholds from spec
    let is_extreme = current_rate > 0.0005 || current_rate < -0.0002;
    let is_spiking = false; // Threshold applied in MarketState with per-ticker config

    FundingScore {
        current_rate,
        rate_15m_ago,
        velocity,
        is_extreme,
        is_spiking,
        passed: !is_spiking,
    }
}

fn build_fuel_input(snapshot: &TickerSnapshot, server_snapshot: Option<&SnapshotTicker>) -> FuelInput {
    if let Some(server) = server_snapshot {
        return FuelInput {
            rvol_5m: server.rvol_5m,
            oi_delta_usd_5m: server.oi_delta_5m,
            liq_rate_usd_per_min: server.liq_rate_usd_per_min,
        };
    }

    let spot = snapshot.binance_perp_last.or(snapshot.latest_price).unwrap_or(0.0);
    let avg_5m = if snapshot.vol_1h > 0.0 {
        snapshot.vol_1h / 12.0
    } else {
        0.0
    };
    let rvol_5m = if avg_5m > 0.0 {
        snapshot.vol_5m / avg_5m
    } else {
        0.0
    };
    let oi_delta_usd_5m = if spot > 0.0 {
        snapshot.oi_delta_5m * spot
    } else {
        0.0
    };

    FuelInput {
        rvol_5m,
        oi_delta_usd_5m,
        liq_rate_usd_per_min: snapshot.liq_rate_per_min,
    }
}

/// Build DataTimestamps from exchange health data
fn build_timestamps(snapshot: &TickerSnapshot, now_ms: i64) -> DataTimestamps {
    // Prefer actual trade lag if available; otherwise use freshest exchange age.
    let trade_lag_ms = snapshot
        .trade_lag_secs
        .map(|secs| (secs.max(0.0) * 1000.0) as i64);
    let min_exchange_age_ms = snapshot
        .exchange_health
        .values()
        .fold(None::<i64>, |min, &secs| {
            let ms = (secs.max(0.0) * 1000.0) as i64;
            Some(min.map_or(ms, |cur| cur.min(ms)))
        });
    let data_age_ms = match (trade_lag_ms, min_exchange_age_ms) {
        (Some(trade_ms), Some(ex_ms)) => Some(trade_ms.min(ex_ms)),
        (Some(trade_ms), None) => Some(trade_ms),
        (None, Some(ex_ms)) => Some(ex_ms),
        (None, None) => None,
    };
    let data_ts = data_age_ms.map(|age| now_ms - age).unwrap_or(0);

    DataTimestamps {
        price_ts: data_ts,
        cvd_ts: data_ts,
        l2_book_ts: data_ts,
        whale_ts: if snapshot.whales.is_empty() {
            now_ms - 60000 // Assume stale if no whales
        } else {
            data_ts
        },
        funding_ts: data_ts, // Funding updates less frequently
        gamma_ts: 0,         // No gamma data from snapshot
        trad_markets_ts: 0,  // Set by caller based on IBKR status
    }
}

/// Build inputs for all tickers in a snapshot
pub fn build_all_inputs(
    snapshot: &AggregatedSnapshot,
    trad_status: TradMarketStatus,
) -> Vec<MarketDataInput> {
    let server_snapshot = snapshot.server_snapshot.as_ref();
    snapshot
        .tickers
        .values()
        .map(|ticker_snap| {
            let server_ticker = server_snapshot
                .and_then(|snap| snap.tickers.get(&ticker_snap.ticker));
            build_market_data_input(ticker_snap, server_ticker, trad_status)
        })
        .collect()
}

fn parse_vol_regime(value: &str) -> Option<VolRegime> {
    match value.to_ascii_lowercase().as_str() {
        "low" => Some(VolRegime::Low),
        "normal" => Some(VolRegime::Normal),
        "high" => Some(VolRegime::High),
        "extreme" => Some(VolRegime::Extreme),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::state::{FlowSignal, VolTrend};
    use std::collections::HashMap;

    fn sample_snapshot() -> TickerSnapshot {
        let mut snap = TickerSnapshot::default();
        snap.ticker = "BTC".to_string();
        snap.binance_perp_last = Some(92000.0);
        snap.realized_vol_1h = Some(0.02);
        snap.realized_vol_trend = VolTrend::Stable;
        snap.atr_14_pct = Some(0.5);
        snap.cvd_5m_total = 500000.0;
        snap.cvd_per_exchange_5m = {
            let mut m = HashMap::new();
            m.insert("binance".to_string(), 300000.0);
            m.insert("bybit".to_string(), 150000.0);
            m.insert("okx".to_string(), 50000.0);
            m
        };
        snap.funding_rate_by_exchange = {
            let mut m = HashMap::new();
            m.insert("binance".to_string(), 0.0001);
            m.insert("bybit".to_string(), 0.00012);
            m
        };
        snap.exchange_health = {
            let mut m = HashMap::new();
            m.insert("binance".to_string(), 0.5);
            m.insert("bybit".to_string(), 0.8);
            m
        };
        snap.flow_signal = FlowSignal::Neutral;
        snap
    }

    #[test]
    fn test_build_market_data_input() {
        let snap = sample_snapshot();
        let input = build_market_data_input(&snap, None, TradMarketStatus::Live);

        assert_eq!(input.ticker, "BTC");
        assert_eq!(input.spot_price, 92000.0);
        assert_eq!(input.trad_status, TradMarketStatus::Live);
    }

    #[test]
    fn test_build_vol_score_stable() {
        let snap = sample_snapshot();
        let vol = build_vol_score(&snap, None);

        assert_eq!(vol.regime, VolRegime::Normal);
        assert!(!vol.is_shock);
        assert!(vol.passed);
    }

    #[test]
    fn test_build_vol_score_expanding() {
        let mut snap = sample_snapshot();
        snap.realized_vol_trend = VolTrend::Expanding;
        let vol = build_vol_score(&snap, None);

        assert_eq!(vol.regime, VolRegime::High);
    }

    #[test]
    fn test_build_flow_score_consensus() {
        let snap = sample_snapshot();
        let flow = build_flow_score(&snap, None);

        // All 3 exchanges have positive CVD
        assert_eq!(flow.venues_total, 3);
        assert_eq!(flow.venues_agreeing, 3);
        assert_eq!(flow.consensus_direction, Direction::Long);
        assert!(flow.passed);
    }

    #[test]
    fn test_build_flow_score_mixed() {
        let mut snap = sample_snapshot();
        snap.cvd_per_exchange_5m.insert("okx".to_string(), -100000.0);
        let flow = build_flow_score(&snap, None);

        // 2 positive, 1 negative = 2/3 consensus
        assert_eq!(flow.venues_agreeing, 2);
        assert_eq!(flow.consensus_direction, Direction::Long);
        assert!(flow.passed); // 66% threshold met
    }

    #[test]
    fn test_build_flow_score_no_consensus() {
        let mut snap = sample_snapshot();
        snap.cvd_per_exchange_5m = {
            let mut m = HashMap::new();
            m.insert("binance".to_string(), 100000.0);
            m.insert("bybit".to_string(), -150000.0);
            m.insert("okx".to_string(), -50000.0);
            m
        };
        let flow = build_flow_score(&snap, None);

        // 1 positive, 2 negative
        assert_eq!(flow.venues_agreeing, 2);
        assert_eq!(flow.consensus_direction, Direction::Short);
    }

    #[test]
    fn test_build_funding_score() {
        let snap = sample_snapshot();
        let funding = build_funding_score(&snap, None);

        // Average of 0.0001 and 0.00012 = 0.00011
        assert!((funding.current_rate - 0.00011).abs() < 0.00001);
        assert!(!funding.is_extreme);
        assert!(funding.passed);
    }

    #[test]
    fn test_build_timestamps_fresh() {
        let snap = sample_snapshot();
        let now = Utc::now().timestamp_millis();
        let ts = build_timestamps(&snap, now);

        // Max staleness is 0.8 seconds, so timestamps should be close to now
        assert!(ts.price_ts > now - 2000);
        assert!(ts.cvd_ts > now - 2000);
    }

    #[test]
    fn test_build_all_inputs() {
        let mut agg_snap = AggregatedSnapshot {
            tickers: HashMap::new(),
            correlation: [[0.0; 3]; 3],
            server_snapshot: None,
        };
        agg_snap
            .tickers
            .insert("BTC".to_string(), sample_snapshot());

        let mut eth_snap = sample_snapshot();
        eth_snap.ticker = "ETH".to_string();
        agg_snap.tickers.insert("ETH".to_string(), eth_snap);

        let inputs = build_all_inputs(&agg_snap, TradMarketStatus::Live);
        assert_eq!(inputs.len(), 2);
    }

    #[test]
    fn test_absorption_detection_distribution() {
        let mut snap = sample_snapshot();
        snap.flow_signal = FlowSignal::Distribution;
        let flow = build_flow_score(&snap, None);

        assert!(flow.absorption_detected);
    }

    #[test]
    fn test_absorption_detection_exhaustion() {
        let mut snap = sample_snapshot();
        snap.flow_signal = FlowSignal::Exhaustion;
        let flow = build_flow_score(&snap, None);

        assert!(flow.absorption_detected);
    }

    #[test]
    fn test_no_absorption_neutral() {
        let snap = sample_snapshot();
        let flow = build_flow_score(&snap, None);

        assert!(!flow.absorption_detected);
    }
}
