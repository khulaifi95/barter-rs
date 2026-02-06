//! Funding Rate Tracker
//!
//! Implements funding rate tracking for the Market State Engine (L3 layer).
//!
//! # Overview
//! - Rolling 15-minute window of funding rate samples
//! - Velocity: change in funding rate over the 15-minute window
//! - Spike detection: velocity > threshold (default 0.0002 = 0.02%)
//! - Extreme detection: rate > 0.0005 (long) or rate < -0.0002 (short)
//!
//! # Usage
//! The funding tracker is used by the Market State Engine to:
//! - Track funding rate momentum (velocity)
//! - Detect funding rate spikes (rapid changes)
//! - Identify extreme funding rates (crowded positioning)

use std::collections::VecDeque;

use crate::shared::market_state::{FundingThresholds, FundingTracker as FundingTrackerTrait};

/// Lookback window in milliseconds (15 minutes)
const LOOKBACK_MS: i64 = 15 * 60 * 1000;

/// Default spike threshold (0.02% change in 15 minutes)
const DEFAULT_SPIKE_THRESHOLD: f64 = 0.0002;

/// Default extreme long threshold (>0.05% funding rate)
const DEFAULT_EXTREME_LONG: f64 = 0.0005;

/// Default extreme short threshold (<-0.02% funding rate)
const DEFAULT_EXTREME_SHORT: f64 = -0.0002;

/// Funding rate tracker for L3 market state classification
///
/// Maintains a rolling 15-minute window of funding rate samples
/// to calculate velocity and detect spikes/extremes.
#[derive(Debug, Clone)]
pub struct FundingRateTracker {
    /// Rolling history of (timestamp_ms, rate) tuples
    history: VecDeque<(i64, f64)>,
    /// Velocity threshold for spike detection
    spike_threshold: f64,
    /// Threshold for extreme long funding (crowded longs)
    extreme_long: f64,
    /// Threshold for extreme short funding (crowded shorts)
    extreme_short: f64,
}

impl Default for FundingRateTracker {
    fn default() -> Self {
        Self {
            history: VecDeque::with_capacity(100),
            spike_threshold: DEFAULT_SPIKE_THRESHOLD,
            extreme_long: DEFAULT_EXTREME_LONG,
            extreme_short: DEFAULT_EXTREME_SHORT,
        }
    }
}

impl FundingRateTracker {
    /// Create a new funding rate tracker with custom thresholds
    ///
    /// # Arguments
    /// * `spike_threshold` - Velocity threshold for spike detection (default 0.0002)
    /// * `extreme_long` - Threshold for extreme long funding (default 0.0005)
    /// * `extreme_short` - Threshold for extreme short funding (default -0.0002)
    pub fn new(spike_threshold: f64, extreme_long: f64, extreme_short: f64) -> Self {
        Self {
            history: VecDeque::with_capacity(100),
            spike_threshold,
            extreme_long,
            extreme_short,
        }
    }

    /// Create a funding rate tracker from configuration thresholds
    ///
    /// # Arguments
    /// * `thresholds` - FundingThresholds from config
    pub fn with_thresholds(thresholds: &FundingThresholds) -> Self {
        Self::new(
            thresholds.spike_threshold,
            thresholds.extreme_long,
            thresholds.extreme_short,
        )
    }

    /// Get the current (most recent) funding rate
    ///
    /// Returns `None` if no funding rates have been recorded
    pub fn current_rate(&self) -> Option<f64> {
        self.history.back().map(|(_, rate)| *rate)
    }

    /// Get the funding rate from approximately 15 minutes ago
    ///
    /// Returns the oldest rate within the 15-minute window, or `None` if empty
    pub fn rate_15m_ago(&self) -> Option<f64> {
        self.history.front().map(|(_, rate)| *rate)
    }

    /// Get the number of samples currently in the history
    pub fn sample_count(&self) -> usize {
        self.history.len()
    }

    /// Check if we have at least 2 samples for velocity calculation
    pub fn has_sufficient_data(&self) -> bool {
        self.history.len() >= 2
    }

