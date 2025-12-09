//! 5-second micro-bar aggregation from live ticks
//!
//! Aggregates tick data into OHLCV bars for correlation calculations.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// A single 5-second OHLCV bar
#[derive(Debug, Clone)]
pub struct MicroBar {
    pub ts: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

/// Aggregates ticks into 5-second OHLC bars
pub struct MicroBarAggregator {
    bar_duration: Duration,
    current_bar_start: Option<Instant>,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: f64,
    last_ts: i64,
}

impl MicroBarAggregator {
    pub fn new() -> Self {
        Self {
            bar_duration: Duration::from_secs(5),
            current_bar_start: None,
            open: 0.0,
            high: f64::MIN,
            low: f64::MAX,
            close: 0.0,
            volume: 0.0,
            last_ts: 0,
        }
    }

    /// Returns Some(bar) when a 5-second bar completes
    pub fn update(&mut self, price: f64, size: f64, ts: i64) -> Option<MicroBar> {
        // Guard: ignore invalid prices
        if price <= 0.0 {
            return None;
        }

        let now = Instant::now();
        self.last_ts = ts;

        match self.current_bar_start {
            None => {
                // Start first bar
                self.current_bar_start = Some(now);
                self.open = price;
                self.high = price;
                self.low = price;
                self.close = price;
                self.volume = size;
                None
            }
            Some(start) if now.duration_since(start) >= self.bar_duration => {
                // Bar complete - emit and start new
                let bar = MicroBar {
                    ts,
                    open: self.open,
                    high: self.high,
                    low: self.low,
                    close: self.close,
                    volume: self.volume,
                };

                // Reset for new bar
                self.current_bar_start = Some(now);
                self.open = price;
                self.high = price;
                self.low = price;
                self.close = price;
                self.volume = size;

                Some(bar)
            }
            Some(_) => {
                // Update current bar
                self.high = self.high.max(price);
                self.low = self.low.min(price);
                self.close = price;
                self.volume += size;
                None
            }
        }
    }

    /// Get current incomplete bar's close price (for display)
    pub fn current_price(&self) -> f64 {
        self.close
    }

    /// Check if we have received any data
    pub fn has_data(&self) -> bool {
        self.current_bar_start.is_some()
    }
}

impl Default for MicroBarAggregator {
    fn default() -> Self {
        Self::new()
    }
}

/// Ring buffer for storing N most recent bars
pub struct BarBuffer {
    bars: VecDeque<MicroBar>,
    max_size: usize,
}

impl BarBuffer {
    pub fn new(max_size: usize) -> Self {
        Self {
            bars: VecDeque::with_capacity(max_size),
            max_size,
        }
    }

    pub fn push(&mut self, bar: MicroBar) {
        if self.bars.len() >= self.max_size {
            self.bars.pop_front();
        }
        self.bars.push_back(bar);
    }

    pub fn len(&self) -> usize {
        self.bars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bars.is_empty()
    }

    /// Get last N bars as references
    pub fn last_n(&self, n: usize) -> Vec<&MicroBar> {
        let start = self.bars.len().saturating_sub(n);
        self.bars.range(start..).collect()
    }

    /// Calculate bar-to-bar returns for last N bars
    /// Returns N-1 returns for N bars
    pub fn returns(&self, n: usize) -> Vec<f64> {
        let bars: Vec<_> = self.last_n(n + 1);
        if bars.len() < 2 {
            return vec![];
        }
        bars.windows(2)
            .map(|w| {
                if w[0].close > 0.0 {
                    (w[1].close - w[0].close) / w[0].close
                } else {
                    0.0
                }
            })
            .collect()
    }

    /// Get the most recent bar's close price
    pub fn last_close(&self) -> Option<f64> {
        self.bars.back().map(|b| b.close)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bar_buffer_returns() {
        let mut buffer = BarBuffer::new(10);

        // Add bars with known closes: 100, 101, 102
        buffer.push(MicroBar { ts: 1, open: 100.0, high: 100.0, low: 100.0, close: 100.0, volume: 1.0 });
        buffer.push(MicroBar { ts: 2, open: 101.0, high: 101.0, low: 101.0, close: 101.0, volume: 1.0 });
        buffer.push(MicroBar { ts: 3, open: 102.0, high: 102.0, low: 102.0, close: 102.0, volume: 1.0 });

        let returns = buffer.returns(3);
        assert_eq!(returns.len(), 2);
        assert!((returns[0] - 0.01).abs() < 0.0001); // 1% return
        assert!((returns[1] - 0.0099).abs() < 0.001); // ~0.99% return
    }

    #[test]
    fn test_bar_buffer_ring_behavior() {
        let mut buffer = BarBuffer::new(3);

        for i in 0..5 {
            buffer.push(MicroBar {
                ts: i,
                open: i as f64,
                high: i as f64,
                low: i as f64,
                close: i as f64,
                volume: 1.0
            });
        }

        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.last_close(), Some(4.0));
    }
}
