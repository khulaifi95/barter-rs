//! Options Context - Synthesized options market state
//!
//! Provides OptionsContext which aggregates Deribit data into actionable context:
//! - Gamma flip price (dynamic, accounts for IV shifts)
//! - Put/Call walls (support/resistance from options positioning)
//! - Max pain strike (price magnet near expiry)
//! - GEXP/DEX/VEX exposures (gamma/delta/vega in USD terms)
//!
//! # NO-GAMMA Mode
//! When options data is stale or unavailable, the orchestrator enters NO-GAMMA mode:
//! - Bias is determined from flow direction only
//! - L2 filter is bypassed (not blocked)

use crate::shared::gamma::GammaCalculator;
use crate::shared::market_state::{
    DeribitClient, DeribitError, GammaEngine, GammaPosition, GammaScore, GammaStatus,
    OptionContract, OptionsChain, TradingBias, Wall,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::time::Duration;

// ============================================================================
// EXPIRY BUCKET TYPES
// ============================================================================

/// Expiry bucket classification for options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ExpiryBucket {
    /// 0-7 days (front bucket, used for bias)
    #[default]
    Short,
    /// 7-30 days
    Medium,
    /// 30+ days
    Long,
}

impl ExpiryBucket {
    /// Get the hour range for this bucket (min_hours, max_hours)
    pub fn hour_range(&self) -> (f64, f64) {
        match self {
            ExpiryBucket::Short => (0.0, 168.0),     // 0-7 days
            ExpiryBucket::Medium => (168.0, 720.0),  // 7-30 days
            ExpiryBucket::Long => (720.0, f64::MAX), // 30+ days
        }
    }

    /// Get display label for this bucket
    pub fn label(&self) -> &'static str {
        match self {
            ExpiryBucket::Short => "0-7d",
            ExpiryBucket::Medium => "7-30d",
            ExpiryBucket::Long => "30d+",
        }
    }

    /// All buckets in order
    pub fn all() -> [ExpiryBucket; 3] {
        [
            ExpiryBucket::Short,
            ExpiryBucket::Medium,
            ExpiryBucket::Long,
        ]
    }
}

/// Metrics for a single expiry bucket
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketMetrics {
    /// Which bucket this is
    pub bucket: ExpiryBucket,
    /// Gamma flip price for contracts in this bucket
    pub gamma_flip: f64,
    /// Max pain strike for contracts in this bucket
    pub max_pain: f64,
    /// Nearest put wall for this bucket
    pub put_wall: Option<Wall>,
    /// Nearest call wall for this bucket
    pub call_wall: Option<Wall>,
    /// Net gamma exposure (GEX) for this bucket
    pub gex: f64,
    /// Net delta exposure (DEX) for this bucket
    pub dex: f64,
    /// Net vega exposure (VEX) for this bucket
    pub vex: f64,
    /// Put/call OI ratio for this bucket
    pub put_call_ratio: f64,
    /// Hours to the nearest expiry in this bucket
    pub hours_to_expiry: f64,
    /// Number of contracts in this bucket
    pub contract_count: usize,
}

impl Default for BucketMetrics {
    fn default() -> Self {
        Self {
            bucket: ExpiryBucket::Short,
            gamma_flip: 0.0,
            max_pain: 0.0,
            put_wall: None,
            call_wall: None,
            gex: 0.0,
            dex: 0.0,
            vex: 0.0,
            put_call_ratio: 0.0,
            hours_to_expiry: f64::MAX,
            contract_count: 0,
        }
    }
}

impl BucketMetrics {
    /// Check if this bucket has meaningful data
    pub fn has_data(&self) -> bool {
        self.contract_count > 0
    }

    /// Convert to GammaScore for orchestrator
    pub fn to_gamma_score(&self, spot: f64, flip_buffer_pct: f64) -> GammaScore {
        let distance_pct = if self.gamma_flip > 0.0 {
            ((spot - self.gamma_flip) / self.gamma_flip) * 100.0
        } else {
            0.0
        };

        let position = if distance_pct.abs() <= flip_buffer_pct {
            GammaPosition::AtFlip
        } else if distance_pct > 0.0 {
            GammaPosition::AboveFlip
        } else {
            GammaPosition::BelowFlip
        };

        let bias = match position {
            GammaPosition::AboveFlip => TradingBias::MeanReversion,
            GammaPosition::BelowFlip => TradingBias::Momentum,
            GammaPosition::AtFlip => TradingBias::MeanReversion,
        };

        GammaScore {
            gamma_flip_price: self.gamma_flip,
            current_price: spot,
            distance_pct,
            position,
            bias,
        }
    }
}

