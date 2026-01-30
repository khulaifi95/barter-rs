//! 1-minute bar aggregation from trade data.
//!
//! Accumulates trades and emits completed bars at minute boundaries.

use crate::aggregator::delta::DeltaTracker;
use crate::parquet::writer::BarEvent;
use barter_instrument::Side;
use chrono::Utc;
use std::collections::HashMap;

/// A completed 1-minute OHLCV bar.
#[derive(Debug, Clone)]
pub struct MinuteBar {
    /// Instrument identifier
    pub instrument_id: String,
    /// Bar type string for Nautilus
    pub bar_type: String,
    /// Open price
    pub open: f64,
    /// High price
    pub high: f64,
    /// Low price
    pub low: f64,
    /// Close price
    pub close: f64,
    /// Total volume
    pub volume: f64,
    /// Quote volume (price * size)
    pub quote_volume: f64,
    /// Bar open timestamp (start of minute) in nanoseconds
    pub ts_open_ns: u64,
    /// Bar close timestamp (end of minute) in nanoseconds - this is ts_event
    pub ts_close_ns: u64,
    /// Ingest timestamp in nanoseconds - this is ts_init
    pub ts_init_ns: u64,
    /// Number of trades in this bar
    pub trade_count: u64,
    /// Delta tracker with buy/sell breakdown
    pub delta: DeltaTracker,
    /// Price precision for encoding
    pub price_precision: u8,
    /// Size precision for encoding
    pub size_precision: u8,
}

impl MinuteBar {
    /// Convert to a BarEvent for Parquet writing.
    pub fn to_bar_event(&self) -> BarEvent {
        BarEvent {
            bar_type: self.bar_type.clone(),
            instrument_id: self.instrument_id.clone(),
            open: self.open,
            high: self.high,
            low: self.low,
            close: self.close,
            volume: self.volume,
            // ts_event = bar CLOSE time per Nautilus spec
            ts_event_ns: self.ts_close_ns,
            ts_init_ns: self.ts_init_ns,
            price_precision: self.price_precision,
            size_precision: self.size_precision,
        }
    }
}

/// Builder for accumulating trades into a 1-minute bar.
#[derive(Debug, Clone)]
pub struct MinuteBarBuilder {
    instrument_id: String,
    bar_type: String,
    open: Option<f64>,
    high: f64,
    low: f64,
    close: f64,
    volume: f64,
    quote_volume: f64,
    /// Minute start timestamp (aligned to minute boundary)
    minute_start_ns: u64,
    trade_count: u64,
    delta: DeltaTracker,
    price_precision: u8,
    size_precision: u8,
}

impl MinuteBarBuilder {
    /// Create a new bar builder for an instrument.
    pub fn new(
        instrument_id: String,
        bar_type: String,
        minute_start_ns: u64,
        price_precision: u8,
        size_precision: u8,
    ) -> Self {
        Self {
            instrument_id,
            bar_type,
            open: None,
            high: f64::MIN,
            low: f64::MAX,
            close: 0.0,
            volume: 0.0,
            quote_volume: 0.0,
            minute_start_ns,
            trade_count: 0,
            delta: DeltaTracker::new(),
            price_precision,
            size_precision,
        }
    }

    /// Add a trade to the bar.
    pub fn add_trade(&mut self, price: f64, size: f64, side: Option<Side>) {
        // First trade sets open
        if self.open.is_none() {
            self.open = Some(price);
        }

        // Update OHLC
        if price > self.high {
            self.high = price;
        }
        if price < self.low {
            self.low = price;
        }
        self.close = price;

        // Update volume
        self.volume += size;
        self.quote_volume += price * size;
        self.trade_count += 1;

        // Track delta
        self.delta.add_trade(side, size);
    }

    /// Check if this builder has any trades.
    pub fn has_trades(&self) -> bool {
        self.trade_count > 0
    }

    /// Get the minute start timestamp.
    pub fn minute_start_ns(&self) -> u64 {
        self.minute_start_ns
    }

    /// Finalize and return the completed bar.
    pub fn finish(self) -> Option<MinuteBar> {
        // Don't emit empty bars
        if self.open.is_none() {
            return None;
        }

        let open = self.open.unwrap();
        let ts_close_ns = self.minute_start_ns + 60_000_000_000; // +60 seconds
        let ts_init_ns = Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64;

        Some(MinuteBar {
            instrument_id: self.instrument_id,
            bar_type: self.bar_type,
            open,
            high: self.high,
            low: self.low,
            close: self.close,
            volume: self.volume,
            quote_volume: self.quote_volume,
            ts_open_ns: self.minute_start_ns,
            ts_close_ns,
            ts_init_ns,
            trade_count: self.trade_count,
            delta: self.delta,
            price_precision: self.price_precision,
            size_precision: self.size_precision,
        })
    }
}

/// Aggregates trades into 1-minute bars for multiple instruments.
pub struct MinuteBarAggregator {
    /// Current bar builders per instrument
    builders: HashMap<String, MinuteBarBuilder>,
    /// Default price precision
    default_price_precision: u8,
    /// Default size precision
    default_size_precision: u8,
}

impl MinuteBarAggregator {
    /// Create a new aggregator.
    pub fn new(default_price_precision: u8, default_size_precision: u8) -> Self {
        Self {
            builders: HashMap::new(),
            default_price_precision,
            default_size_precision,
        }
    }

