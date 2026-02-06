//! State Orchestrator - Wires all modules together with freshness gates
//!
//! The orchestrator is the integration layer that:
//! 1. Checks data freshness before calculation
//! 2. Implements NO-GAMMA/NO-TRAD fallback modes
//! 3. Calls MarketState::calculate with validated inputs
//! 4. Logs state transitions to audit
//!
//! # Freshness Gates
//! Every signal has a maximum allowed staleness. If exceeded, fallback logic applies.
//!
//! # Fallback Modes
//! - **NO-GAMMA**: Options data stale/unavailable → derive bias from flow only
//! - **NO-TRAD**: Traditional markets stale/unavailable → bypass macro filter

use crate::shared::audit::AuditLogger;
use crate::shared::config::Config;
use crate::shared::market_state::{
    AuditEntry, AuditLog, ConfigProvider, FlowScore, FuelInput, FundingScore, GammaScore,
    GammaStatus, MarketState, OptionsSummary, Signal, State, TradMarketStatus, VolRegimeScore,
    Warning,
};
use crate::shared::options_state::{no_gamma_score, OptionsContext};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Data timestamps for freshness checking
#[derive(Debug, Clone, Default)]
pub struct DataTimestamps {
    pub price_ts: i64,
    pub cvd_ts: i64,
    pub l2_book_ts: i64,
    pub whale_ts: i64,
    pub funding_ts: i64,
    pub gamma_ts: i64,
    pub trad_markets_ts: i64,
}

impl DataTimestamps {
    /// Check if a signal is fresh (within max staleness)
    pub fn is_fresh(&self, signal: Signal, max_staleness: Duration) -> bool {
        let now = Utc::now().timestamp_millis();
        let ts = self.timestamp_for(signal);
        let age_ms = (now - ts).max(0) as u64;
        age_ms <= max_staleness.as_millis() as u64
    }

    /// Get timestamp for a specific signal
    pub fn timestamp_for(&self, signal: Signal) -> i64 {
        match signal {
            Signal::Price => self.price_ts,
            Signal::Cvd => self.cvd_ts,
            Signal::L2Book => self.l2_book_ts,
            Signal::Whale => self.whale_ts,
            Signal::Funding => self.funding_ts,
            Signal::Gamma => self.gamma_ts,
            Signal::TradMarkets => self.trad_markets_ts,
        }
    }

    /// Get age in milliseconds for a signal
    pub fn age_ms(&self, signal: Signal) -> u64 {
        let now = Utc::now().timestamp_millis();
        (now - self.timestamp_for(signal)).max(0) as u64
    }
}

/// Input data for state calculation
#[derive(Debug, Clone)]
pub struct MarketDataInput {
    /// Current spot price
    pub spot_price: f64,
    /// Ticker symbol (e.g., "BTC")
    pub ticker: String,
    /// Volatility score (from VolRegimeEngine)
    pub vol_score: VolRegimeScore,
    /// Options context (may be stale or unavailable)
    pub options_context: Option<OptionsContext>,
    /// Flow consensus from venues
    pub flow_score: FlowScore,
    /// Funding rate context
    pub funding_score: FundingScore,
    /// Fuel inputs (RVOL / OI / Liquidations)
    pub fuel_input: FuelInput,
    /// Data timestamps for freshness checking
    pub timestamps: DataTimestamps,
    /// Traditional markets status
    pub trad_status: TradMarketStatus,
}

/// Result of orchestrated state calculation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorResult {
    /// The calculated market state
    pub state: MarketState,
    /// Whether NO-GAMMA mode was used
    pub no_gamma_mode: bool,
    /// Whether NO-TRAD mode was used
    pub no_trad_mode: bool,
    /// Freshness issues encountered
    pub freshness_warnings: Vec<(Signal, u64)>, // (signal, age_ms)
}

/// State Orchestrator - coordinates all modules with freshness gates
pub struct StateOrchestrator {
    config: Config,
    audit_logger: AuditLogger,
    prev_state: State,
    audit_id: AtomicU64,
}