// ============================================================================
// OPTIONS CONTEXT
// ============================================================================

/// Options-derived market context (from Deribit)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptionsContext {
    /// Net gamma exposure in USD (positive = dealers long gamma)
    pub gexp: f64,
    /// Net delta exposure in USD notional
    pub dexp: f64,
    /// Net vega exposure in USD per 1% IV move
    pub vexp: f64,
    /// Put/Call open interest ratio (puts / calls)
    pub put_call_oi_ratio: f64,
    /// The critical level: where gamma flips sign (all-expiry)
    pub gamma_flip_price: f64,
    /// Max pain strike (price magnet near expiry)
    pub max_pain: f64,
    /// Nearest significant put wall (support)
    pub put_wall: Option<Wall>,
    /// Nearest significant call wall (resistance)
    pub call_wall: Option<Wall>,
    /// Hours until next major expiry
    pub hours_to_expiry: f64,
    /// Last update timestamp (Unix millis)
    pub last_update: i64,
    /// Per-bucket metrics [Short, Medium, Long]
    pub buckets: [BucketMetrics; 3],
    /// Which bucket is used for L2 bias (usually Short/front)
    pub front_bucket: ExpiryBucket,
}

impl Default for OptionsContext {
    fn default() -> Self {
        Self {
            gexp: 0.0,
            dexp: 0.0,
            vexp: 0.0,
            put_call_oi_ratio: 0.0,
            gamma_flip_price: 0.0,
            max_pain: 0.0,
            put_wall: None,
            call_wall: None,
            hours_to_expiry: f64::MAX,
            last_update: 0,
            buckets: [
                BucketMetrics {
                    bucket: ExpiryBucket::Short,
                    ..Default::default()
                },
                BucketMetrics {
                    bucket: ExpiryBucket::Medium,
                    ..Default::default()
                },
                BucketMetrics {
                    bucket: ExpiryBucket::Long,
                    ..Default::default()
                },
            ],
            front_bucket: ExpiryBucket::Short,
        }
    }
}

impl OptionsContext {
    /// Check if options data is fresh enough to use
    pub fn is_fresh(&self, max_staleness: Duration) -> bool {
        let now = Utc::now().timestamp_millis();
        let age_ms = (now - self.last_update) as u64;
        age_ms <= max_staleness.as_millis() as u64
    }

    /// Get data age in milliseconds
    pub fn age_ms(&self) -> u64 {
        let now = Utc::now().timestamp_millis();
        (now - self.last_update).max(0) as u64
    }

    /// Convert to GammaStatus for orchestrator
    pub fn to_gamma_status(&self, max_staleness: Duration) -> GammaStatus {
        if self.last_update == 0 {
            GammaStatus::Unavailable
        } else if self.is_fresh(max_staleness) {
            GammaStatus::Live {
                flip_price: self.gamma_flip_price,
            }
        } else {
            GammaStatus::Stale {
                last_flip_price: self.gamma_flip_price,
            }
        }
    }

    /// Calculate GammaScore from current context and spot price
    pub fn to_gamma_score(&self, spot: f64, flip_buffer_pct: f64) -> GammaScore {
        let distance_pct = if self.gamma_flip_price > 0.0 {
            ((spot - self.gamma_flip_price) / self.gamma_flip_price) * 100.0
        } else {
            0.0
        };

        let position = if distance_pct.abs() <= flip_buffer_pct {
            GammaPosition::AtFlip
        } else if distance_pct > 0.0 {
            GammaPosition::AboveFlip
        } else {
            GammaPosition::BelowFlip
        };

        let bias = match position {
            GammaPosition::AboveFlip => TradingBias::MeanReversion,
            GammaPosition::BelowFlip => TradingBias::Momentum,
            GammaPosition::AtFlip => TradingBias::MeanReversion, // Default at flip
        };

        GammaScore {
            gamma_flip_price: self.gamma_flip_price,
            current_price: spot,
            distance_pct,
            position,
            bias,
        }
    }

    /// Get the front bucket metrics (used for L2 bias)
    pub fn front_bucket_metrics(&self) -> &BucketMetrics {
        let idx = match self.front_bucket {
            ExpiryBucket::Short => 0,
            ExpiryBucket::Medium => 1,
            ExpiryBucket::Long => 2,
        };
        &self.buckets[idx]
    }

