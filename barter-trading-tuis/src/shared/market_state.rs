//! Market State Engine - Core Types (FROZEN)
//!
//! Defines all types for the hierarchical state machine that synthesizes
//! market signals into actionable WAIT/READY/CAUTION states.
//!
//! # Architecture
//! ```text
//! L1: REGIME FILTER  →  "Can I trade at all?"
//!          ↓
//! L2: BIAS FILTER    →  "Which direction makes sense?"
//!          ↓
//! L3: TRIGGER        →  "Is now the right time?"
//!          ↓
//!     STATE OUTPUT   →  WAIT / READY / CAUTION
//! ```
//!
//! # TYPES FREEZE NOTICE
//! These types are frozen after Phase 0. Any changes require orchestrator approval.
//! All thresholds come from config/thresholds.toml - NO hardcoded values.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::time::Duration;

// ============================================================================
// CORE OUTPUT TYPES
// ============================================================================

/// Primary output of the state engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketState {
    /// Current state
    pub state: State,
    /// Directional bias (if state != Wait)
    pub bias: Option<TradingBias>,
    /// Confidence 0-100
    pub confidence: u8,
    /// Full breakdown for auditability
    pub components: StateComponents,
    /// Why this state? Human-readable
    pub reason: String,
    /// Unique ID for audit correlation
    pub audit_id: u64,
    /// Timestamp (Unix millis)
    pub timestamp: i64,
}

/// Trading state output
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum State {
    /// Do not trade - conditions not met
    #[default]
    Wait,
    /// Conditions aligned - can execute
    Ready,
    /// Can trade but with elevated risk
    Caution,
}

/// Directional bias derived from gamma context
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum TradingBias {
    /// Price above gamma flip - fade moves (default: matches AboveFlip default)
    #[default]
    MeanReversion,
    /// Price below gamma flip - follow moves
    Momentum,
}

/// Market direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Direction {
    Long,
    Short,
    #[default]
    Neutral,
}

// ============================================================================
// COMPONENT SCORES (Auditable Breakdown)
// ============================================================================

/// Full breakdown of how state was determined
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StateComponents {
    /// L1: Regime filter result
    pub vol_regime: VolRegimeScore,
    /// L2: Gamma/bias filter result
    pub gamma_context: GammaScore,
    /// L3: Flow consensus result
    pub flow_consensus: FlowScore,
    /// L3: Funding context result
    pub funding_context: FundingScore,
    /// L3: Fuel quality result
    pub fuel_context: FuelScore,
    /// Options context summary (if available)
    pub options_context: Option<OptionsSummary>,
    /// Warnings (don't block, but flag)
    pub warnings: Vec<Warning>,
}

// ============================================================================
// L1: VOLATILITY REGIME
// ============================================================================

/// L1 volatility regime score
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VolRegimeScore {
    /// Current realized volatility
    pub current_rv: f64,
    /// Percentile vs 7-day history (0-100)
    pub percentile: f64,
    /// Current regime classification
    pub regime: VolRegime,
    /// 1-minute return Z-score for shock detection
    pub zscore_1m: f64,
    /// Is there a shock (flash crash) in progress?
    pub is_shock: bool,
    /// Did L1 pass? (both percentile and shock checks)
    pub passed: bool,
}

/// Volatility regime classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum VolRegime {
    /// <20th percentile - quiet, range-bound
    Low,
    /// 20-80th percentile - standard conditions
    #[default]
    Normal,
    /// 80-95th percentile - elevated, wider stops needed
    High,
    /// >95th percentile - crisis/euphoria, sit out
    Extreme,
}

// ============================================================================
// L2: GAMMA CONTEXT
// ============================================================================

/// L2 gamma/bias score
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GammaScore {
    /// Gamma flip price from options
    pub gamma_flip_price: f64,
    /// Current spot price
    pub current_price: f64,
    /// Distance from gamma flip (%)
    pub distance_pct: f64,
    /// Position relative to flip
    pub position: GammaPosition,
    /// Derived trading bias
    pub bias: TradingBias,
}

/// Position relative to gamma flip
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum GammaPosition {
    /// In positive gamma zone (above flip)
    #[default]
    AboveFlip,
    /// In negative gamma zone (below flip)
    BelowFlip,
    /// Within 0.5% of flip (uncertain)
    AtFlip,
}

// ============================================================================
// L3: FLOW CONSENSUS
// ============================================================================

/// L3 flow consensus score
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FlowScore {
    /// Number of venues agreeing on direction
    pub venues_agreeing: u8,
    /// Total venues with data
    pub venues_total: u8,
    /// Consensus direction
    pub consensus_direction: Direction,
    /// Net CVD across venues (USD)
    pub cvd_net: f64,
    /// Is absorption detected?
    pub absorption_detected: bool,
    /// Did L3 flow check pass?
    pub passed: bool,
}

// ============================================================================
// L3: FUNDING CONTEXT
// ============================================================================

/// L3 funding rate score
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FundingScore {
    /// Current funding rate
    pub current_rate: f64,
    /// Funding rate 15 minutes ago
    pub rate_15m_ago: f64,
    /// Velocity (change per 15m)
    pub velocity: f64,
    /// Is rate extreme? (>0.05% or <-0.02%)
    pub is_extreme: bool,
    /// Is rate spiking? (velocity > 0.02% in 15m)
    pub is_spiking: bool,
    /// Did L3 funding check pass?
    pub passed: bool,
}

// ============================================================================
// L3: FUEL QUALITY (Flow + Fuel)
// ============================================================================