    /// Process a trade event, returning a completed bar if minute boundary crossed.
    pub fn process_trade(
        &mut self,
        instrument_id: &str,
        price: f64,
        size: f64,
        side: Option<Side>,
        ts_event_ns: u64,
        price_precision: u8,
        size_precision: u8,
    ) -> Option<MinuteBar> {
        let minute_start_ns = align_to_minute(ts_event_ns);

        // Check if we need to emit a completed bar
        let completed_bar = if let Some(builder) = self.builders.get(instrument_id) {
            if builder.minute_start_ns() != minute_start_ns && builder.has_trades() {
                // Minute boundary crossed - take the old builder
                self.builders.remove(instrument_id).and_then(|b| b.finish())
            } else {
                None
            }
        } else {
            None
        };

        // Get or create builder for current minute
        let bar_type = format!("{}-1-MINUTE-LAST-EXTERNAL", instrument_id);
        let price_precision = if price_precision == 0 {
            self.default_price_precision
        } else {
            price_precision
        };
        let size_precision = if size_precision == 0 {
            self.default_size_precision
        } else {
            size_precision
        };
        let builder = self.builders.entry(instrument_id.to_string()).or_insert_with(|| {
            MinuteBarBuilder::new(
                instrument_id.to_string(),
                bar_type,
                minute_start_ns,
                price_precision,
                size_precision,
            )
        });

        // If the builder is for an old minute (shouldn't happen after above check, but be safe)
        if builder.minute_start_ns() != minute_start_ns {
            let bar_type = format!("{}-1-MINUTE-LAST-EXTERNAL", instrument_id);
            *builder = MinuteBarBuilder::new(
                instrument_id.to_string(),
                bar_type,
                minute_start_ns,
                price_precision,
                size_precision,
            );
        }

        // Add the trade
        builder.add_trade(price, size, side);

        completed_bar
    }

    /// Flush all current bars (e.g., at shutdown).
    #[allow(dead_code)]
    pub fn flush_all(&mut self) -> Vec<MinuteBar> {
        let mut bars = Vec::new();
        for (_, builder) in self.builders.drain() {
            if let Some(bar) = builder.finish() {
                bars.push(bar);
            }
        }
        bars
    }

    /// Get the number of instruments being tracked.
    #[allow(dead_code)]
    pub fn instrument_count(&self) -> usize {
        self.builders.len()
    }
}

/// Align a nanosecond timestamp to the start of its minute.
#[inline]
fn align_to_minute(ts_ns: u64) -> u64 {
    let minute_ns = 60_000_000_000u64;
    (ts_ns / minute_ns) * minute_ns
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_align_to_minute() {
        // 15:30:45.123456789 -> 15:30:00.000000000
        let ts = 1706540445_123_456_789u64;
        let aligned = align_to_minute(ts);
        assert_eq!(aligned % 60_000_000_000, 0);
    }

    #[test]
    fn test_bar_builder_basic() {
        let mut builder = MinuteBarBuilder::new(
            "BTCUSDT-PERP.BINANCE".to_string(),
            "BTCUSDT-PERP.BINANCE-1-MINUTE-LAST-EXTERNAL".to_string(),
            1706540400_000_000_000,
            2,
            3,
        );

        builder.add_trade(100_000.0, 0.1, Some(Side::Buy));
        builder.add_trade(100_050.0, 0.2, Some(Side::Sell));
        builder.add_trade(99_950.0, 0.15, Some(Side::Buy));

        assert!(builder.has_trades());
        let bar = builder.finish().unwrap();

        assert_eq!(bar.open, 100_000.0);
        assert_eq!(bar.high, 100_050.0);
        assert_eq!(bar.low, 99_950.0);
        assert_eq!(bar.close, 99_950.0);
        assert!((bar.volume - 0.45).abs() < 0.001);
        assert_eq!(bar.trade_count, 3);
    }

    #[test]
    fn test_bar_builder_empty() {
        let builder = MinuteBarBuilder::new(
            "BTCUSDT-PERP.BINANCE".to_string(),
            "BTCUSDT-PERP.BINANCE-1-MINUTE-LAST-EXTERNAL".to_string(),
            1706540400_000_000_000,
            2,
            3,
        );

        assert!(!builder.has_trades());
        assert!(builder.finish().is_none());
    }

    #[test]
    fn test_aggregator_minute_boundary() {
        let mut agg = MinuteBarAggregator::new(2, 3);

        // First minute
        let minute1_start = 1706540400_000_000_000u64;
        let result = agg.process_trade(
            "BTCUSDT-PERP.BINANCE",
            100_000.0,
            0.1,
            Some(Side::Buy),
            minute1_start + 1_000_000_000, // 1 second in
            2,
            3,
        );
        assert!(result.is_none()); // No bar yet

        // Cross minute boundary
        let minute2_start = minute1_start + 60_000_000_000;
        let result = agg.process_trade(
            "BTCUSDT-PERP.BINANCE",
            100_100.0,
            0.2,
            Some(Side::Buy),
            minute2_start + 1_000_000_000,
            2,
            3,
        );

        // Should emit the first bar
        assert!(result.is_some());
        let bar = result.unwrap();
        assert_eq!(bar.open, 100_000.0);
        assert_eq!(bar.close, 100_000.0);
        assert!((bar.volume - 0.1).abs() < 0.001);
    }

    #[test]
    fn test_bar_timestamps() {
        let minute_start = 1706540400_000_000_000u64;
        let mut builder = MinuteBarBuilder::new(
            "BTCUSDT-PERP.BINANCE".to_string(),
            "BTCUSDT-PERP.BINANCE-1-MINUTE-LAST-EXTERNAL".to_string(),
            minute_start,
            2,
            3,
        );

        builder.add_trade(100_000.0, 0.1, Some(Side::Buy));
        let bar = builder.finish().unwrap();

        // ts_open should be minute start
        assert_eq!(bar.ts_open_ns, minute_start);
        // ts_close should be minute end (ts_open + 60s)
        assert_eq!(bar.ts_close_ns, minute_start + 60_000_000_000);
    }
}