    /// Calculate GammaScore from front bucket (preferred for L2 bias)
    pub fn to_gamma_score_from_front_bucket(&self, spot: f64, flip_buffer_pct: f64) -> GammaScore {
        let front = self.front_bucket_metrics();
        if front.has_data() {
            front.to_gamma_score(spot, flip_buffer_pct)
        } else {
            // Fall back to all-expiry flip if front bucket has no data
            self.to_gamma_score(spot, flip_buffer_pct)
        }
    }

    /// Check if buckets have conflicting bias (cross-bucket warning)
    pub fn has_bucket_conflict(&self, spot: f64) -> bool {
        let front = &self.buckets[0]; // Short
        let medium = &self.buckets[1]; // Medium

        if !front.has_data() || !medium.has_data() {
            return false;
        }

        // Conflict if front says above flip but medium says below (or vice versa)
        let front_above = front.gamma_flip > 0.0 && spot > front.gamma_flip;
        let medium_above = medium.gamma_flip > 0.0 && spot > medium.gamma_flip;

        front_above != medium_above
    }

    /// Get bucket by index
    pub fn bucket(&self, bucket: ExpiryBucket) -> &BucketMetrics {
        match bucket {
            ExpiryBucket::Short => &self.buckets[0],
            ExpiryBucket::Medium => &self.buckets[1],
            ExpiryBucket::Long => &self.buckets[2],
        }
    }
}

/// Builder for OptionsContext from raw Deribit data
pub struct OptionsContextBuilder {
    gamma_calculator: GammaCalculator,
}

impl Default for OptionsContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl OptionsContextBuilder {
    pub fn new() -> Self {
        Self {
            gamma_calculator: GammaCalculator::default(),
        }
    }

    pub fn with_gamma_calculator(mut self, calc: GammaCalculator) -> Self {
        self.gamma_calculator = calc;
        self
    }

    /// Build OptionsContext from options chain and current spot price
    pub fn build(&self, chain: &OptionsChain, spot: f64) -> OptionsContext {
        let gamma_flip_price = self.gamma_calculator.calculate_flip(chain, spot);
        let (put_wall, call_wall) = self.gamma_calculator.nearest_walls(chain, spot);

        // Calculate max pain (strike with most OI where calls + puts converge)
        let max_pain = self.calculate_max_pain(chain);

        // Calculate net GEXP (simplified: sum of dealer gamma exposure)
        let gexp = self.calculate_gexp(chain, spot);
        let dexp = self.calculate_dexp(chain, spot);
        let vexp = self.calculate_vexp(chain);
        let put_call_oi_ratio = self.calculate_put_call_ratio(chain);

        // Calculate hours to nearest expiry
        let hours_to_expiry = self.calculate_hours_to_expiry(chain);

        // Calculate per-bucket metrics
        let buckets = [
            self.calculate_bucket_metrics(chain, spot, ExpiryBucket::Short),
            self.calculate_bucket_metrics(chain, spot, ExpiryBucket::Medium),
            self.calculate_bucket_metrics(chain, spot, ExpiryBucket::Long),
        ];

        // Determine front bucket (use Short if it has data, otherwise first with data)
        let front_bucket = if buckets[0].has_data() {
            ExpiryBucket::Short
        } else if buckets[1].has_data() {
            ExpiryBucket::Medium
        } else {
            ExpiryBucket::Long
        };

        tracing::info!(
            "OptionsContext built: gexp={:.2}M, dexp={:.2}M, vexp={:.2}M, p/c={:.2}, flip=${:.0}, max_pain=${:.0}",
            gexp / 1_000_000.0,
            dexp / 1_000_000.0,
            vexp / 1_000_000.0,
            put_call_oi_ratio,
            gamma_flip_price,
            max_pain
        );

        // Log bucket details
        for bucket in buckets.iter() {
            if bucket.has_data() {
                tracing::info!(
                    "  Bucket[{}]: {} contracts, flip=${:.0}, gex={:.2}M, p/c={:.2}",
                    bucket.bucket.label(),
                    bucket.contract_count,
                    bucket.gamma_flip,
                    bucket.gex / 1_000_000.0,
                    bucket.put_call_ratio
                );
            }
        }

        OptionsContext {
            gexp,
            dexp,
            vexp,
            put_call_oi_ratio,
            gamma_flip_price,
            max_pain,
            put_wall,
            call_wall,
            hours_to_expiry,
            last_update: Utc::now().timestamp_millis(),
            buckets,
            front_bucket,
        }
    }