/// Raw inputs for fuel quality evaluation
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FuelInput {
    /// Relative volume (5m vs 1h baseline)
    pub rvol_5m: f64,
    /// OI delta over 5m (USD)
    pub oi_delta_usd_5m: f64,
    /// Liquidation rate per minute (USD)
    pub liq_rate_usd_per_min: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FuelQuality {
    High,
    Medium,
    Low,
    Fail,
}

impl Default for FuelQuality {
    fn default() -> Self {
        FuelQuality::Medium
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RvolStatus {
    Strong,
    Normal,
    Thin,
    Fail,
}

impl Default for RvolStatus {
    fn default() -> Self {
        RvolStatus::Normal
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OiTrend {
    NewMoney,
    Squeeze,
    Flat,
}

impl Default for OiTrend {
    fn default() -> Self {
        OiTrend::Flat
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LiqState {
    Low,
    Moderate,
    High,
    Exhaustion,
}

impl Default for LiqState {
    fn default() -> Self {
        LiqState::Low
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FuelScore {
    pub rvol: f64,
    pub rvol_status: RvolStatus,
    pub oi_delta_usd_5m: f64,
    pub oi_trend: OiTrend,
    pub liq_rate_usd_per_min: f64,
    pub liq_state: LiqState,
    pub quality: FuelQuality,
    pub passed: bool,
}

// ============================================================================
// WARNINGS
// ============================================================================

/// Non-blocking warnings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Warning {
    /// Approaching gamma flip level
    ApproachingGammaFlip { distance_pct: f64 },
    /// Funding rate elevated
    FundingElevated { rate: f64 },
    /// Liquidation cluster nearby
    LiquidationClusterNearby { price: f64, size_usd: f64 },
    /// Single venue disagrees with consensus
    SingleVenueDisagreeing { venue: String },
    /// Low liquidity session
    LowLiquiditySession,
    /// Data staleness warning
    StaleData { signal: Signal, age_ms: u64 },
    /// No gamma data available (NO-GAMMA mode)
    NoGammaData,
    /// No trad markets data (NO-TRAD mode)
    NoTradMarketsData,
    /// Expiry buckets have conflicting bias (e.g., front says above flip, medium says below)
    ExpiryBucketConflict,
}

// ============================================================================
// SIGNAL TYPES (for freshness gate)
// ============================================================================

/// Signal types for freshness gate lookups
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Signal {
    Price,
    Cvd,
    L2Book,
    Whale,
    Funding,
    Gamma,
    TradMarkets,
}

// ============================================================================
// DATA AVAILABILITY STATUS
// ============================================================================

/// Data availability status for fallback modes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum DataStatus {
    /// Data is fresh and available
    #[default]
    Live,
    /// Data is stale (beyond max staleness)
    Stale,
    /// No connection/data unavailable
    Unavailable,
}

/// Gamma data status for NO-GAMMA fallback
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum GammaStatus {
    /// Use gamma for bias
    Live { flip_price: f64 },
    /// Use last known flip price
    Stale { last_flip_price: f64 },
    /// No gamma data - use flow-only bias
    #[default]
    Unavailable,
}

/// Traditional markets data status for NO-TRAD fallback
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum TradMarketStatus {
    /// Data fresh, use macro divergence filter
    Live,
    /// Data old, bypass macro filter
    Stale,
    /// No connection, bypass macro filter
    #[default]
    Unavailable,
}

// ============================================================================
// AUDIT ENTRY
// ============================================================================

/// Audit entry - logged only on STATE FLIPS
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unique ID
    pub audit_id: u64,
    /// Timestamp (Unix millis)
    pub timestamp: i64,
    /// Ticker symbol
    pub ticker: String,
    /// Current price
    pub price: f64,
    /// Previous state
    pub prev_state: State,
    /// New state
    pub new_state: State,
    /// Trading bias
    pub bias: Option<TradingBias>,
    /// Confidence score
    pub confidence: u8,
    /// Volatility percentile
    pub vol_percentile: f64,
    /// Volatility regime
    pub vol_regime: VolRegime,
    /// Gamma flip price
    pub gamma_flip_price: Option<f64>,
    /// Distance from gamma flip (%)
    pub price_vs_flip_pct: Option<f64>,
    /// CVD consensus string (e.g., "3/3 SELL")
    pub cvd_consensus: String,
    /// Current funding rate
    pub funding_rate: f64,
    /// Funding velocity
    pub funding_velocity: f64,
    /// Human-readable reason
    pub reason: String,
}

// ============================================================================
// CONFIGURATION TYPES (schema only - values from TOML)
// ============================================================================

/// All configurable thresholds - deserialized from config/thresholds.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thresholds {
    pub volatility: VolatilityThresholds,
    pub consensus: ConsensusThresholds,
    pub funding: FundingThresholds,
    pub fuel: FuelThresholds,
    pub freshness_ms: FreshnessThresholds,
    pub gamma: GammaThresholds,
    pub audit: AuditThresholds,
}

/// Volatility thresholds
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolatilityThresholds {
    /// L1 kill threshold (e.g., 95)
    pub percentile_extreme: f64,
    /// Caution threshold (e.g., 80)
    pub percentile_high: f64,
    /// Flash crash threshold (e.g., 3.5)
    pub zscore_shock: f64,
}

/// Consensus thresholds
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusThresholds {
    /// Minimum agreement % for READY (e.g., 66 = 2/3)
    pub min_agreement_pct: u8,
}

/// Funding rate thresholds
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingThresholds {
    /// Velocity considered spike (e.g., 0.0002)
    pub spike_threshold: f64,
    /// Extreme long threshold (e.g., 0.0005)
    pub extreme_long: f64,
    /// Extreme short threshold (e.g., -0.0002)
    pub extreme_short: f64,
}

/// Fuel quality thresholds (L3)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuelThresholds {
    /// RVOL pass threshold (>=)
    pub rvol_pass: f64,
    /// RVOL caution threshold (>=)
    pub rvol_caution: f64,
    /// OI delta fail threshold for momentum bias (USD, <=)
    pub oi_momentum_fail_usd: f64,
    /// OI delta caution threshold for momentum bias (USD, <=)
    pub oi_momentum_caution_usd: f64,
    /// Liquidation caution threshold (USD/min)
    pub liq_caution_usd_per_min: f64,
    /// Liquidation fail threshold (USD/min)
    pub liq_fail_usd_per_min: f64,
}

/// Freshness thresholds in milliseconds
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreshnessThresholds {
    pub price: u64,
    pub cvd: u64,
    pub l2_book: u64,
    pub whale: u64,
    pub funding: u64,
    pub gamma: u64,
    pub trad_markets: u64,
}

/// Gamma calculation thresholds
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GammaThresholds {
    /// Buffer % around flip for "AT_FLIP" (e.g., 0.5)
    pub flip_buffer_pct: f64,
}

/// Audit logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditThresholds {
    /// Bounded mpsc buffer size
    pub channel_size: usize,
    /// Log directory
    pub log_dir: String,
    /// Rotation size in MB
    pub rotation_mb: u64,
    /// Retention days
    pub retention_days: u32,
}

/// Per-ticker threshold overrides
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TickerOverrides {
    pub volatility: Option<VolatilityOverrides>,
    pub funding: Option<FundingOverrides>,
}

/// Volatility overrides for specific ticker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolatilityOverrides {
    pub percentile_extreme: Option<f64>,
    pub percentile_high: Option<f64>,
    pub zscore_shock: Option<f64>,
}

/// Funding overrides for specific ticker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingOverrides {
    pub spike_threshold: Option<f64>,
    pub extreme_long: Option<f64>,
    pub extreme_short: Option<f64>,
}

// ============================================================================
// TRAIT INTERFACES (frozen - agents implement these)
// ============================================================================

/// Volatility engine trait (Agent B implements in vol_regime.rs)
pub trait VolatilityEngine: Send + Sync {
    fn push_rv(&mut self, rv: f64);
    fn percentile(&self) -> f64;
    fn regime(&self) -> VolRegime;
    fn push_return_1m(&mut self, ret: f64);
    fn zscore_1m(&self) -> f64;
    fn is_shock(&self) -> bool;
}

/// Audit log trait (Agent C implements in audit.rs)
pub trait AuditLog: Send + Sync {
    /// Non-blocking log - never waits, drops if channel full
    fn log(&self, entry: AuditEntry);
}

/// Config provider trait (Agent D implements in config.rs)
pub trait ConfigProvider: Send + Sync {
    /// Get all thresholds
    fn thresholds(&self) -> &Thresholds;
    /// Get freshness duration for a specific signal
    fn freshness(&self, signal: Signal) -> Duration;
}

/// Deribit client trait (Agent E implements in deribit.rs)
#[allow(async_fn_in_trait)]
pub trait DeribitClient: Send + Sync {
    async fn fetch_options_chain(&self, ticker: &str) -> Result<OptionsChain, DeribitError>;
}

/// Gamma engine trait (Agent F implements in gamma.rs)
pub trait GammaEngine: Send + Sync {
    fn calculate_flip(&self, chain: &OptionsChain, spot: f64) -> f64;
    fn nearest_walls(&self, chain: &OptionsChain, spot: f64) -> (Option<Wall>, Option<Wall>);
}

/// Funding tracker trait (Agent B implements in funding.rs)
pub trait FundingTracker: Send + Sync {
    fn push(&mut self, ts: i64, rate: f64);
    fn velocity(&self) -> f64;
    fn is_spiking(&self) -> bool;
    fn is_extreme(&self, rate: f64) -> bool;
}

// ============================================================================
// DERIBIT / OPTIONS TYPES (for trait signatures)
// ============================================================================

/// Options chain from Deribit (used by DeribitClient trait)
#[derive(Debug, Clone, Default)]
pub struct OptionsChain {
    pub contracts: Vec<OptionContract>,
    pub timestamp: i64,
}

/// Single option contract
#[derive(Debug, Clone)]
pub struct OptionContract {
    pub instrument_name: String,
    pub strike: f64,
    pub expiry: i64,
    pub is_call: bool,
    pub open_interest: f64,
    pub mark_iv: f64,
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
}

/// Significant options wall (used by GammaEngine trait)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wall {
    /// Strike price
    pub strike: f64,
    /// Notional USD value
    pub notional_usd: f64,
    /// Distance from current price (%)
    pub distance_pct: f64,
}

