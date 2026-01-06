//! Volatility Regime Engine
//!
//! Implements L1 volatility regime classification for the Market State Engine.
//!
//! # Overview
//! - Rolling 7-day volatility history (168 hourly samples) for percentile calculation
//! - 1-minute return buffer (60 samples = 1 hour) for Z-score shock detection
//! - Percentile: rank current RV vs 7-day history (0-100)
//! - Z-Score: (latest_return - mean) / std_dev
//! - Shock: abs(zscore) >= shock_threshold (default 3.5)
//!
//! # Regime Thresholds
//! - Low: <20th percentile
//! - Normal: 20-80th percentile
//! - High: 80-95th percentile
//! - Extreme: >=95th percentile

use std::collections::VecDeque;

use crate::shared::market_state::{VolRegime, VolatilityEngine};

/// Number of hourly samples for 7-day history (24 * 7 = 168)
const HOURLY_SAMPLES: usize = 168;

/// Number of 1-minute samples for shock detection (1 hour)
const MINUTE_SAMPLES: usize = 60;

/// Default shock threshold (Z-score magnitude)
const DEFAULT_SHOCK_THRESHOLD: f64 = 3.5;

/// Volatility regime engine for L1 classification
///
/// Maintains rolling windows for:
/// - Hourly realized volatility samples (7 days)
/// - 1-minute returns (1 hour) for shock detection
#[derive(Debug, Clone)]
pub struct VolRegimeEngine {
    /// Rolling 7-day hourly RV history (168 samples max)
    hourly_rv: VecDeque<f64>,
    /// Rolling 1-hour minute returns (60 samples max)
    returns_1m: VecDeque<f64>,
    /// Current realized volatility value
    current_rv: f64,
    /// Z-score threshold for shock detection
    shock_threshold: f64,
    /// Low regime threshold percentile (default 20)
    low_threshold: f64,
    /// High regime threshold percentile (default 80)
    high_threshold: f64,
    /// Extreme regime threshold percentile (default 95)
    extreme_threshold: f64,
}

impl Default for VolRegimeEngine {
    fn default() -> Self {
        Self::new(DEFAULT_SHOCK_THRESHOLD, 20.0, 80.0, 95.0)
    }
}

impl VolRegimeEngine {
    /// Create a new volatility regime engine with configurable thresholds
    ///
    /// # Arguments
    /// * `shock_threshold` - Z-score magnitude threshold for shock detection (default 3.5)
    /// * `low_threshold` - Percentile threshold for Low regime (default 20)
    /// * `high_threshold` - Percentile threshold for High regime (default 80)
    /// * `extreme_threshold` - Percentile threshold for Extreme regime (default 95)
    pub fn new(
        shock_threshold: f64,
        low_threshold: f64,
        high_threshold: f64,
        extreme_threshold: f64,
    ) -> Self {
        Self {
            hourly_rv: VecDeque::with_capacity(HOURLY_SAMPLES),
            returns_1m: VecDeque::with_capacity(MINUTE_SAMPLES),
            current_rv: 0.0,
            shock_threshold,
            low_threshold,
            high_threshold,
            extreme_threshold,
        }
    }

    /// Create with default thresholds from spec
    pub fn with_defaults() -> Self {
        Self::default()
    }

    /// Get the number of hourly RV samples currently stored
    pub fn rv_sample_count(&self) -> usize {
        self.hourly_rv.len()
    }

    /// Get the number of 1-minute return samples currently stored
    pub fn return_sample_count(&self) -> usize {
        self.returns_1m.len()
    }

    /// Check if we have enough data for reliable percentile calculation
    /// Requires at least 24 samples (1 day) for meaningful percentile
    pub fn has_minimum_rv_data(&self) -> bool {
        self.hourly_rv.len() >= 24
    }

    /// Get current realized volatility
    pub fn current_rv(&self) -> f64 {
        self.current_rv
    }