    /// Get the time span covered by current history in milliseconds
    pub fn time_span_ms(&self) -> i64 {
        if self.history.len() < 2 {
            return 0;
        }

        let newest_ts = self.history.back().map(|(ts, _)| *ts).unwrap_or(0);
        let oldest_ts = self.history.front().map(|(ts, _)| *ts).unwrap_or(0);

        newest_ts - oldest_ts
    }

    /// Remove entries older than the lookback window
    fn prune_old_entries(&mut self, current_ts: i64) {
        let cutoff = current_ts - LOOKBACK_MS;
        while let Some((ts, _)) = self.history.front() {
            if *ts < cutoff {
                self.history.pop_front();
            } else {
                break;
            }
        }
    }
}

impl FundingTrackerTrait for FundingRateTracker {
    /// Push a new funding rate sample
    ///
    /// # Arguments
    /// * `ts` - Timestamp in milliseconds
    /// * `rate` - Funding rate (e.g., 0.0001 = 0.01%)
    fn push(&mut self, ts: i64, rate: f64) {
        // First, prune old entries beyond the 15-minute window
        self.prune_old_entries(ts);

        // Add new entry
        self.history.push_back((ts, rate));
    }

    /// Calculate funding rate velocity (change over 15-minute window)
    ///
    /// Returns `newest_rate - oldest_rate`
    /// Returns 0.0 if insufficient data
    fn velocity(&self) -> f64 {
        if self.history.len() < 2 {
            return 0.0;
        }

        let newest_rate = self.history.back().map(|(_, r)| *r).unwrap_or(0.0);
        let oldest_rate = self.history.front().map(|(_, r)| *r).unwrap_or(0.0);

        newest_rate - oldest_rate
    }

    /// Check if funding rate is spiking (rapid change)
    ///
    /// Returns true if |velocity| > spike_threshold
    fn is_spiking(&self) -> bool {
        self.velocity().abs() > self.spike_threshold
    }