/// Summary of a single expiry bucket for UI display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketSummary {
    /// Bucket label (e.g., "0-7d", "7-30d", "30d+")
    pub label: String,
    /// Gamma flip price for this bucket
    pub gamma_flip: f64,
    /// Max pain strike for this bucket
    pub max_pain: f64,
    /// Nearest put wall for this bucket
    pub put_wall: Option<Wall>,
    /// Nearest call wall for this bucket
    pub call_wall: Option<Wall>,
    /// Net GEX for this bucket
    pub gex: f64,
    /// Net DEX for this bucket
    pub dex: f64,
    /// Put/call ratio for this bucket
    pub put_call_ratio: f64,
    /// Hours to nearest expiry in this bucket
    pub hours_to_expiry: f64,
    /// Whether this is the front bucket (used for bias)
    pub is_front: bool,
    /// Number of contracts in this bucket
    pub contract_count: usize,
}

/// Options context summary for UI/audit display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptionsSummary {
    /// Net Gamma Exposure (USD / 1% move)
    pub gexp: f64,
    /// Net Delta Exposure (USD notional)
    pub dexp: f64,
    /// Net Vega Exposure (USD / 1% IV move)
    pub vexp: f64,
    /// Put/Call open interest ratio (puts / calls)
    pub put_call_oi_ratio: f64,
    /// Gamma flip price (all-expiry)
    pub gamma_flip_price: f64,
    /// Max pain strike
    pub max_pain: f64,
    /// Nearest put wall
    pub put_wall: Option<Wall>,
    /// Nearest call wall
    pub call_wall: Option<Wall>,
    /// Hours to nearest expiry
    pub hours_to_expiry: f64,
    /// Last update timestamp (Unix millis)
    pub last_update: i64,
    /// Per-bucket summaries for UI display
    pub buckets: Vec<BucketSummary>,
}