    /// Calculate mean of 1-minute returns
    fn returns_mean(&self) -> f64 {
        if self.returns_1m.is_empty() {
            return 0.0;
        }
        self.returns_1m.iter().sum::<f64>() / self.returns_1m.len() as f64
    }

    /// Calculate standard deviation of 1-minute returns
    fn returns_std(&self) -> f64 {
        if self.returns_1m.len() < 2 {
            return 0.0;
        }

        let mean = self.returns_mean();
        let variance = self
            .returns_1m
            .iter()
            .map(|&r| {
                let diff = r - mean;
                diff * diff
            })
            .sum::<f64>()
            / (self.returns_1m.len() - 1) as f64;

        variance.sqrt()
    }

    /// Get the latest 1-minute return
    fn latest_return(&self) -> Option<f64> {
        self.returns_1m.back().copied()
    }
}

impl VolatilityEngine for VolRegimeEngine {
    /// Push a new hourly realized volatility sample
    fn push_rv(&mut self, rv: f64) {
        // Maintain fixed capacity
        if self.hourly_rv.len() >= HOURLY_SAMPLES {
            self.hourly_rv.pop_front();
        }
        self.hourly_rv.push_back(rv);
        self.current_rv = rv;
    }

    /// Calculate percentile of current RV vs 7-day history
    ///
    /// Returns 0-100 where:
    /// - 0 = lowest volatility in history
    /// - 100 = highest volatility in history
    fn percentile(&self) -> f64 {
        if self.hourly_rv.is_empty() {
            return 50.0; // Default to middle if no data
        }

        // Count how many historical values are below current
        let count_below = self
            .hourly_rv
            .iter()
            .filter(|&&v| v < self.current_rv)
            .count();

        // Percentile = (count below / total) * 100
        (count_below as f64 / self.hourly_rv.len() as f64) * 100.0
    }

    /// Determine volatility regime based on percentile thresholds
    fn regime(&self) -> VolRegime {
        let pct = self.percentile();

        if pct >= self.extreme_threshold {
            VolRegime::Extreme
        } else if pct >= self.high_threshold {
            VolRegime::High
        } else if pct >= self.low_threshold {
            VolRegime::Normal
        } else {
            VolRegime::Low
        }
    }

    /// Push a new 1-minute return for shock detection
    fn push_return_1m(&mut self, ret: f64) {
        // Maintain fixed capacity
        if self.returns_1m.len() >= MINUTE_SAMPLES {
            self.returns_1m.pop_front();
        }
        self.returns_1m.push_back(ret);
    }

    /// Calculate Z-score of latest 1-minute return
    ///
    /// Z-score = (latest_return - mean) / std_dev
    /// Returns 0.0 if insufficient data
    fn zscore_1m(&self) -> f64 {
        if self.returns_1m.len() < 2 {
            return 0.0;
        }

        let latest = match self.latest_return() {
            Some(r) => r,
            None => return 0.0,
        };

        let mean = self.returns_mean();
        let std = self.returns_std();

        if std < 1e-10 {
            // Avoid division by near-zero
            return 0.0;
        }

        (latest - mean) / std
    }

