//! Large trade detection.
//!
//! Detects trades exceeding configurable notional thresholds:
//! - LARGE: $2M+
//! - WHALE: $5M+
//! - MEGA: $10M+

use crate::config::LargeTradesConfig;
use chrono::Utc;

/// Category of large trade based on notional size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LargeTradeCategory {
    /// $2M+ notional
    Large,
    /// $5M+ notional
    Whale,
    /// $10M+ notional
    Mega,
}

impl LargeTradeCategory {
    /// Get string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            LargeTradeCategory::Large => "LARGE",
            LargeTradeCategory::Whale => "WHALE",
            LargeTradeCategory::Mega => "MEGA",
        }
    }
}

impl std::fmt::Display for LargeTradeCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A detected large trade.
#[derive(Debug, Clone)]
pub struct LargeTrade {
    /// Trade timestamp (nanoseconds)
    pub ts_event: i64,
    /// Processing timestamp (nanoseconds)
    pub ts_init: i64,
    /// Instrument identifier
    pub instrument_id: String,
    /// Trade ID from exchange
    pub trade_id: String,
    /// Trade price
    pub price: f64,
    /// Trade size
    pub size: f64,
    /// Trade side ("BUY" or "SELL")
    pub side: String,
    /// Notional value in USD
    pub notional_usd: f64,
    /// Category based on notional
    pub category: LargeTradeCategory,
    /// Threshold that was exceeded (USD)
    pub threshold_used: f64,
    /// Threshold mode ("absolute", "percentile", etc.)
    pub threshold_mode: String,
}

/// Detector for large trades.
pub struct LargeTradeDetector {
    large_threshold: f64,
    whale_threshold: f64,
    mega_threshold: f64,
    threshold_mode: String,
}

impl LargeTradeDetector {
    /// Create a new detector with the given configuration.
    pub fn new(config: &LargeTradesConfig) -> Self {
        Self {
            large_threshold: config.large_threshold_usd,
            whale_threshold: config.whale_threshold_usd,
            mega_threshold: config.mega_threshold_usd,
            threshold_mode: config.threshold_mode.clone(),
        }
    }

    /// Create with explicit thresholds.
    pub fn with_thresholds(large: f64, whale: f64, mega: f64) -> Self {
        Self {
            large_threshold: large,
            whale_threshold: whale,
            mega_threshold: mega,
            threshold_mode: "absolute".to_string(),
        }
    }

    /// Check if a trade qualifies as large and return its category.
    pub fn classify(&self, notional_usd: f64) -> Option<LargeTradeCategory> {
        if notional_usd >= self.mega_threshold {
            Some(LargeTradeCategory::Mega)
        } else if notional_usd >= self.whale_threshold {
            Some(LargeTradeCategory::Whale)
        } else if notional_usd >= self.large_threshold {
            Some(LargeTradeCategory::Large)
        } else {
            None
        }
    }

    /// Get the threshold used for a category.
    pub fn threshold_for_category(&self, category: LargeTradeCategory) -> f64 {
        match category {
            LargeTradeCategory::Large => self.large_threshold,
            LargeTradeCategory::Whale => self.whale_threshold,
            LargeTradeCategory::Mega => self.mega_threshold,
        }
    }

    /// Process a trade and return LargeTrade if it qualifies.
    pub fn process_trade(
        &self,
        ts_event: i64,
        instrument_id: &str,
        trade_id: &str,
        price: f64,
        size: f64,
        side: &str,
    ) -> Option<LargeTrade> {
        let notional_usd = price * size;

        let category = self.classify(notional_usd)?;
        let threshold_used = self.threshold_for_category(category);

        Some(LargeTrade {
            ts_event,
            ts_init: Utc::now().timestamp_nanos_opt().unwrap_or(0),
            instrument_id: instrument_id.to_string(),
            trade_id: trade_id.to_string(),
            price,
            size,
            side: side.to_string(),
            notional_usd,
            category,
            threshold_used,
            threshold_mode: self.threshold_mode.clone(),
        })
    }

    /// Get the minimum threshold (LARGE).
    pub fn min_threshold(&self) -> f64 {
        self.large_threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_detector() -> LargeTradeDetector {
        LargeTradeDetector::with_thresholds(2_000_000.0, 5_000_000.0, 10_000_000.0)
    }

    #[test]
    fn test_classify_large() {
        let detector = make_detector();
        assert_eq!(
            detector.classify(2_000_000.0),
            Some(LargeTradeCategory::Large)
        );
        assert_eq!(
            detector.classify(3_000_000.0),
            Some(LargeTradeCategory::Large)
        );
        assert_eq!(
            detector.classify(4_999_999.0),
            Some(LargeTradeCategory::Large)
        );
    }

    #[test]
    fn test_classify_whale() {
        let detector = make_detector();
        assert_eq!(
            detector.classify(5_000_000.0),
            Some(LargeTradeCategory::Whale)
        );
        assert_eq!(
            detector.classify(7_500_000.0),
            Some(LargeTradeCategory::Whale)
        );
        assert_eq!(
            detector.classify(9_999_999.0),
            Some(LargeTradeCategory::Whale)
        );
    }

    #[test]
    fn test_classify_mega() {
        let detector = make_detector();
        assert_eq!(
            detector.classify(10_000_000.0),
            Some(LargeTradeCategory::Mega)
        );
        assert_eq!(
            detector.classify(50_000_000.0),
            Some(LargeTradeCategory::Mega)
        );
    }

    #[test]
    fn test_classify_below_threshold() {
        let detector = make_detector();
        assert_eq!(detector.classify(1_999_999.0), None);
        assert_eq!(detector.classify(1_000_000.0), None);
        assert_eq!(detector.classify(100.0), None);
    }

    #[test]
    fn test_process_trade_large() {
        let detector = make_detector();

        // $2.5M trade (25 BTC @ $100,000)
        let result = detector.process_trade(
            1_706_832_000_000_000_000,
            "BTCUSDT-PERP.BINANCE",
            "trade123",
            100_000.0,
            25.0,
            "BUY",
        );

        assert!(result.is_some());
        let trade = result.unwrap();
        assert_eq!(trade.category, LargeTradeCategory::Large);
        assert_eq!(trade.notional_usd, 2_500_000.0);
        assert_eq!(trade.threshold_used, 2_000_000.0);
    }

    #[test]
    fn test_process_trade_below_threshold() {
        let detector = make_detector();

        // $500K trade (5 BTC @ $100,000)
        let result = detector.process_trade(
            1_706_832_000_000_000_000,
            "BTCUSDT-PERP.BINANCE",
            "trade123",
            100_000.0,
            5.0,
            "SELL",
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_category_as_str() {
        assert_eq!(LargeTradeCategory::Large.as_str(), "LARGE");
        assert_eq!(LargeTradeCategory::Whale.as_str(), "WHALE");
        assert_eq!(LargeTradeCategory::Mega.as_str(), "MEGA");
    }

    #[test]
    fn test_threshold_for_category() {
        let detector = make_detector();
        assert_eq!(
            detector.threshold_for_category(LargeTradeCategory::Large),
            2_000_000.0
        );
        assert_eq!(
            detector.threshold_for_category(LargeTradeCategory::Whale),
            5_000_000.0
        );
        assert_eq!(
            detector.threshold_for_category(LargeTradeCategory::Mega),
            10_000_000.0
        );
    }
}