/// Deribit API error
#[derive(Debug, Clone)]
pub enum DeribitError {
    NetworkError(String),
    RateLimited,
    ParseError(String),
}

impl std::fmt::Display for DeribitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeribitError::NetworkError(msg) => write!(f, "Network error: {}", msg),
            DeribitError::RateLimited => write!(f, "Rate limited"),
            DeribitError::ParseError(msg) => write!(f, "Parse error: {}", msg),
        }
    }
}

impl std::error::Error for DeribitError {}

// ============================================================================
// MARKET STATE CALCULATION (Agent A Implementation)
// ============================================================================

impl MarketState {
    /// Calculate market state from current data using hierarchical kill logic.
    ///
    /// # Hierarchical Logic
    /// - L1: REGIME FILTER (Safety Gate) - Vol/shock check, fails immediately to WAIT
    /// - L2: BIAS FILTER (Direction Gate) - Gamma-based bias determination
    /// - L3: TRIGGER (Execution Gate) - CVD consensus + funding check
    ///
    /// # Arguments
    /// * `vol_score` - L1 volatility regime score
    /// * `gamma_score` - L2 gamma/bias score
    /// * `flow_score` - L3 flow consensus score
    /// * `funding_score` - L3 funding context score
    /// * `thresholds` - Configuration thresholds from TOML
    /// * `prev_audit_id` - Previous audit ID for incrementing
    ///
    /// # Returns
    /// Complete `MarketState` with state, bias, confidence, and audit info
    pub fn calculate(
        vol_score: &VolRegimeScore,
        gamma_score: &GammaScore,
        flow_score: &FlowScore,
        funding_score: &FundingScore,
        fuel_input: &FuelInput,
        thresholds: &Thresholds,
        prev_audit_id: u64,
    ) -> Self {
        let mut warnings: Vec<Warning> = Vec::new();

        let funding_score = Self::normalize_funding(funding_score, thresholds);

        // ====================================================================
        // L1: REGIME FILTER (Safety Gate)
        // Extreme vol or shock = immediate WAIT, skip L2/L3
        // ====================================================================
        let l1_result = Self::evaluate_l1(vol_score, thresholds, &mut warnings);
        if l1_result == State::Wait {
            return Self::build_wait_state(
                vol_score,
                gamma_score,
                flow_score,
                &funding_score,
                warnings,
                Self::build_l1_fail_reason(vol_score),
                prev_audit_id,
            );
        }

        // ====================================================================
        // L2: BIAS FILTER (Direction Gate)
        // Determine trading bias from gamma context
        // ====================================================================
        let bias = Self::evaluate_l2(gamma_score, thresholds, &mut warnings);

        // ====================================================================
        // L3: TRIGGER (Execution Gate)
        // CVD consensus + funding velocity check
        // ====================================================================
        let l3_trigger = Self::evaluate_l3_trigger(flow_score, &funding_score, thresholds, &mut warnings);
        let fuel_score = Self::evaluate_fuel(&bias, fuel_input, thresholds, &mut warnings);
        let l3_result = Self::combine_l3(l3_trigger, Self::fuel_state(&fuel_score));

        // ====================================================================
        // COMBINE RESULTS
        // ====================================================================
        let state = Self::combine_results(l1_result, l3_result);
        let confidence = Self::calculate_confidence(&state, vol_score, flow_score, &funding_score, &fuel_score, thresholds);
        let reason = Self::build_reason(&state, &bias, vol_score, flow_score, &funding_score, &fuel_score, thresholds);

        // Per spec: bias is only present when state != WAIT
        let final_bias = if state == State::Wait { None } else { Some(bias) };

        Self {
            state,
            bias: final_bias,
            confidence,
            components: StateComponents {
                vol_regime: vol_score.clone(),
                gamma_context: gamma_score.clone(),
                flow_consensus: flow_score.clone(),
                funding_context: funding_score.clone(),
                fuel_context: fuel_score.clone(),
                options_context: None,
                warnings,
            },
            reason,
            audit_id: prev_audit_id + 1,
            timestamp: Utc::now().timestamp_millis(),
        }
    }

    /// L1: Evaluate volatility regime filter.
    ///
    /// Kill conditions (immediate WAIT):
    /// - Vol percentile >= 95th (extreme volatility)
    /// - Z-score >= 3.5 (flash crash/spike)
    ///
    /// Caution conditions:
    /// - Vol percentile 80-95th (elevated volatility)
    fn evaluate_l1(
        vol: &VolRegimeScore,
        thresholds: &Thresholds,
        _warnings: &mut Vec<Warning>,
    ) -> State {
        // Check kill conditions first
        if vol.percentile >= thresholds.volatility.percentile_extreme {
            return State::Wait;
        }
        if vol.zscore_1m.abs() >= thresholds.volatility.zscore_shock {
            return State::Wait;
        }
        if vol.is_shock {
            return State::Wait;
        }

        // Check caution conditions
        if vol.percentile >= thresholds.volatility.percentile_high {
            return State::Caution;
        }

        State::Ready
    }

    /// L2: Evaluate gamma/bias filter.
    ///
    /// Determines trading bias based on price position relative to gamma flip:
    /// - Above flip -> MeanReversion (fade moves)
    /// - Below flip -> Momentum (follow moves)
    /// - At flip (within buffer) -> Default to MeanReversion with warning
    fn evaluate_l2(
        gamma: &GammaScore,
        thresholds: &Thresholds,
        warnings: &mut Vec<Warning>,
    ) -> TradingBias {
        // Check if approaching gamma flip (within 2x buffer)
        let approach_threshold = thresholds.gamma.flip_buffer_pct * 2.0;
        if gamma.distance_pct.abs() <= approach_threshold && gamma.distance_pct.abs() > 0.0 {
            warnings.push(Warning::ApproachingGammaFlip {
                distance_pct: gamma.distance_pct,
            });
        }

        // Determine bias from position
        match gamma.position {
            GammaPosition::AboveFlip => TradingBias::MeanReversion,
            GammaPosition::BelowFlip => TradingBias::Momentum,
            GammaPosition::AtFlip => {
                // At flip = uncertain, use gamma's pre-calculated bias
                // This allows upstream to determine bias from other factors
                gamma.bias
            }
        }
    }