    /// Check if current conditions indicate a shock (flash crash/pump)
    ///
    /// Returns true if |zscore| >= shock_threshold
    fn is_shock(&self) -> bool {
        self.zscore_1m().abs() >= self.shock_threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_construction() {
        let engine = VolRegimeEngine::default();
        assert_eq!(engine.shock_threshold, 3.5);
        assert_eq!(engine.low_threshold, 20.0);
        assert_eq!(engine.high_threshold, 80.0);
        assert_eq!(engine.extreme_threshold, 95.0);
        assert_eq!(engine.rv_sample_count(), 0);
        assert_eq!(engine.return_sample_count(), 0);
    }

    #[test]
    fn test_custom_thresholds() {
        let engine = VolRegimeEngine::new(4.0, 25.0, 75.0, 90.0);
        assert_eq!(engine.shock_threshold, 4.0);
        assert_eq!(engine.low_threshold, 25.0);
        assert_eq!(engine.high_threshold, 75.0);
        assert_eq!(engine.extreme_threshold, 90.0);
    }

    #[test]
    fn test_push_rv_maintains_capacity() {
        let mut engine = VolRegimeEngine::default();

        // Push more than capacity
        for i in 0..200 {
            engine.push_rv(i as f64);
        }

        // Should be capped at HOURLY_SAMPLES (168)
        assert_eq!(engine.rv_sample_count(), HOURLY_SAMPLES);
        // Current RV should be the last pushed value
        assert_eq!(engine.current_rv(), 199.0);
    }

    #[test]
    fn test_push_return_maintains_capacity() {
        let mut engine = VolRegimeEngine::default();

        // Push more than capacity
        for i in 0..100 {
            engine.push_return_1m(i as f64 * 0.01);
        }

        // Should be capped at MINUTE_SAMPLES (60)
        assert_eq!(engine.return_sample_count(), MINUTE_SAMPLES);
    }

    #[test]
    fn test_percentile_empty() {
        let engine = VolRegimeEngine::default();
        // Default to 50 when no data
        assert_eq!(engine.percentile(), 50.0);
    }

    #[test]
    fn test_percentile_calculation() {
        let mut engine = VolRegimeEngine::default();

        // Push values 1-100
        for i in 1..=100 {
            engine.push_rv(i as f64);
        }

        // Current RV is 100 (the max), so percentile should be ~99
        // 99 values below 100 / 100 total = 99%
        assert!((engine.percentile() - 99.0).abs() < 0.1);

        // Push a value in the middle
        engine.push_rv(50.0);
        // Now we have 101 samples, 49 values below 50
        // Actually we have values 2-100 plus 50, current_rv = 50
        // Values below 50: 2,3,...,49 = 48 values
        // Total = 101
        // Percentile = 48/101 * 100 ~= 47.5%
        let pct = engine.percentile();
        assert!(pct > 40.0 && pct < 60.0);
    }

    #[test]
    fn test_regime_classification_low() {
        let mut engine = VolRegimeEngine::default();

        // Create a distribution where current RV is in bottom 20%
        for i in 1..=100 {
            engine.push_rv(i as f64);
        }
        // Set current RV to 10 (10th percentile)
        engine.push_rv(10.0);

        assert_eq!(engine.regime(), VolRegime::Low);
    }

    #[test]
    fn test_regime_classification_normal() {
        let mut engine = VolRegimeEngine::default();

        // Create a distribution
        for i in 1..=100 {
            engine.push_rv(i as f64);
        }
        // Set current RV to 50 (50th percentile)
        engine.push_rv(50.0);

        assert_eq!(engine.regime(), VolRegime::Normal);
    }

    #[test]
    fn test_regime_classification_high() {
        let mut engine = VolRegimeEngine::default();

        // Create a distribution
        for i in 1..=100 {
            engine.push_rv(i as f64);
        }
        // Set current RV to 90 (should be ~89th percentile)
        engine.push_rv(90.0);

        assert_eq!(engine.regime(), VolRegime::High);
    }

    #[test]
    fn test_regime_classification_extreme() {
        let mut engine = VolRegimeEngine::default();

        // Create a distribution
        for i in 1..=100 {
            engine.push_rv(i as f64);
        }
        // Set current RV to 99 (should be ~98th percentile)
        engine.push_rv(99.0);

        assert_eq!(engine.regime(), VolRegime::Extreme);
    }

    #[test]
    fn test_zscore_insufficient_data() {
        let mut engine = VolRegimeEngine::default();

        // No data
        assert_eq!(engine.zscore_1m(), 0.0);

        // One sample
        engine.push_return_1m(0.01);
        assert_eq!(engine.zscore_1m(), 0.0);
    }

    #[test]
    fn test_zscore_calculation() {
        let mut engine = VolRegimeEngine::default();

        // Push stable returns
        for _ in 0..30 {
            engine.push_return_1m(0.001); // 0.1% return
        }

        // Push a big move
        engine.push_return_1m(0.05); // 5% return

        // Z-score should be significantly positive
        let zscore = engine.zscore_1m();
        assert!(zscore > 2.0, "Z-score should be high: {}", zscore);
    }

    #[test]
    fn test_zscore_negative_shock() {
        let mut engine = VolRegimeEngine::default();

        // Push stable returns
        for _ in 0..30 {
            engine.push_return_1m(0.001);
        }

        // Push a big drop
        engine.push_return_1m(-0.05); // -5% return

        // Z-score should be significantly negative
        let zscore = engine.zscore_1m();
        assert!(zscore < -2.0, "Z-score should be low: {}", zscore);
    }

    #[test]
    fn test_is_shock_detection() {
        let mut engine = VolRegimeEngine::default();

        // Push stable returns with realistic variance (oscillating around 0)
        for i in 0..50 {
            // Alternate between small positive and negative returns
            let ret = if i % 2 == 0 { 0.001 } else { -0.0005 };
            engine.push_return_1m(ret);
        }

        // Should not be a shock with a return within normal range
        engine.push_return_1m(0.002); // 0.2% is within normal variance
        assert!(!engine.is_shock(), "Normal return should not trigger shock");

        // Push extreme return - 10% move when typical returns are <0.2%
        engine.push_return_1m(0.1); // 10% return - massive shock

        // Should detect as shock
        assert!(engine.is_shock(), "10% move should trigger shock");
    }

    #[test]
    fn test_is_shock_with_custom_threshold() {
        let mut engine = VolRegimeEngine::new(2.0, 20.0, 80.0, 95.0); // Lower threshold

        // Push stable returns
        for _ in 0..50 {
            engine.push_return_1m(0.001);
        }

        // Push moderate move
        engine.push_return_1m(0.02);

        // With lower threshold, this might trigger a shock
        let zscore = engine.zscore_1m();
        // The shock detection depends on the actual z-score
        assert_eq!(engine.is_shock(), zscore.abs() >= 2.0);
    }

    #[test]
    fn test_zero_std_dev_handling() {
        let mut engine = VolRegimeEngine::default();

        // Push identical returns
        for _ in 0..10 {
            engine.push_return_1m(0.001);
        }

        // Std dev is ~0, should return 0 z-score to avoid NaN
        assert_eq!(engine.zscore_1m(), 0.0);
    }

    #[test]
    fn test_has_minimum_rv_data() {
        let mut engine = VolRegimeEngine::default();

        assert!(!engine.has_minimum_rv_data());

        for i in 0..23 {
            engine.push_rv(i as f64);
        }
        assert!(!engine.has_minimum_rv_data());

        engine.push_rv(24.0);
        assert!(engine.has_minimum_rv_data());
    }

    #[test]
    fn test_send_sync() {
        // Compile-time check that VolRegimeEngine is Send + Sync
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<VolRegimeEngine>();
    }

    #[test]
    fn test_realistic_scenario() {
        let mut engine = VolRegimeEngine::default();

        // Simulate 7 days of hourly RV data with realistic values
        // Normal market: RV between 0.01 (1%) and 0.03 (3%)
        for i in 0..168 {
            let rv = 0.015 + 0.005 * ((i as f64 * 0.1).sin()); // oscillating RV
            engine.push_rv(rv);
        }

        // Simulate 1 hour of minute returns
        for i in 0..60 {
            let ret = 0.0001 * ((i as f64 * 0.5).sin()); // small oscillating returns
            engine.push_return_1m(ret);
        }

        // Check that we're in normal regime with no shock
        let regime = engine.regime();
        let is_shock = engine.is_shock();

        println!("Regime: {:?}, Percentile: {:.2}%", regime, engine.percentile());
        println!("Z-score: {:.4}, Is shock: {}", engine.zscore_1m(), is_shock);

        assert!(!is_shock);
        // Regime could be Normal or Low depending on the exact values
        assert!(matches!(regime, VolRegime::Low | VolRegime::Normal | VolRegime::High));
    }
}