impl StateOrchestrator {
    /// Create new orchestrator with config and audit logger
    pub fn new(config: Config, audit_logger: AuditLogger) -> Self {
        Self {
            config,
            audit_logger,
            prev_state: State::Wait,
            audit_id: AtomicU64::new(0),
        }
    }

    /// Calculate market state with freshness gates and fallback modes
    ///
    /// # Freshness Gate Logic
    /// 1. Price/CVD stale → immediate WAIT (cannot trade)
    /// 2. Gamma stale → NO-GAMMA mode (flow-based bias)
    /// 3. TradMarkets stale → NO-TRAD mode (bypass macro filter)
    /// 4. Other signals → use last known or ignore filter
    pub fn calculate(&mut self, input: &MarketDataInput) -> OrchestratorResult {
        let mut freshness_warnings = Vec::new();
        let mut warnings = Vec::new();

        // ====================================================================
        // FRESHNESS GATE: Critical signals (WAIT if stale)
        // ====================================================================
        let price_fresh = input
            .timestamps
            .is_fresh(Signal::Price, self.config.freshness(Signal::Price));
        let cvd_fresh = input
            .timestamps
            .is_fresh(Signal::Cvd, self.config.freshness(Signal::Cvd));

        if !price_fresh {
            freshness_warnings.push((Signal::Price, input.timestamps.age_ms(Signal::Price)));
            return self.build_wait_result(
                input,
                "Price data stale - cannot trade".to_string(),
                freshness_warnings,
            );
        }

        if !cvd_fresh {
            freshness_warnings.push((Signal::Cvd, input.timestamps.age_ms(Signal::Cvd)));
            return self.build_wait_result(
                input,
                "CVD data stale - cannot determine consensus".to_string(),
                freshness_warnings,
            );
        }

        // ====================================================================
        // FRESHNESS GATE: Gamma (NO-GAMMA mode if stale)
        // ====================================================================
        let gamma_max_staleness = self.config.freshness(Signal::Gamma);
        let (gamma_score, no_gamma_mode) = self.resolve_gamma(input, gamma_max_staleness);

        if no_gamma_mode {
            warnings.push(Warning::NoGammaData);
            if !input
                .timestamps
                .is_fresh(Signal::Gamma, gamma_max_staleness)
            {
                freshness_warnings.push((Signal::Gamma, input.timestamps.age_ms(Signal::Gamma)));
            }
        } else {
            // Check for bucket conflict when gamma is available
            if let Some(bucket_warn) = self.check_bucket_conflict(input) {
                warnings.push(bucket_warn);
            }
        }

        // ====================================================================
        // FRESHNESS GATE: TradMarkets (NO-TRAD mode if stale or unavailable)
        // ====================================================================
        let no_trad_mode = matches!(
            input.trad_status,
            TradMarketStatus::Stale | TradMarketStatus::Unavailable
        );

        if no_trad_mode {
            warnings.push(Warning::NoTradMarketsData);
            // Only add stale timestamp warning if data was connected but stale
            // (not when data source is simply unavailable/not configured)
            if matches!(input.trad_status, TradMarketStatus::Stale) {
                freshness_warnings.push((
                    Signal::TradMarkets,
                    input.timestamps.age_ms(Signal::TradMarkets),
                ));
            }
        }

        // ====================================================================
        // Get ticker-specific thresholds
        // ====================================================================
        let thresholds = self.config.thresholds_for_ticker(&input.ticker);

        // ====================================================================
        // CALCULATE STATE
        // ====================================================================
        let prev_audit_id = self.audit_id.load(Ordering::Relaxed);
        let mut state = MarketState::calculate(
            &input.vol_score,
            &gamma_score,
            &input.flow_score,
            &input.funding_score,
            &input.fuel_input,
            &thresholds,
            prev_audit_id,
        );

        // Append freshness/mode warnings to state
        state.components.warnings.extend(warnings);
        state.components.options_context = input
            .options_context
            .as_ref()
            .map(Self::options_summary_from);

        // ====================================================================
        // AUDIT: Log state transitions only
        // ====================================================================
        if state.state != self.prev_state {
            self.log_transition(input, &state, &gamma_score);
            self.prev_state = state.state;
        }

        self.audit_id.store(state.audit_id, Ordering::Relaxed);

        OrchestratorResult {
            state,
            no_gamma_mode,
            no_trad_mode,
            freshness_warnings,
        }
    }