    /// Async build from Deribit client
    pub async fn build_from_deribit<C: DeribitClient>(
        &self,
        client: &C,
        ticker: &str,
        spot: f64,
    ) -> Result<OptionsContext, DeribitError> {
        let chain = client.fetch_options_chain(ticker).await?;
        Ok(self.build(&chain, spot))
    }

    /// Calculate max pain strike (simplified: highest combined OI)
    fn calculate_max_pain(&self, chain: &OptionsChain) -> f64 {
        use std::collections::HashMap;

        let mut oi_by_strike: HashMap<i64, f64> = HashMap::new();

        for contract in &chain.contracts {
            let strike_key = (contract.strike * 100.0) as i64; // Use cents for key
            *oi_by_strike.entry(strike_key).or_insert(0.0) += contract.open_interest;
        }

        oi_by_strike
            .into_iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(strike_key, _)| strike_key as f64 / 100.0)
            .unwrap_or(0.0)
    }

    /// Calculate net gamma exposure (simplified)
    fn calculate_gexp(&self, chain: &OptionsChain, spot: f64) -> f64 {
        // Simplified GEXP: sum of (gamma * OI * spot^2 * 0.01) for each contract
        // Positive for calls (dealers long gamma when selling calls)
        // Negative for puts (dealers short gamma when selling puts)

        // Debug: count contracts with non-zero gamma
        let contracts_with_gamma = chain.contracts.iter().filter(|c| c.gamma > 0.0).count();

        let gexp: f64 = chain
            .contracts
            .iter()
            .map(|c| {
                let sign = if c.is_call { 1.0 } else { -1.0 };
                sign * c.gamma * c.open_interest * spot * spot * 0.01
            })
            .sum();

        tracing::debug!(
            "GEXP calc: {} contracts, {} with gamma>0, spot={:.0}, gexp={:.0}",
            chain.contracts.len(),
            contracts_with_gamma,
            spot,
            gexp
        );

        gexp
    }

    /// Calculate net delta exposure (USD notional)
    fn calculate_dexp(&self, chain: &OptionsChain, spot: f64) -> f64 {
        chain
            .contracts
            .iter()
            .map(|c| c.delta * c.open_interest * spot)
            .sum()
    }

    /// Calculate net vega exposure (USD per 1% IV move)
    fn calculate_vexp(&self, chain: &OptionsChain) -> f64 {
        chain
            .contracts
            .iter()
            .map(|c| c.vega * c.open_interest)
            .sum()
    }

    /// Calculate put/call open interest ratio
    fn calculate_put_call_ratio(&self, chain: &OptionsChain) -> f64 {
        let mut put_oi = 0.0;
        let mut call_oi = 0.0;

        for contract in &chain.contracts {
            if contract.is_call {
                call_oi += contract.open_interest;
            } else {
                put_oi += contract.open_interest;
            }
        }

        if call_oi > 0.0 {
            put_oi / call_oi
        } else {
            0.0
        }
    }

    /// Calculate hours until nearest expiry
    fn calculate_hours_to_expiry(&self, chain: &OptionsChain) -> f64 {
        let now = Utc::now().timestamp_millis();

        chain
            .contracts
            .iter()
            .filter(|c| c.expiry > now)
            .map(|c| (c.expiry - now) as f64 / (1000.0 * 60.0 * 60.0))
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or(f64::MAX)
    }

    /// Calculate metrics for a specific expiry bucket
    fn calculate_bucket_metrics(
        &self,
        chain: &OptionsChain,
        spot: f64,
        bucket: ExpiryBucket,
    ) -> BucketMetrics {
        let now = Utc::now().timestamp_millis();
        let (min_hours, max_hours) = bucket.hour_range();

        // Filter contracts by expiry bucket
        let filtered: Vec<&OptionContract> = chain
            .contracts
            .iter()
            .filter(|c| {
                if c.expiry <= now {
                    return false;
                }
                let hours = (c.expiry - now) as f64 / (1000.0 * 60.0 * 60.0);
                hours >= min_hours && hours < max_hours
            })
            .collect();

        if filtered.is_empty() {
            return BucketMetrics {
                bucket,
                ..Default::default()
            };
        }

        // Create a filtered chain for gamma flip calculation
        let filtered_chain = OptionsChain {
            contracts: filtered.iter().map(|c| (*c).clone()).collect(),
            timestamp: chain.timestamp,
        };

        // Calculate gamma flip and walls for this bucket
        let gamma_flip = self.gamma_calculator.calculate_flip(&filtered_chain, spot);
        let (put_wall, call_wall) = self.gamma_calculator.nearest_walls(&filtered_chain, spot);

        // Calculate max pain for this bucket
        let max_pain = self.calculate_max_pain(&filtered_chain);

        // Calculate GEX for this bucket
        let gex: f64 = filtered
            .iter()
            .map(|c| {
                let sign = if c.is_call { 1.0 } else { -1.0 };
                sign * c.gamma * c.open_interest * spot * spot * 0.01
            })
            .sum();

        // Calculate DEX for this bucket
        let dex: f64 = filtered
            .iter()
            .map(|c| c.delta * c.open_interest * spot)
            .sum();

        // Calculate VEX for this bucket
        let vex: f64 = filtered.iter().map(|c| c.vega * c.open_interest).sum();

        // Calculate put/call ratio for this bucket
        let mut put_oi = 0.0;
        let mut call_oi = 0.0;
        for c in &filtered {
            if c.is_call {
                call_oi += c.open_interest;
            } else {
                put_oi += c.open_interest;
            }
        }
        let put_call_ratio = if call_oi > 0.0 { put_oi / call_oi } else { 0.0 };

        // Calculate hours to nearest expiry in this bucket
        let hours_to_expiry = filtered
            .iter()
            .filter(|c| c.expiry > now)
            .map(|c| (c.expiry - now) as f64 / (1000.0 * 60.0 * 60.0))
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or(f64::MAX);

        BucketMetrics {
            bucket,
            gamma_flip,
            max_pain,
            put_wall,
            call_wall,
            gex,
            dex,
            vex,
            put_call_ratio,
            hours_to_expiry,
            contract_count: filtered.len(),
        }
    }
}

