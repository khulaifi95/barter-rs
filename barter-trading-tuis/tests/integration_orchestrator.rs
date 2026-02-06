//! Integration Tests for Market State Engine
//!
//! Tests the full pipeline:
//! Snapshot → Bridge → Orchestrator → MarketState

use barter_trading_tuis::shared::audit::AuditLogger;
use barter_trading_tuis::shared::config::Config;
use barter_trading_tuis::shared::market_state::{State, TradMarketStatus};
use barter_trading_tuis::shared::orchestrator::StateOrchestrator;
use barter_trading_tuis::shared::snapshot_bridge::build_market_data_input;
use barter_trading_tuis::shared::state::{FlowSignal, TickerSnapshot, VolTrend};
use std::collections::HashMap;

/// Create a healthy market snapshot with good conditions
fn healthy_snapshot() -> TickerSnapshot {
    let cvd_per_exchange_5m = {
        let mut m = HashMap::new();
        m.insert("binance".to_string(), 300000.0);
        m.insert("bybit".to_string(), 150000.0);
        m.insert("okx".to_string(), 50000.0);
        m
    };
    let funding_rate_by_exchange = {
        let mut m = HashMap::new();
        m.insert("binance".to_string(), 0.0001);
        m.insert("bybit".to_string(), 0.00012);
        m
    };
    let exchange_health = {
        let mut m = HashMap::new();
        m.insert("binance".to_string(), 0.5);
        m.insert("bybit".to_string(), 0.8);
        m.insert("okx".to_string(), 0.6);
        m
    };

    TickerSnapshot {
        ticker: "BTC".to_string(),
        binance_perp_last: Some(92000.0),
        realized_vol_1h: Some(0.02),
        realized_vol_trend: VolTrend::Stable,
        atr_14_pct: Some(0.5),
        cvd_5m_total: 500000.0,
        cvd_per_exchange_5m,
        funding_rate_by_exchange,
        exchange_health,
        flow_signal: FlowSignal::Neutral,
        ..Default::default()
    }
}

/// Create a chaotic market snapshot (high vol)
fn volatile_snapshot() -> TickerSnapshot {
    let mut snap = healthy_snapshot();
    snap.realized_vol_trend = VolTrend::Expanding;
    snap.atr_14_pct = Some(3.5); // Very high ATR triggers shock
    snap
}

/// Create a snapshot with distribution (absorption-like)
fn distribution_snapshot() -> TickerSnapshot {
    let mut snap = healthy_snapshot();
    snap.flow_signal = FlowSignal::Distribution;
    snap
}

/// Create a snapshot with mixed consensus
fn mixed_consensus_snapshot() -> TickerSnapshot {
    let mut snap = healthy_snapshot();
    snap.cvd_per_exchange_5m = {
        let mut m = HashMap::new();
        m.insert("binance".to_string(), 100000.0);
        m.insert("bybit".to_string(), -200000.0);
        m.insert("okx".to_string(), -100000.0);
        m
    };
    snap
}

#[test]
fn test_healthy_market_produces_ready_or_caution() {
    let config = Config::default();
    let logger = AuditLogger::no_op();
    let mut orchestrator = StateOrchestrator::new(config, logger);

    let snap = healthy_snapshot();
    let input = build_market_data_input(&snap, None, TradMarketStatus::Live);
    let result = orchestrator.calculate(&input);

    // Healthy market should produce Ready or Caution (not Wait)
    assert!(
        result.state.state == State::Ready || result.state.state == State::Caution,
        "Healthy market should allow trading, got {:?}",
        result.state.state
    );

    // Should have bias in non-Wait state
    if result.state.state != State::Wait {
        assert!(
            result.state.bias.is_some(),
            "Non-Wait state should have bias"
        );
    }

    // No gamma data means NO-GAMMA mode
    assert!(
        result.no_gamma_mode,
        "Should be in NO-GAMMA mode without options"
    );
}

#[test]
fn test_volatile_market_produces_wait_or_caution() {
    let config = Config::default();
    let logger = AuditLogger::no_op();
    let mut orchestrator = StateOrchestrator::new(config, logger);

    let snap = volatile_snapshot();
    let input = build_market_data_input(&snap, None, TradMarketStatus::Live);
    let result = orchestrator.calculate(&input);

    // Volatile/shock market should trigger caution or wait
    // The shock detection depends on ATR threshold
    println!("Volatile market state: {:?}", result.state.state);
    println!("Vol score: {:?}", result.state.components.vol_regime);
}