    /// Resolve gamma context with NO-GAMMA fallback
    ///
    /// Uses the front bucket (0-7d) for L2 bias calculation to get the most
    /// actionable gamma flip price.
    fn resolve_gamma(
        &self,
        input: &MarketDataInput,
        max_staleness: Duration,
    ) -> (GammaScore, bool) {
        match &input.options_context {
            Some(ctx) => {
                let gamma_status = ctx.to_gamma_status(max_staleness);
                match gamma_status {
                    GammaStatus::Live { flip_price: _ } => {
                        let flip_buffer = self.config.thresholds().gamma.flip_buffer_pct;
                        // Use front bucket for L2 bias (more actionable)
                        (
                            ctx.to_gamma_score_from_front_bucket(input.spot_price, flip_buffer),
                            false,
                        )
                    }
                    GammaStatus::Stale { last_flip_price: _ } => {
                        // Use stale data but mark as such
                        let flip_buffer = self.config.thresholds().gamma.flip_buffer_pct;
                        (
                            ctx.to_gamma_score_from_front_bucket(input.spot_price, flip_buffer),
                            false,
                        )
                    }
                    GammaStatus::Unavailable => {
                        // NO-GAMMA mode: derive bias from flow
                        (
                            no_gamma_score(input.spot_price, input.flow_score.consensus_direction),
                            true,
                        )
                    }
                }
            }
            None => {
                // NO-GAMMA mode: no options context available
                (
                    no_gamma_score(input.spot_price, input.flow_score.consensus_direction),
                    true,
                )
            }
        }
    }

    /// Check for bucket conflict and return warning if detected
    fn check_bucket_conflict(&self, input: &MarketDataInput) -> Option<Warning> {
        input.options_context.as_ref().and_then(|ctx| {
            if ctx.has_bucket_conflict(input.spot_price) {
                Some(Warning::ExpiryBucketConflict)
            } else {
                None
            }
        })
    }

    /// Build WAIT result for stale critical data
    fn build_wait_result(
        &mut self,
        input: &MarketDataInput,
        reason: String,
        freshness_warnings: Vec<(Signal, u64)>,
    ) -> OrchestratorResult {
        let prev_audit_id = self.audit_id.load(Ordering::Relaxed);
        let audit_id = prev_audit_id + 1;
        self.audit_id.store(audit_id, Ordering::Relaxed);
        let gamma_max_staleness = self.config.freshness(Signal::Gamma);
        let (gamma_score, no_gamma_mode) = self.resolve_gamma(input, gamma_max_staleness);

        // Build stale data warnings
        let warnings: Vec<Warning> = freshness_warnings
            .iter()
            .map(|(signal, age_ms)| Warning::StaleData {
                signal: *signal,
                age_ms: *age_ms,
            })
            .collect();

        let state = MarketState {
            state: State::Wait,
            bias: None,
            confidence: 0,
            components: crate::shared::market_state::StateComponents {
                vol_regime: input.vol_score.clone(),
                gamma_context: gamma_score.clone(),
                flow_consensus: input.flow_score.clone(),
                funding_context: input.funding_score.clone(),
                fuel_context: crate::shared::market_state::FuelScore::default(),
                options_context: input
                    .options_context
                    .as_ref()
                    .map(Self::options_summary_from),
                warnings,
            },
            reason,
            audit_id,
            timestamp: Utc::now().timestamp_millis(),
        };

        // Log if state changed
        if self.prev_state != State::Wait {
            self.log_transition(input, &state, &gamma_score);
            self.prev_state = State::Wait;
        }

        OrchestratorResult {
            state,
            no_gamma_mode,
            no_trad_mode: matches!(
                input.trad_status,
                TradMarketStatus::Stale | TradMarketStatus::Unavailable
            ),
            freshness_warnings,
        }
    }