    /// L3: Evaluate trigger conditions (flow consensus + funding).
    ///
    /// Pass conditions:
    /// - CVD consensus >= 66% venues agree
    /// - Funding not spiking (velocity < threshold)
    /// - No absorption detected
    ///
    /// Returns:
    /// - Ready: All pass
    /// - Caution: Partial pass
    /// - Wait: Critical fail (absorption)
    fn evaluate_l3_trigger(
        flow: &FlowScore,
        funding: &FundingScore,
        thresholds: &Thresholds,
        warnings: &mut Vec<Warning>,
    ) -> State {
        let mut pass_count = 0;
        let mut has_critical_fail = false;

        // Check CVD consensus
        let consensus_pct = if flow.venues_total > 0 {
            (flow.venues_agreeing as f64 / flow.venues_total as f64) * 100.0
        } else {
            0.0
        };
        let consensus_passes = consensus_pct >= thresholds.consensus.min_agreement_pct as f64;
        if consensus_passes {
            pass_count += 1;
        }

        // Check funding velocity (not spiking)
        let funding_passes = !funding.is_spiking;
        if funding_passes {
            pass_count += 1;
        } else {
            warnings.push(Warning::FundingElevated {
                rate: funding.current_rate,
            });
        }

        // Check absorption (critical - blocks if detected)
        let no_absorption = !flow.absorption_detected;
        if no_absorption {
            pass_count += 1;
        } else {
            has_critical_fail = true;
        }

        // Determine L3 result
        let total_checks = 3;
        if has_critical_fail {
            // Absorption is a critical fail - go to WAIT
            State::Wait
        } else if pass_count == total_checks {
            State::Ready
        } else if pass_count >= 2 {
            // 2/3 passing = partial success -> CAUTION
            State::Caution
        } else {
            State::Wait
        }
    }

    fn normalize_funding(funding: &FundingScore, thresholds: &Thresholds) -> FundingScore {
        let mut normalized = funding.clone();
        normalized.is_spiking = normalized.velocity.abs() >= thresholds.funding.spike_threshold;
        normalized.is_extreme = normalized.current_rate > thresholds.funding.extreme_long
            || normalized.current_rate < thresholds.funding.extreme_short;
        normalized.passed = !normalized.is_spiking;
        normalized
    }

    fn evaluate_fuel(
        bias: &TradingBias,
        fuel: &FuelInput,
        thresholds: &Thresholds,
        _warnings: &mut Vec<Warning>,
    ) -> FuelScore {
        let rvol_status = if fuel.rvol_5m <= 0.0 {
            RvolStatus::Normal
        } else if fuel.rvol_5m >= thresholds.fuel.rvol_pass * 1.5 {
            RvolStatus::Strong
        } else if fuel.rvol_5m >= thresholds.fuel.rvol_pass {
            RvolStatus::Normal
        } else if fuel.rvol_5m >= thresholds.fuel.rvol_caution {
            RvolStatus::Thin
        } else {
            RvolStatus::Fail
        };

        let has_oi = fuel.oi_delta_usd_5m.abs() > 1.0;
        let oi_trend = if !has_oi {
            OiTrend::Flat
        } else {
            match bias {
                TradingBias::Momentum => {
                    if fuel.oi_delta_usd_5m > 0.0 {
                        OiTrend::NewMoney
                    } else if fuel.oi_delta_usd_5m <= thresholds.fuel.oi_momentum_fail_usd {
                        OiTrend::Squeeze
                    } else {
                        OiTrend::Flat
                    }
                }
                TradingBias::MeanReversion => {
                    if fuel.oi_delta_usd_5m <= 0.0 {
                        OiTrend::Squeeze
                    } else {
                        OiTrend::NewMoney
                    }
                }
            }
        };

        let liq_state = if fuel.liq_rate_usd_per_min >= thresholds.fuel.liq_fail_usd_per_min {
            LiqState::Exhaustion
        } else if fuel.liq_rate_usd_per_min >= thresholds.fuel.liq_caution_usd_per_min {
            LiqState::High
        } else if fuel.liq_rate_usd_per_min > 0.0 {
            LiqState::Moderate
        } else {
            LiqState::Low
        };

        let mut fail = false;
        let mut caution = false;

        if matches!(rvol_status, RvolStatus::Fail) {
            fail = true;
        } else if matches!(rvol_status, RvolStatus::Thin) {
            caution = true;
        }

        if has_oi {
            match bias {
                TradingBias::Momentum => {
                    if fuel.oi_delta_usd_5m <= thresholds.fuel.oi_momentum_fail_usd {
                        fail = true;
                    } else if fuel.oi_delta_usd_5m <= thresholds.fuel.oi_momentum_caution_usd {
                        caution = true;
                    }
                }
                TradingBias::MeanReversion => {
                    if fuel.oi_delta_usd_5m > 0.0 {
                        caution = true;
                    }
                }
            }
        }

        if matches!(liq_state, LiqState::Exhaustion) {
            fail = true;
        } else if matches!(liq_state, LiqState::High) {
            caution = true;
        }

        let quality = if fail {
            FuelQuality::Fail
        } else if caution {
            FuelQuality::Medium
        } else {
            FuelQuality::High
        };

        FuelScore {
            rvol: fuel.rvol_5m,
            rvol_status,
            oi_delta_usd_5m: fuel.oi_delta_usd_5m,
            oi_trend,
            liq_rate_usd_per_min: fuel.liq_rate_usd_per_min,
            liq_state,
            quality,
            passed: !fail,
        }
    }