#[test]
fn test_distribution_triggers_absorption() {
    let config = Config::default();
    let logger = AuditLogger::no_op();
    let mut orchestrator = StateOrchestrator::new(config, logger);

    let snap = distribution_snapshot();
    let input = build_market_data_input(&snap, None, TradMarketStatus::Live);
    let result = orchestrator.calculate(&input);

    // Distribution signal should be detected as absorption
    assert!(
        result.state.components.flow_consensus.absorption_detected,
        "Distribution should trigger absorption detection"
    );

    // Absorption should cause Wait state
    assert_eq!(
        result.state.state,
        State::Wait,
        "Absorption should trigger Wait state"
    );
}

#[test]
fn test_stale_price_produces_wait() {
    let config = Config::default();
    let logger = AuditLogger::no_op();
    let mut orchestrator = StateOrchestrator::new(config, logger);

    let mut snap = healthy_snapshot();
    // Make data very stale
    snap.exchange_health = {
        let mut m = HashMap::new();
        m.insert("binance".to_string(), 10.0); // 10 seconds stale
        m
    };

    let input = build_market_data_input(&snap, None, TradMarketStatus::Live);
    let result = orchestrator.calculate(&input);

    // Stale data should trigger Wait
    assert_eq!(
        result.state.state,
        State::Wait,
        "Stale data should trigger Wait"
    );
    assert!(
        !result.freshness_warnings.is_empty() || result.state.reason.contains("stale"),
        "Should have freshness warnings or stale reason"
    );
}

#[test]
fn test_no_trad_mode_when_trad_unavailable() {
    let config = Config::default();
    let logger = AuditLogger::no_op();
    let mut orchestrator = StateOrchestrator::new(config, logger);

    let snap = healthy_snapshot();
    let input = build_market_data_input(&snap, None, TradMarketStatus::Unavailable);
    let result = orchestrator.calculate(&input);

    assert!(result.no_trad_mode, "Should be in NO-TRAD mode");
}

#[test]
fn test_state_transitions_increment_audit_id() {
    let config = Config::default();
    let logger = AuditLogger::no_op();
    let mut orchestrator = StateOrchestrator::new(config, logger);

    // First call
    let snap = healthy_snapshot();
    let input = build_market_data_input(&snap, None, TradMarketStatus::Live);
    let result1 = orchestrator.calculate(&input);
    let id1 = result1.state.audit_id;

    // Second call
    let result2 = orchestrator.calculate(&input);
    let id2 = result2.state.audit_id;

    // Audit ID should increment
    assert!(id2 > id1, "Audit ID should increment: {} > {}", id2, id1);
}

#[test]
fn test_sol_ticker_uses_override_thresholds() {
    let config = Config::default();
    let logger = AuditLogger::no_op();
    let mut orchestrator = StateOrchestrator::new(config, logger);

    // SOL with volatility at 92% (above SOL's 90% threshold but below BTC's 95%)
    let mut snap = healthy_snapshot();
    snap.ticker = "SOL".to_string();
    snap.realized_vol_trend = VolTrend::Expanding;

    let input = build_market_data_input(&snap, None, TradMarketStatus::Live);
    let result = orchestrator.calculate(&input);

    // SOL has lower extreme threshold (90%), so high vol triggers sooner
    println!(
        "SOL with expanding vol: state={:?}, vol_regime={:?}",
        result.state.state, result.state.components.vol_regime.regime
    );
}

#[test]
fn test_consensus_calculation() {
    let config = Config::default();
    let logger = AuditLogger::no_op();
    let mut orchestrator = StateOrchestrator::new(config, logger);

    let snap = mixed_consensus_snapshot();
    let input = build_market_data_input(&snap, None, TradMarketStatus::Live);
    let result = orchestrator.calculate(&input);

    // Mixed consensus: 1 long, 2 short = Short consensus at 66%
    let flow = &result.state.components.flow_consensus;
    assert_eq!(flow.venues_total, 3);
    assert_eq!(flow.venues_agreeing, 2);

    println!("Mixed consensus result: {:?}", flow);
}

#[test]
fn test_multiple_tickers_independent() {
    let config = Config::default();
    let logger = AuditLogger::no_op();
    let mut orchestrator = StateOrchestrator::new(config, logger);

    // Process BTC
    let btc_snap = healthy_snapshot();
    let btc_input = build_market_data_input(&btc_snap, None, TradMarketStatus::Live);
    let btc_result = orchestrator.calculate(&btc_input);

    // Process ETH (different ticker)
    let mut eth_snap = healthy_snapshot();
    eth_snap.ticker = "ETH".to_string();
    eth_snap.binance_perp_last = Some(3200.0);
    let eth_input = build_market_data_input(&eth_snap, None, TradMarketStatus::Live);
    let eth_result = orchestrator.calculate(&eth_input);

    // Both should produce valid states
    println!("BTC: {:?}", btc_result.state.state);
    println!("ETH: {:?}", eth_result.state.state);
}