/// Default GammaScore when in NO-GAMMA mode (flow-based bias)
pub fn no_gamma_score(
    spot: f64,
    flow_direction: crate::shared::market_state::Direction,
) -> GammaScore {
    use crate::shared::market_state::Direction;

    // In NO-GAMMA mode, derive bias from flow direction
    let bias = match flow_direction {
        Direction::Long => TradingBias::Momentum, // Follow the flow up
        Direction::Short => TradingBias::Momentum, // Follow the flow down
        Direction::Neutral => TradingBias::MeanReversion, // Default to mean reversion
    };

    GammaScore {
        gamma_flip_price: 0.0, // Unknown
        current_price: spot,
        distance_pct: 0.0,
        position: GammaPosition::AtFlip, // Unknown position
        bias,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::market_state::OptionContract;

    fn sample_chain() -> OptionsChain {
        OptionsChain {
            contracts: vec![
                OptionContract {
                    instrument_name: "BTC-90000-P".to_string(),
                    strike: 90000.0,
                    expiry: Utc::now().timestamp_millis() + 86400000, // +1 day
                    is_call: false,
                    open_interest: 1000.0,
                    mark_iv: 0.5,
                    delta: -0.3,
                    gamma: 0.00001,
                    vega: 100.0,
                },
                OptionContract {
                    instrument_name: "BTC-95000-C".to_string(),
                    strike: 95000.0,
                    expiry: Utc::now().timestamp_millis() + 86400000,
                    is_call: true,
                    open_interest: 800.0,
                    mark_iv: 0.55,
                    delta: 0.4,
                    gamma: 0.00001,
                    vega: 120.0,
                },
                OptionContract {
                    instrument_name: "BTC-100000-C".to_string(),
                    strike: 100000.0,
                    expiry: Utc::now().timestamp_millis() + 86400000,
                    is_call: true,
                    open_interest: 1200.0,
                    mark_iv: 0.6,
                    delta: 0.25,
                    gamma: 0.000008,
                    vega: 80.0,
                },
            ],
            timestamp: Utc::now().timestamp_millis(),
        }
    }

    #[test]
    fn test_options_context_default() {
        let ctx = OptionsContext::default();
        assert_eq!(ctx.gexp, 0.0);
        assert_eq!(ctx.dexp, 0.0);
        assert_eq!(ctx.vexp, 0.0);
        assert_eq!(ctx.gamma_flip_price, 0.0);
        assert_eq!(ctx.last_update, 0);
    }

    #[test]
    fn test_options_context_freshness() {
        let ctx = OptionsContext {
            last_update: Utc::now().timestamp_millis(),
            ..Default::default()
        };

        assert!(ctx.is_fresh(Duration::from_secs(60)));
        assert!(ctx.age_ms() < 1000);
    }

    #[test]
    fn test_options_context_stale() {
        let ctx = OptionsContext {
            last_update: Utc::now().timestamp_millis() - 600_000, // 10 minutes ago
            ..Default::default()
        };

        assert!(!ctx.is_fresh(Duration::from_secs(300))); // 5 min max
    }

    #[test]
    fn test_gamma_status_live() {
        let ctx = OptionsContext {
            last_update: Utc::now().timestamp_millis(),
            gamma_flip_price: 92000.0,
            ..Default::default()
        };

        let status = ctx.to_gamma_status(Duration::from_secs(600));
        assert!(matches!(status, GammaStatus::Live { flip_price } if flip_price == 92000.0));
    }

    #[test]
    fn test_gamma_status_stale() {
        let ctx = OptionsContext {
            last_update: Utc::now().timestamp_millis() - 700_000, // 11+ minutes ago
            gamma_flip_price: 92000.0,
            ..Default::default()
        };

        let status = ctx.to_gamma_status(Duration::from_secs(600)); // 10 min max
        assert!(
            matches!(status, GammaStatus::Stale { last_flip_price } if last_flip_price == 92000.0)
        );
    }

    #[test]
    fn test_gamma_status_unavailable() {
        let ctx = OptionsContext::default(); // last_update = 0
        let status = ctx.to_gamma_status(Duration::from_secs(600));
        assert!(matches!(status, GammaStatus::Unavailable));
    }

    #[test]
    fn test_to_gamma_score_above_flip() {
        let ctx = OptionsContext {
            gamma_flip_price: 90000.0,
            last_update: Utc::now().timestamp_millis(),
            ..Default::default()
        };

        let score = ctx.to_gamma_score(92000.0, 0.5);
        assert_eq!(score.position, GammaPosition::AboveFlip);
        assert_eq!(score.bias, TradingBias::MeanReversion);
        assert!(score.distance_pct > 0.0);
    }

    #[test]
    fn test_to_gamma_score_below_flip() {
        let ctx = OptionsContext {
            gamma_flip_price: 95000.0,
            last_update: Utc::now().timestamp_millis(),
            ..Default::default()
        };

        let score = ctx.to_gamma_score(92000.0, 0.5);
        assert_eq!(score.position, GammaPosition::BelowFlip);
        assert_eq!(score.bias, TradingBias::Momentum);
        assert!(score.distance_pct < 0.0);
    }

    #[test]
    fn test_to_gamma_score_at_flip() {
        let ctx = OptionsContext {
            gamma_flip_price: 92000.0,
            last_update: Utc::now().timestamp_millis(),
            ..Default::default()
        };

        let score = ctx.to_gamma_score(92100.0, 0.5); // Within 0.5%
        assert_eq!(score.position, GammaPosition::AtFlip);
    }

    #[test]
    fn test_builder_creates_context() {
        let builder = OptionsContextBuilder::new();
        let chain = sample_chain();
        let ctx = builder.build(&chain, 92000.0);

        assert!(ctx.gamma_flip_price > 0.0);
        assert!(ctx.last_update > 0);
        assert!(ctx.hours_to_expiry < 48.0); // Less than 2 days
    }

    #[test]
    fn test_max_pain_calculation() {
        let builder = OptionsContextBuilder::new();
        let chain = sample_chain();
        let ctx = builder.build(&chain, 92000.0);

        // Max pain should be strike with highest combined OI
        // In our sample, 100000 has 1200 OI (highest)
        assert_eq!(ctx.max_pain, 100000.0);
    }

    #[test]
    fn test_no_gamma_score_long_flow() {
        use crate::shared::market_state::Direction;
        let score = no_gamma_score(92000.0, Direction::Long);
        assert_eq!(score.bias, TradingBias::Momentum);
        assert_eq!(score.position, GammaPosition::AtFlip);
    }

    #[test]
    fn test_no_gamma_score_neutral_flow() {
        use crate::shared::market_state::Direction;
        let score = no_gamma_score(92000.0, Direction::Neutral);
        assert_eq!(score.bias, TradingBias::MeanReversion);
    }
}