    fn fuel_state(fuel: &FuelScore) -> State {
        match fuel.quality {
            FuelQuality::High => State::Ready,
            FuelQuality::Medium | FuelQuality::Low => State::Caution,
            FuelQuality::Fail => State::Wait,
        }
    }

    fn combine_l3(trigger: State, fuel: State) -> State {
        if trigger == State::Wait || fuel == State::Wait {
            return State::Wait;
        }
        if trigger == State::Caution || fuel == State::Caution {
            return State::Caution;
        }
        State::Ready
    }

    /// Combine L1 and L3 results into final state.
    ///
    /// Priority: Wait > Caution > Ready
    fn combine_results(l1_result: State, l3_result: State) -> State {
        // If either layer says Wait, final is Wait
        if l1_result == State::Wait || l3_result == State::Wait {
            return State::Wait;
        }

        // If either layer says Caution, final is Caution
        if l1_result == State::Caution || l3_result == State::Caution {
            return State::Caution;
        }

        // Both Ready = Ready
        State::Ready
    }

    /// Calculate confidence score (0-100) based on signal alignment.
    ///
    /// Factors:
    /// - Volatility: Lower percentile = higher confidence
    /// - CVD consensus: Higher agreement = higher confidence
    /// - Funding: Not extreme/spiking = higher confidence
    fn calculate_confidence(
        state: &State,
        vol: &VolRegimeScore,
        flow: &FlowScore,
        funding: &FundingScore,
        fuel: &FuelScore,
        thresholds: &Thresholds,
    ) -> u8 {
        // Base confidence by state
        let base = match state {
            State::Wait => 0,
            State::Caution => 40,
            State::Ready => 70,
        };

        if *state == State::Wait {
            return 0;
        }

        // Volatility factor: lower percentile = more confidence (0-15 points)
        let vol_factor = if vol.percentile < 50.0 {
            15
        } else if vol.percentile < 70.0 {
            10
        } else if vol.percentile < 80.0 {
            5
        } else {
            0
        };

        // CVD consensus factor: higher agreement = more confidence (0-10 points)
        let consensus_pct = if flow.venues_total > 0 {
            (flow.venues_agreeing as f64 / flow.venues_total as f64) * 100.0
        } else {
            0.0
        };
        let cvd_factor = if consensus_pct >= 100.0 {
            10
        } else if consensus_pct >= 80.0 {
            7
        } else if consensus_pct >= 66.0 {
            5
        } else {
            0
        };

        // Funding factor: stable funding = more confidence (0-5 points)
        let funding_spike = funding.velocity.abs() >= thresholds.funding.spike_threshold;
        let funding_extreme = funding.current_rate > thresholds.funding.extreme_long
            || funding.current_rate < thresholds.funding.extreme_short;
        let funding_factor = if !funding_extreme && !funding_spike {
            5
        } else if !funding_spike {
            2
        } else {
            0
        };

        let fuel_factor = match fuel.quality {
            FuelQuality::High => 10,
            FuelQuality::Medium => 5,
            FuelQuality::Low => 2,
            FuelQuality::Fail => 0,
        };

        // Cap at 100
        std::cmp::min(100, base + vol_factor + cvd_factor + funding_factor + fuel_factor)
    }

    /// Build human-readable reason string for L1 failure.
    fn build_l1_fail_reason(vol: &VolRegimeScore) -> String {
        if vol.is_shock || vol.zscore_1m.abs() >= 3.5 {
            format!(
                "L1 FAIL: Volatility shock detected (zscore={:.2})",
                vol.zscore_1m
            )
        } else {
            format!(
                "L1 FAIL: Extreme volatility (percentile={:.1}%, regime={:?})",
                vol.percentile, vol.regime
            )
        }
    }

    /// Build human-readable reason string for current state.
    fn build_reason(
        state: &State,
        bias: &TradingBias,
        vol: &VolRegimeScore,
        flow: &FlowScore,
        funding: &FundingScore,
        fuel: &FuelScore,
        thresholds: &Thresholds,
    ) -> String {
        let consensus_str = format!(
            "{}/{} {}",
            flow.venues_agreeing,
            flow.venues_total,
            match flow.consensus_direction {
                Direction::Long => "LONG",
                Direction::Short => "SHORT",
                Direction::Neutral => "NEUTRAL",
            }
        );

        match state {
            State::Ready => {
                format!(
                    "READY: {} bias, CVD {}, vol={:.0}%ile, funding={:.4}%, fuel={:?}",
                    match bias {
                        TradingBias::MeanReversion => "MeanReversion",
                        TradingBias::Momentum => "Momentum",
                    },
                    consensus_str,
                    vol.percentile,
                    funding.current_rate * 100.0,
                    fuel.quality
                )
            }
            State::Caution => {
                let mut reasons = Vec::new();
                if vol.percentile >= 80.0 {
                    reasons.push(format!("elevated vol ({:.0}%ile)", vol.percentile));
                }
                if funding.velocity.abs() >= thresholds.funding.spike_threshold {
                    reasons.push("funding spiking".to_string());
                }
                if flow.venues_agreeing < flow.venues_total {
                    reasons.push(format!("partial consensus ({})", consensus_str));
                }
                if matches!(fuel.quality, FuelQuality::Medium | FuelQuality::Low) {
                    reasons.push("fuel weak".to_string());
                }
                if reasons.is_empty() {
                    reasons.push("mixed signals".to_string());
                }
                format!(
                    "CAUTION: {} bias, {}",
                    match bias {
                        TradingBias::MeanReversion => "MeanReversion",
                        TradingBias::Momentum => "Momentum",
                    },
                    reasons.join(", ")
                )
            }
            State::Wait => {
                let mut reasons = Vec::new();
                if flow.absorption_detected {
                    reasons.push("absorption detected".to_string());
                }
                let funding_spike = funding.velocity.abs() >= thresholds.funding.spike_threshold;
                let funding_extreme = funding.current_rate > thresholds.funding.extreme_long
                    || funding.current_rate < thresholds.funding.extreme_short;
                if funding_spike && funding_extreme {
                    reasons.push("funding crisis".to_string());
                }
                let consensus_pct = if flow.venues_total > 0 {
                    (flow.venues_agreeing as f64 / flow.venues_total as f64) * 100.0
                } else {
                    0.0
                };
                if consensus_pct < 66.0 {
                    reasons.push(format!("low consensus ({})", consensus_str));
                }
                if matches!(fuel.quality, FuelQuality::Fail) {
                    reasons.push("fuel fail".to_string());
                }
                if reasons.is_empty() {
                    reasons.push("conditions not aligned".to_string());
                }
                format!("WAIT: {}", reasons.join(", "))
            }
        }
    }