    /// Check if a funding rate is extreme
    ///
    /// Returns true if:
    /// - rate > extreme_long (crowded longs paying shorts)
    /// - rate < extreme_short (crowded shorts paying longs)
    ///
    /// # Arguments
    /// * `rate` - Funding rate to check
    fn is_extreme(&self, rate: f64) -> bool {
        rate > self.extreme_long || rate < self.extreme_short
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_construction() {
        let tracker = FundingRateTracker::default();
        assert_eq!(tracker.spike_threshold, DEFAULT_SPIKE_THRESHOLD);
        assert_eq!(tracker.extreme_long, DEFAULT_EXTREME_LONG);
        assert_eq!(tracker.extreme_short, DEFAULT_EXTREME_SHORT);
        assert_eq!(tracker.sample_count(), 0);
    }

    #[test]
    fn test_custom_thresholds() {
        let tracker = FundingRateTracker::new(0.0003, 0.001, -0.0005);
        assert_eq!(tracker.spike_threshold, 0.0003);
        assert_eq!(tracker.extreme_long, 0.001);
        assert_eq!(tracker.extreme_short, -0.0005);
    }

    #[test]
    fn test_with_thresholds_from_config() {
        let thresholds = FundingThresholds {
            spike_threshold: 0.0004,
            extreme_long: 0.0008,
            extreme_short: -0.0003,
        };
        let tracker = FundingRateTracker::with_thresholds(&thresholds);
        assert_eq!(tracker.spike_threshold, 0.0004);
        assert_eq!(tracker.extreme_long, 0.0008);
        assert_eq!(tracker.extreme_short, -0.0003);
    }

    #[test]
    fn test_push_and_current_rate() {
        let mut tracker = FundingRateTracker::default();

        // Empty history
        assert!(tracker.current_rate().is_none());

        // Push first rate
        tracker.push(1000, 0.0001);
        assert_eq!(tracker.current_rate(), Some(0.0001));
        assert_eq!(tracker.sample_count(), 1);

        // Push second rate
        tracker.push(2000, 0.0002);
        assert_eq!(tracker.current_rate(), Some(0.0002));
        assert_eq!(tracker.sample_count(), 2);
    }

    #[test]
    fn test_rate_15m_ago() {
        let mut tracker = FundingRateTracker::default();

        // Empty history
        assert!(tracker.rate_15m_ago().is_none());

        // Push rates within 15-minute window
        let base_ts = 1_000_000;
        tracker.push(base_ts, 0.0001);
        tracker.push(base_ts + 60_000, 0.0002);
        tracker.push(base_ts + 120_000, 0.0003);

        // Oldest rate should be the first one
        assert_eq!(tracker.rate_15m_ago(), Some(0.0001));
    }

    #[test]
    fn test_prune_old_entries() {
        let mut tracker = FundingRateTracker::default();

        let base_ts = 1_000_000;

        // Push entries over more than 15 minutes
        tracker.push(base_ts, 0.0001);
        tracker.push(base_ts + 5 * 60_000, 0.0002); // +5 min
        tracker.push(base_ts + 10 * 60_000, 0.0003); // +10 min

        // All should still be present
        assert_eq!(tracker.sample_count(), 3);

        // Push entry beyond 15-minute window from first entry
        tracker.push(base_ts + 16 * 60_000, 0.0004); // +16 min

        // First entry should be pruned (it's now 16 min old)
        assert_eq!(tracker.sample_count(), 3);
        assert_eq!(tracker.rate_15m_ago(), Some(0.0002)); // Second entry is now oldest
    }

    #[test]
    fn test_velocity_empty() {
        let tracker = FundingRateTracker::default();
        assert_eq!(tracker.velocity(), 0.0);
    }

    #[test]
    fn test_velocity_single_entry() {
        let mut tracker = FundingRateTracker::default();
        tracker.push(1000, 0.0001);
        assert_eq!(tracker.velocity(), 0.0);
    }

    #[test]
    fn test_velocity_positive() {
        let mut tracker = FundingRateTracker::default();

        let base_ts = 1_000_000;
        tracker.push(base_ts, 0.0001);
        tracker.push(base_ts + 60_000, 0.0003);

        // Velocity = newest - oldest = 0.0003 - 0.0001 = 0.0002
        assert!((tracker.velocity() - 0.0002).abs() < 1e-10);
    }

    #[test]
    fn test_velocity_negative() {
        let mut tracker = FundingRateTracker::default();

        let base_ts = 1_000_000;
        tracker.push(base_ts, 0.0003);
        tracker.push(base_ts + 60_000, 0.0001);

        // Velocity = newest - oldest = 0.0001 - 0.0003 = -0.0002
        assert!((tracker.velocity() - (-0.0002)).abs() < 1e-10);
    }

    #[test]
    fn test_is_spiking_false() {
        let mut tracker = FundingRateTracker::default();

        let base_ts = 1_000_000;
        tracker.push(base_ts, 0.0001);
        tracker.push(base_ts + 60_000, 0.00015); // Small change

        // Velocity = 0.00005, which is less than threshold of 0.0002
        assert!(!tracker.is_spiking());
    }

    #[test]
    fn test_is_spiking_true_positive() {
        let mut tracker = FundingRateTracker::default();

        let base_ts = 1_000_000;
        tracker.push(base_ts, 0.0001);
        tracker.push(base_ts + 60_000, 0.0004); // Large positive change

        // Velocity = 0.0003, which exceeds threshold of 0.0002
        assert!(tracker.is_spiking());
    }

    #[test]
    fn test_is_spiking_true_negative() {
        let mut tracker = FundingRateTracker::default();

        let base_ts = 1_000_000;
        tracker.push(base_ts, 0.0004);
        tracker.push(base_ts + 60_000, 0.0001); // Large negative change

        // Velocity = -0.0003, abs value exceeds threshold of 0.0002
        assert!(tracker.is_spiking());
    }

    #[test]
    fn test_is_extreme_long() {
        let tracker = FundingRateTracker::default();

        // Below threshold
        assert!(!tracker.is_extreme(0.0004));

        // At threshold (not extreme - must exceed)
        assert!(!tracker.is_extreme(0.0005));

        // Above threshold
        assert!(tracker.is_extreme(0.0006));
        assert!(tracker.is_extreme(0.001));
    }

    #[test]
    fn test_is_extreme_short() {
        let tracker = FundingRateTracker::default();

        // Above threshold (not extreme)
        assert!(!tracker.is_extreme(-0.0001));

        // At threshold (not extreme - must be below)
        assert!(!tracker.is_extreme(-0.0002));

        // Below threshold
        assert!(tracker.is_extreme(-0.0003));
        assert!(tracker.is_extreme(-0.001));
    }

    #[test]
    fn test_is_extreme_normal() {
        let tracker = FundingRateTracker::default();

        // Normal funding rates
        assert!(!tracker.is_extreme(0.0001));
        assert!(!tracker.is_extreme(0.0));
        assert!(!tracker.is_extreme(-0.0001));
    }

    #[test]
    fn test_has_sufficient_data() {
        let mut tracker = FundingRateTracker::default();

        // No data
        assert!(!tracker.has_sufficient_data());

        // One sample
        tracker.push(1000, 0.0001);
        assert!(!tracker.has_sufficient_data());

        // Two samples
        tracker.push(2000, 0.0002);
        assert!(tracker.has_sufficient_data());
    }

    #[test]
    fn test_time_span_ms() {
        let mut tracker = FundingRateTracker::default();

        // No data
        assert_eq!(tracker.time_span_ms(), 0);

        // One sample
        tracker.push(1000, 0.0001);
        assert_eq!(tracker.time_span_ms(), 0);

        // Two samples
        tracker.push(5000, 0.0002);
        assert_eq!(tracker.time_span_ms(), 4000);

        // More samples
        tracker.push(10000, 0.0003);
        assert_eq!(tracker.time_span_ms(), 9000);
    }

    #[test]
    fn test_lookback_window_boundary() {
        let mut tracker = FundingRateTracker::default();

        let base_ts = 1_000_000;

        // Push at exactly 15 minutes apart
        tracker.push(base_ts, 0.0001);
        tracker.push(base_ts + LOOKBACK_MS, 0.0002);

        // Both should be present (boundary case)
        assert_eq!(tracker.sample_count(), 2);

        // Push just beyond the window
        tracker.push(base_ts + LOOKBACK_MS + 1, 0.0003);

        // First entry should be pruned
        assert_eq!(tracker.sample_count(), 2);
    }

    #[test]
    fn test_multiple_entries_same_timestamp() {
        let mut tracker = FundingRateTracker::default();

        // This can happen if multiple venues report at the same time
        tracker.push(1000, 0.0001);
        tracker.push(1000, 0.0002);
        tracker.push(1000, 0.0003);

        // All entries should be present
        assert_eq!(tracker.sample_count(), 3);

        // Current rate should be the last pushed
        assert_eq!(tracker.current_rate(), Some(0.0003));

        // Oldest rate should be the first pushed
        assert_eq!(tracker.rate_15m_ago(), Some(0.0001));
    }

    #[test]
    fn test_send_sync() {
        // Compile-time check that FundingRateTracker is Send + Sync
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FundingRateTracker>();
    }

    #[test]
    fn test_realistic_funding_scenario() {
        let mut tracker = FundingRateTracker::default();

        let base_ts = 1_700_000_000_000; // Realistic timestamp

        // Simulate funding rate updates every minute for 20 minutes
        // Starting with normal funding, then spiking
        let rates = [
            0.0001, 0.0001, 0.00012, 0.00011, 0.0001, // Minutes 0-4: stable
            0.00015, 0.00018, 0.0002, 0.00025, 0.0003, // Minutes 5-9: rising
            0.00035, 0.0004, 0.00045, 0.0005, 0.00055, // Minutes 10-14: spiking
            0.0006, 0.00058, 0.00056, 0.00054,
            0.00052, // Minutes 15-19: extreme then stabilizing
        ];

        for (i, rate) in rates.iter().enumerate() {
            let ts = base_ts + (i as i64) * 60_000;
            tracker.push(ts, *rate);
        }

        // At minute 19 (timestamp 19*60000 from base), cutoff is at minute 4
        // So entries at minutes 0-3 are pruned, entries 4-19 remain (16 entries)
        // The window keeps entries within 15 minutes (inclusive of boundary)
        assert!(tracker.sample_count() <= 20); // At most 20, pruning removes old ones
        assert!(tracker.sample_count() >= 15); // Should retain at least 15 minutes of data

        // Current rate should be 0.00052
        assert_eq!(tracker.current_rate(), Some(0.00052));

        // Rate 15m ago should be around minute 4-5 value (after pruning)
        let oldest = tracker.rate_15m_ago().unwrap();
        // Minute 4 = 0.0001, Minute 5 = 0.00015
        assert!(
            (0.0001..=0.00015).contains(&oldest),
            "oldest rate was {}",
            oldest
        );

        // Should be spiking (large velocity over window)
        let velocity = tracker.velocity();
        println!(
            "Velocity: {:.6}, is_spiking: {}",
            velocity,
            tracker.is_spiking()
        );

        // Velocity = 0.00052 - oldest (~0.0001) = ~0.00042 > 0.0002 threshold
        assert!(tracker.is_spiking());

        // Current rate is extreme (>0.0005)
        assert!(tracker.is_extreme(0.00052));
    }

    #[test]
    fn test_negative_funding_scenario() {
        let mut tracker = FundingRateTracker::default();

        let base_ts = 1_000_000;

        // Simulate shorts dominating (negative funding)
        tracker.push(base_ts, -0.0001);
        tracker.push(base_ts + 5 * 60_000, -0.00015);
        tracker.push(base_ts + 10 * 60_000, -0.00025);

        // Velocity should be negative (becoming more negative)
        assert!(tracker.velocity() < 0.0);

        // -0.00025 is below extreme_short threshold of -0.0002
        assert!(tracker.is_extreme(-0.00025));
    }

    #[test]
    fn test_velocity_with_custom_threshold() {
        let tracker = FundingRateTracker::new(0.0001, 0.0005, -0.0002);

        // With lower spike threshold of 0.0001
        assert_eq!(tracker.spike_threshold, 0.0001);
    }

    #[test]
    fn test_trait_implementation() {
        // Verify that FundingRateTracker implements FundingTrackerTrait
        let mut tracker: Box<dyn FundingTrackerTrait> = Box::new(FundingRateTracker::default());

        tracker.push(1000, 0.0001);
        tracker.push(2000, 0.0003);

        assert!((tracker.velocity() - 0.0002).abs() < 1e-10);
        assert!(!tracker.is_spiking()); // At threshold, not spiking (need to exceed)
        assert!(!tracker.is_extreme(0.0001));
    }

    #[test]
    fn test_edge_case_velocity_at_threshold() {
        let mut tracker = FundingRateTracker::default();

        let base_ts = 1_000_000;

        // Push rates with velocity exactly at threshold
        tracker.push(base_ts, 0.0001);
        tracker.push(base_ts + 60_000, 0.0003); // Velocity = 0.0002

        // Velocity equals threshold, should NOT be spiking (must exceed)
        assert!((tracker.velocity() - 0.0002).abs() < 1e-10);
        assert!(!tracker.is_spiking());

        // Push slightly more to exceed threshold
        tracker.push(base_ts + 120_000, 0.000301);

        // Now velocity = 0.000301 - 0.0001 = 0.000201 > 0.0002
        assert!(tracker.is_spiking());
    }

    #[test]
    fn test_clone() {
        let mut tracker = FundingRateTracker::default();
        tracker.push(1000, 0.0001);
        tracker.push(2000, 0.0002);

        let cloned = tracker.clone();

        assert_eq!(cloned.sample_count(), tracker.sample_count());
        assert_eq!(cloned.current_rate(), tracker.current_rate());
        assert_eq!(cloned.velocity(), tracker.velocity());
    }
}