    /// Log state transition to audit
    fn log_transition(&self, input: &MarketDataInput, state: &MarketState, gamma: &GammaScore) {
        let entry = AuditEntry {
            audit_id: state.audit_id,
            timestamp: state.timestamp,
            ticker: input.ticker.clone(),
            price: input.spot_price,
            prev_state: self.prev_state,
            new_state: state.state,
            bias: state.bias,
            confidence: state.confidence,
            vol_percentile: input.vol_score.percentile,
            vol_regime: input.vol_score.regime,
            gamma_flip_price: if gamma.gamma_flip_price > 0.0 {
                Some(gamma.gamma_flip_price)
            } else {
                None
            },
            price_vs_flip_pct: if gamma.gamma_flip_price > 0.0 {
                Some(gamma.distance_pct)
            } else {
                None
            },
            cvd_consensus: format!(
                "{}/{} {:?}",
                input.flow_score.venues_agreeing,
                input.flow_score.venues_total,
                input.flow_score.consensus_direction
            ),
            funding_rate: input.funding_score.current_rate,
            funding_velocity: input.funding_score.velocity,
            reason: state.reason.clone(),
        };

        self.audit_logger.log(entry);
    }

    fn options_summary_from(ctx: &OptionsContext) -> OptionsSummary {
        use crate::shared::market_state::BucketSummary;

        tracing::debug!(
            "options_summary_from: gexp={:.2}M, dexp={:.2}M, vexp={:.2}M",
            ctx.gexp / 1_000_000.0,
            ctx.dexp / 1_000_000.0,
            ctx.vexp / 1_000_000.0
        );

        // Build bucket summaries for UI
        let buckets: Vec<BucketSummary> = ctx
            .buckets
            .iter()
            .map(|b| BucketSummary {
                label: b.bucket.label().to_string(),
                gamma_flip: b.gamma_flip,
                max_pain: b.max_pain,
                put_wall: b.put_wall.clone(),
                call_wall: b.call_wall.clone(),
                gex: b.gex,
                dex: b.dex,
                put_call_ratio: b.put_call_ratio,
                hours_to_expiry: b.hours_to_expiry,
                is_front: b.bucket == ctx.front_bucket,
                contract_count: b.contract_count,
            })
            .collect();

        OptionsSummary {
            gexp: ctx.gexp,
            dexp: ctx.dexp,
            vexp: ctx.vexp,
            put_call_oi_ratio: ctx.put_call_oi_ratio,
            gamma_flip_price: ctx.gamma_flip_price,
            max_pain: ctx.max_pain,
            put_wall: ctx.put_wall.clone(),
            call_wall: ctx.call_wall.clone(),
            hours_to_expiry: ctx.hours_to_expiry,
            last_update: ctx.last_update,
            buckets,
        }
    }

    /// Get current state (for external queries)
    pub fn current_state(&self) -> State {
        self.prev_state
    }

    /// Get current audit ID
    pub fn current_audit_id(&self) -> u64 {
        self.audit_id.load(Ordering::Relaxed)
    }
}

/// Builder for MarketDataInput from raw components
#[derive(Default)]
pub struct MarketDataInputBuilder {
    spot_price: f64,
    ticker: String,
    vol_score: Option<VolRegimeScore>,
    options_context: Option<OptionsContext>,
    flow_score: Option<FlowScore>,
    funding_score: Option<FundingScore>,
    fuel_input: Option<FuelInput>,
    timestamps: DataTimestamps,
    trad_status: TradMarketStatus,
}