    /// Build a WAIT state when L1 fails (early exit path).
    fn build_wait_state(
        vol_score: &VolRegimeScore,
        gamma_score: &GammaScore,
        flow_score: &FlowScore,
        funding_score: &FundingScore,
        warnings: Vec<Warning>,
        reason: String,
        prev_audit_id: u64,
    ) -> Self {
        Self {
            state: State::Wait,
            bias: None, // No bias on L1 fail - didn't evaluate L2
            confidence: 0,
            components: StateComponents {
                vol_regime: vol_score.clone(),
                gamma_context: gamma_score.clone(),
                flow_consensus: flow_score.clone(),
                funding_context: funding_score.clone(),
                fuel_context: FuelScore::default(),
                options_context: None,
                warnings,
            },
            reason,
            audit_id: prev_audit_id + 1,
            timestamp: Utc::now().timestamp_millis(),
        }
    }
}

impl State {
    /// Returns true if this state allows trading.
    pub fn allows_trading(&self) -> bool {
        matches!(self, State::Ready | State::Caution)
    }

    /// Returns true if this state requires caution.
    pub fn requires_caution(&self) -> bool {
        matches!(self, State::Caution)
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            State::Wait => write!(f, "WAIT"),
            State::Ready => write!(f, "READY"),
            State::Caution => write!(f, "CAUTION"),
        }
    }
}

impl std::fmt::Display for TradingBias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TradingBias::MeanReversion => write!(f, "MeanReversion"),
            TradingBias::Momentum => write!(f, "Momentum"),
        }
    }
}

