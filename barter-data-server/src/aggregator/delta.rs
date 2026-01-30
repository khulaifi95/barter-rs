//! Delta tracking for buy/sell volume analysis.
//!
//! Delta = Buy Volume - Sell Volume
//! CVD (Cumulative Volume Delta) = Running sum of delta

use barter_instrument::Side;

/// Tracks buy/sell volume delta within a bar.
#[derive(Debug, Clone, Default)]
pub struct DeltaTracker {
    /// Total buy volume in the current period
    pub buy_volume: f64,
    /// Total sell volume in the current period
    pub sell_volume: f64,
    /// Number of buy trades
    pub buy_count: u64,
    /// Number of sell trades
    pub sell_count: u64,
}

impl DeltaTracker {
    /// Create a new delta tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a trade to the tracker.
    pub fn add_trade(&mut self, side: Option<Side>, volume: f64) {
        match side {
            Some(Side::Buy) => {
                self.buy_volume += volume;
                self.buy_count += 1;
            }
            Some(Side::Sell) => {
                self.sell_volume += volume;
                self.sell_count += 1;
            }
            None => {
                // No aggressor side - split evenly (conservative approach)
                self.buy_volume += volume / 2.0;
                self.sell_volume += volume / 2.0;
            }
        }
    }

    /// Get the delta (buy - sell) for the current period.
    #[inline]
    pub fn delta(&self) -> f64 {
        self.buy_volume - self.sell_volume
    }

    /// Get the total volume (buy + sell).
    #[inline]
    #[allow(dead_code)]
    pub fn total_volume(&self) -> f64 {
        self.buy_volume + self.sell_volume
    }

    /// Get the buy ratio (0.0 - 1.0).
    #[inline]
    #[allow(dead_code)]
    pub fn buy_ratio(&self) -> f64 {
        let total = self.total_volume();
        if total > 0.0 {
            self.buy_volume / total
        } else {
            0.5
        }
    }

    /// Reset the tracker for the next period.
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.buy_volume = 0.0;
        self.sell_volume = 0.0;
        self.buy_count = 0;
        self.sell_count = 0;
    }

    /// Take the current values and reset.
    #[allow(dead_code)]
    pub fn take(&mut self) -> DeltaTracker {
        let snapshot = self.clone();
        self.reset();
        snapshot
    }
}

/// Cumulative Volume Delta tracker (persists across bars).
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct CvdTracker {
    /// Cumulative delta since tracking started
    pub cvd: f64,
    /// Number of bars processed
    pub bars_processed: u64,
}

#[allow(dead_code)]
impl CvdTracker {
    /// Create a new CVD tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a bar's delta to the cumulative total.
    pub fn add_bar_delta(&mut self, delta: f64) {
        self.cvd += delta;
        self.bars_processed += 1;
    }

    /// Reset CVD to zero.
    pub fn reset(&mut self) {
        self.cvd = 0.0;
        self.bars_processed = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delta_tracker_basic() {
        let mut tracker = DeltaTracker::new();

        tracker.add_trade(Some(Side::Buy), 100.0);
        tracker.add_trade(Some(Side::Sell), 60.0);

        assert_eq!(tracker.buy_volume, 100.0);
        assert_eq!(tracker.sell_volume, 60.0);
        assert_eq!(tracker.delta(), 40.0);
        assert_eq!(tracker.total_volume(), 160.0);
    }

    #[test]
    fn test_delta_tracker_no_aggressor() {
        let mut tracker = DeltaTracker::new();

        tracker.add_trade(None, 100.0);

        assert_eq!(tracker.buy_volume, 50.0);
        assert_eq!(tracker.sell_volume, 50.0);
        assert_eq!(tracker.delta(), 0.0);
    }

    #[test]
    fn test_delta_tracker_reset() {
        let mut tracker = DeltaTracker::new();
        tracker.add_trade(Some(Side::Buy), 100.0);

        tracker.reset();

        assert_eq!(tracker.buy_volume, 0.0);
        assert_eq!(tracker.sell_volume, 0.0);
        assert_eq!(tracker.buy_count, 0);
    }

    #[test]
    fn test_cvd_tracker() {
        let mut cvd = CvdTracker::new();

        cvd.add_bar_delta(10.0);
        cvd.add_bar_delta(-5.0);
        cvd.add_bar_delta(15.0);

        assert_eq!(cvd.cvd, 20.0);
        assert_eq!(cvd.bars_processed, 3);
    }

    #[test]
    fn test_buy_ratio() {
        let mut tracker = DeltaTracker::new();

        // 75% buy
        tracker.add_trade(Some(Side::Buy), 75.0);
        tracker.add_trade(Some(Side::Sell), 25.0);

        assert!((tracker.buy_ratio() - 0.75).abs() < 0.001);
    }
}