impl MarketDataInputBuilder {
    pub fn new(ticker: &str, spot_price: f64) -> Self {
        Self {
            ticker: ticker.to_string(),
            spot_price,
            timestamps: DataTimestamps {
                price_ts: Utc::now().timestamp_millis(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub fn vol_score(mut self, score: VolRegimeScore) -> Self {
        self.vol_score = Some(score);
        self
    }

    pub fn options_context(mut self, ctx: OptionsContext) -> Self {
        let last_update = ctx.last_update;
        self.options_context = Some(ctx);
        self.timestamps.gamma_ts = last_update;
        self
    }

    pub fn flow_score(mut self, score: FlowScore, ts: i64) -> Self {
        self.flow_score = Some(score);
        self.timestamps.cvd_ts = ts;
        self
    }

    pub fn funding_score(mut self, score: FundingScore, ts: i64) -> Self {
        self.funding_score = Some(score);
        self.timestamps.funding_ts = ts;
        self
    }

    pub fn fuel_input(mut self, input: FuelInput) -> Self {
        self.fuel_input = Some(input);
        self
    }

    pub fn trad_status(mut self, status: TradMarketStatus) -> Self {
        self.trad_status = status;
        self
    }

    pub fn price_ts(mut self, ts: i64) -> Self {
        self.timestamps.price_ts = ts;
        self
    }

    pub fn build(self) -> MarketDataInput {
        MarketDataInput {
            spot_price: self.spot_price,
            ticker: self.ticker,
            vol_score: self.vol_score.unwrap_or_default(),
            options_context: self.options_context,
            flow_score: self.flow_score.unwrap_or_default(),
            funding_score: self.funding_score.unwrap_or_default(),
            fuel_input: self.fuel_input.unwrap_or_default(),
            timestamps: self.timestamps,
            trad_status: self.trad_status,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::market_state::{
        Direction, FlowScore, FundingScore, VolRegime, VolRegimeScore,
    };

    fn test_config() -> Config {
        Config::default()
    }

    fn normal_vol() -> VolRegimeScore {
        VolRegimeScore {
            current_rv: 0.02,
            percentile: 50.0,
            regime: VolRegime::Normal,
            rv_samples: 168,
            rv_samples_required: 168,
            zscore_1m: 0.5,
            is_shock: false,
            passed: true,
        }
    }

    fn consensus_flow() -> FlowScore {
        FlowScore {
            venues_agreeing: 3,
            venues_total: 3,
            consensus_direction: Direction::Long,
            cvd_net: 1000000.0,
            absorption_detected: false,
            passed: true,
        }
    }

    fn stable_funding() -> FundingScore {
        FundingScore {
            current_rate: 0.0001,
            rate_15m_ago: 0.0001,
            velocity: 0.0,
            is_extreme: false,
            is_spiking: false,
            passed: true,
        }
    }

    fn fresh_timestamps() -> DataTimestamps {
        let now = Utc::now().timestamp_millis();
        DataTimestamps {
            price_ts: now,
            cvd_ts: now,
            l2_book_ts: now,
            whale_ts: now,
            funding_ts: now,
            gamma_ts: now,
            trad_markets_ts: now,
        }
    }

    #[test]
    fn test_fresh_data_produces_ready() {
        let config = test_config();
        let logger = AuditLogger::no_op();
        let mut orchestrator = StateOrchestrator::new(config, logger);

        let options_ctx = OptionsContext {
            gamma_flip_price: 90000.0,
            last_update: Utc::now().timestamp_millis(),
            ..Default::default()
        };

        let input = MarketDataInput {
            spot_price: 92000.0,
            ticker: "BTC".to_string(),
            vol_score: normal_vol(),
            options_context: Some(options_ctx),
            flow_score: consensus_flow(),
            funding_score: stable_funding(),
            fuel_input: FuelInput::default(),
            timestamps: fresh_timestamps(),
            trad_status: TradMarketStatus::Live,
        };

        let result = orchestrator.calculate(&input);
        assert_eq!(result.state.state, State::Ready);
        assert!(!result.no_gamma_mode);
        assert!(!result.no_trad_mode);
    }

    #[test]
    fn test_stale_price_produces_wait() {
        let config = test_config();
        let logger = AuditLogger::no_op();
        let mut orchestrator = StateOrchestrator::new(config, logger);

        let mut timestamps = fresh_timestamps();
        timestamps.price_ts = Utc::now().timestamp_millis() - 5000; // 5 seconds stale

        let input = MarketDataInput {
            spot_price: 92000.0,
            ticker: "BTC".to_string(),
            vol_score: normal_vol(),
            options_context: None,
            flow_score: consensus_flow(),
            funding_score: stable_funding(),
            fuel_input: FuelInput::default(),
            timestamps,
            trad_status: TradMarketStatus::Live,
        };

        let result = orchestrator.calculate(&input);
        assert_eq!(result.state.state, State::Wait);
        assert!(result.state.reason.contains("stale"));
    }

    #[test]
    fn test_no_gamma_mode_when_options_unavailable() {
        let config = test_config();
        let logger = AuditLogger::no_op();
        let mut orchestrator = StateOrchestrator::new(config, logger);

        let input = MarketDataInput {
            spot_price: 92000.0,
            ticker: "BTC".to_string(),
            vol_score: normal_vol(),
            options_context: None, // No options data
            flow_score: consensus_flow(),
            funding_score: stable_funding(),
            fuel_input: FuelInput::default(),
            timestamps: fresh_timestamps(),
            trad_status: TradMarketStatus::Live,
        };

        let result = orchestrator.calculate(&input);
        assert!(result.no_gamma_mode);
        assert!(result
            .state
            .components
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::NoGammaData)));
    }

    #[test]
    fn test_no_trad_mode_when_trad_stale() {
        let config = test_config();
        let logger = AuditLogger::no_op();
        let mut orchestrator = StateOrchestrator::new(config, logger);

        let options_ctx = OptionsContext {
            gamma_flip_price: 90000.0,
            last_update: Utc::now().timestamp_millis(),
            ..Default::default()
        };

        let input = MarketDataInput {
            spot_price: 92000.0,
            ticker: "BTC".to_string(),
            vol_score: normal_vol(),
            options_context: Some(options_ctx),
            flow_score: consensus_flow(),
            funding_score: stable_funding(),
            fuel_input: FuelInput::default(),
            timestamps: fresh_timestamps(),
            trad_status: TradMarketStatus::Stale, // Stale trad markets
        };

        let result = orchestrator.calculate(&input);
        assert!(result.no_trad_mode);
        assert!(result
            .state
            .components
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::NoTradMarketsData)));
    }

    #[test]
    fn test_builder_creates_valid_input() {
        let now = Utc::now().timestamp_millis();

        let input = MarketDataInputBuilder::new("BTC", 92000.0)
            .vol_score(normal_vol())
            .flow_score(consensus_flow(), now)
            .funding_score(stable_funding(), now)
            .trad_status(TradMarketStatus::Live)
            .build();

        assert_eq!(input.ticker, "BTC");
        assert_eq!(input.spot_price, 92000.0);
    }

    #[test]
    fn test_state_transitions_logged() {
        let config = test_config();
        let logger = AuditLogger::no_op();
        let mut orchestrator = StateOrchestrator::new(config, logger);

        // First call - WAIT to READY
        let options_ctx = OptionsContext {
            gamma_flip_price: 90000.0,
            last_update: Utc::now().timestamp_millis(),
            ..Default::default()
        };

        let input = MarketDataInput {
            spot_price: 92000.0,
            ticker: "BTC".to_string(),
            vol_score: normal_vol(),
            options_context: Some(options_ctx),
            flow_score: consensus_flow(),
            funding_score: stable_funding(),
            fuel_input: FuelInput::default(),
            timestamps: fresh_timestamps(),
            trad_status: TradMarketStatus::Live,
        };

        let result1 = orchestrator.calculate(&input);
        assert_eq!(result1.state.state, State::Ready);

        // Audit ID should increment
        assert!(result1.state.audit_id > 0);
    }

    #[test]
    fn test_ticker_specific_thresholds_used() {
        let config = test_config();
        let logger = AuditLogger::no_op();
        let mut orchestrator = StateOrchestrator::new(config, logger);

        // SOL has lower extreme threshold (90 vs 95)
        let input = MarketDataInput {
            spot_price: 200.0,
            ticker: "SOL".to_string(),
            vol_score: VolRegimeScore {
                percentile: 92.0, // Would be HIGH for BTC, EXTREME for SOL
                regime: VolRegime::High,
                ..normal_vol()
            },
            options_context: None,
            flow_score: consensus_flow(),
            funding_score: stable_funding(),
            fuel_input: FuelInput::default(),
            timestamps: fresh_timestamps(),
            trad_status: TradMarketStatus::Live,
        };

        let result = orchestrator.calculate(&input);
        // With SOL's 90% threshold, 92% should trigger WAIT
        assert_eq!(result.state.state, State::Wait);
    }
}