impl std::fmt::Display for VolRegime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VolRegime::Low => write!(f, "LOW"),
            VolRegime::Normal => write!(f, "NORMAL"),
            VolRegime::High => write!(f, "HIGH"),
            VolRegime::Extreme => write!(f, "EXTREME"),
        }
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn default_thresholds() -> Thresholds {
        Thresholds {
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
                rvol_pass: 1.2,
                rvol_caution: 0.8,
                oi_momentum_fail_usd: -15_000_000.0,
                oi_momentum_caution_usd: -5_000_000.0,
                liq_caution_usd_per_min: 150_000.0,
                liq_fail_usd_per_min: 2_000_000.0,
            },
            freshness_ms: FreshnessThresholds {
                price: 1000,
                cvd: 2000,
                l2_book: 2000,
                whale: 5000,
                funding: 60000,
                gamma: 300000,
                trad_markets: 60000,
            },
            gamma: GammaThresholds {
                flip_buffer_pct: 0.5,
            },
            audit: AuditThresholds {
                channel_size: 1000,
                log_dir: "/tmp/audit".to_string(),
                rotation_mb: 100,
                retention_days: 30,
            },
        }
    }

    fn normal_vol_score() -> VolRegimeScore {
        VolRegimeScore {
            current_rv: 0.02,
            percentile: 50.0,
            regime: VolRegime::Normal,
            zscore_1m: 0.5,
            is_shock: false,
            passed: true,
        }
    }

    fn extreme_vol_score() -> VolRegimeScore {
        VolRegimeScore {
            current_rv: 0.08,
            percentile: 97.0,
            regime: VolRegime::Extreme,
            zscore_1m: 4.0,
            is_shock: true,
            passed: false,
        }
    }

    fn default_gamma_score() -> GammaScore {
        GammaScore {
            gamma_flip_price: 100000.0,
            current_price: 101000.0,
            distance_pct: 1.0,
            position: GammaPosition::AboveFlip,
            bias: TradingBias::MeanReversion,
        }
    }

    fn consensus_flow_score() -> FlowScore {
        FlowScore {
            venues_agreeing: 3,
            venues_total: 3,
            consensus_direction: Direction::Long,
            cvd_net: 1000000.0,
            absorption_detected: false,
            passed: true,
        }
    }

    fn partial_flow_score() -> FlowScore {
        FlowScore {
            venues_agreeing: 2,
            venues_total: 3,
            consensus_direction: Direction::Long,
            cvd_net: 500000.0,
            absorption_detected: false,
            passed: true,
        }
    }

    fn stable_funding_score() -> FundingScore {
        FundingScore {
            current_rate: 0.0001,
            rate_15m_ago: 0.0001,
            velocity: 0.0,
            is_extreme: false,
            is_spiking: false,
            passed: true,
        }
    }

    fn spiking_funding_score() -> FundingScore {
        FundingScore {
            current_rate: 0.0006,
            rate_15m_ago: 0.0001,
            velocity: 0.0005,
            is_extreme: true,
            is_spiking: true,
            passed: false,
        }
    }

    #[test]
    fn test_l1_extreme_vol_kills_state() {
        let thresholds = default_thresholds();
        let state = MarketState::calculate(
            &extreme_vol_score(),
            &default_gamma_score(),
            &consensus_flow_score(),
            &stable_funding_score(),
            &FuelInput::default(),
            &thresholds,
            0,
        );

        assert_eq!(state.state, State::Wait);
        assert!(state.bias.is_none()); // No bias on L1 fail
        assert_eq!(state.confidence, 0);
        assert!(state.reason.contains("L1 FAIL"));
    }

    #[test]
    fn test_ready_state_all_conditions_pass() {
        let thresholds = default_thresholds();
        let state = MarketState::calculate(
            &normal_vol_score(),
            &default_gamma_score(),
            &consensus_flow_score(),
            &stable_funding_score(),
            &FuelInput::default(),
            &thresholds,
            0,
        );

        assert_eq!(state.state, State::Ready);
        assert_eq!(state.bias, Some(TradingBias::MeanReversion));
        assert!(state.confidence >= 70);
        assert!(state.reason.contains("READY"));
    }

    #[test]
    fn test_caution_state_partial_consensus() {
        let thresholds = default_thresholds();
        let state = MarketState::calculate(
            &normal_vol_score(),
            &default_gamma_score(),
            &partial_flow_score(),
            &stable_funding_score(),
            &FuelInput::default(),
            &thresholds,
            0,
        );

        // 2/3 consensus still passes the 66% threshold
        assert!(state.state == State::Ready || state.state == State::Caution);
    }

    #[test]
    fn test_caution_state_high_vol() {
        let high_vol = VolRegimeScore {
            percentile: 85.0,
            regime: VolRegime::High,
            ..normal_vol_score()
        };
        let thresholds = default_thresholds();
        let state = MarketState::calculate(
            &high_vol,
            &default_gamma_score(),
            &consensus_flow_score(),
            &stable_funding_score(),
            &FuelInput::default(),
            &thresholds,
            0,
        );

        assert_eq!(state.state, State::Caution);
        assert!(state.reason.contains("CAUTION"));
    }

    #[test]
    fn test_wait_state_absorption() {
        let absorption_flow = FlowScore {
            absorption_detected: true,
            ..consensus_flow_score()
        };
        let thresholds = default_thresholds();
        let state = MarketState::calculate(
            &normal_vol_score(),
            &default_gamma_score(),
            &absorption_flow,
            &stable_funding_score(),
            &FuelInput::default(),
            &thresholds,
            0,
        );

        assert_eq!(state.state, State::Wait);
        // Per spec: bias must be None when state is WAIT (even if L3 caused it, not L1)
        assert!(state.bias.is_none(), "bias must be None when state is WAIT");
        assert!(state.reason.contains("absorption"));
    }

    #[test]
    fn test_wait_state_l3_multiple_fails_has_no_bias() {
        // L3 WAIT due to low consensus + funding spiking (2 fails = pass_count < 2)
        // bias should still be None per spec
        let low_consensus_flow = FlowScore {
            venues_agreeing: 1,
            venues_total: 3,
            consensus_direction: Direction::Long,
            cvd_net: 100000.0,
            absorption_detected: false,
            passed: false,
        };
        let thresholds = default_thresholds();
        let state = MarketState::calculate(
            &normal_vol_score(),
            &default_gamma_score(),
            &low_consensus_flow,
            &spiking_funding_score(), // Funding also failing
            &thresholds,
            0,
        );

        // With consensus fail + funding spike, only no_absorption passes = 1/3 = WAIT
        assert_eq!(state.state, State::Wait);
        assert!(state.bias.is_none(), "bias must be None when state is WAIT due to L3 fail");
    }

    #[test]
    fn test_audit_id_increments() {
        let thresholds = default_thresholds();
        let state = MarketState::calculate(
            &normal_vol_score(),
            &default_gamma_score(),
            &consensus_flow_score(),
            &stable_funding_score(),
            &FuelInput::default(),
            &thresholds,
            100,
        );

        assert_eq!(state.audit_id, 101);
    }

    #[test]
    fn test_bias_above_flip() {
        let gamma = GammaScore {
            position: GammaPosition::AboveFlip,
            bias: TradingBias::MeanReversion,
            ..default_gamma_score()
        };
        let thresholds = default_thresholds();
        let state = MarketState::calculate(
            &normal_vol_score(),
            &gamma,
            &consensus_flow_score(),
            &stable_funding_score(),
            &FuelInput::default(),
            &thresholds,
            0,
        );

        assert_eq!(state.bias, Some(TradingBias::MeanReversion));
    }

    #[test]
    fn test_bias_below_flip() {
        let gamma = GammaScore {
            position: GammaPosition::BelowFlip,
            bias: TradingBias::Momentum,
            current_price: 99000.0,
            distance_pct: -1.0,
            ..default_gamma_score()
        };
        let thresholds = default_thresholds();
        let state = MarketState::calculate(
            &normal_vol_score(),
            &gamma,
            &consensus_flow_score(),
            &stable_funding_score(),
            &FuelInput::default(),
            &thresholds,
            0,
        );

        assert_eq!(state.bias, Some(TradingBias::Momentum));
    }

    #[test]
    fn test_funding_spike_adds_warning() {
        let thresholds = default_thresholds();
        let state = MarketState::calculate(
            &normal_vol_score(),
            &default_gamma_score(),
            &consensus_flow_score(),
            &spiking_funding_score(),
            &FuelInput::default(),
            &thresholds,
            0,
        );

        // Should have FundingElevated warning
        let has_funding_warning = state
            .components
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::FundingElevated { .. }));
        assert!(has_funding_warning);
    }

    #[test]
    fn test_state_display() {
        assert_eq!(format!("{}", State::Wait), "WAIT");
        assert_eq!(format!("{}", State::Ready), "READY");
        assert_eq!(format!("{}", State::Caution), "CAUTION");
    }

    #[test]
    fn test_state_allows_trading() {
        assert!(!State::Wait.allows_trading());
        assert!(State::Ready.allows_trading());
        assert!(State::Caution.allows_trading());
    }
}
