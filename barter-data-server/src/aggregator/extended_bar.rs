//! Extended bar with additional market data (OI, funding, delta, L2, liquidations).
//!
//! Extends the standard OHLCV bar with:
//! - Open Interest (OI)
//! - Funding Rate
//! - Buy/Sell Volume Delta
//! - CVD (Cumulative Volume Delta)
//! - Liquidation aggregates (1m)
//! - L2 depth bands (10/50/100 bps, base + notional)

use crate::aggregator::minute_bar::MinuteBar;
use barter_instrument::Side;

/// Extended bar with OI, funding rate, and delta metrics.
#[derive(Debug, Clone)]
pub struct ExtendedBar {
    // Core OHLCV
    pub instrument_id: String,
    #[allow(dead_code)]
    pub bar_type: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub quote_volume: f64,

    // Timestamps
    /// Bar open time (start of minute) - for TradingView alignment
    pub ts_open_ns: u64,
    /// Bar close time (end of minute) - this is ts_event per Nautilus
    pub ts_close_ns: u64,
    /// Ingest time - this is ts_init
    pub ts_init_ns: u64,

    // Extended metrics
    /// Open Interest at bar close (if available)
    pub open_interest: f64,
    /// Open Interest change since previous bar
    pub oi_change: f64,
    /// Funding rate at bar close (if available)
    pub funding_rate: f64,
    /// Buy volume in this bar
    pub buy_volume: f64,
    /// Sell volume in this bar
    pub sell_volume: f64,
    /// Delta (buy - sell) for this bar
    pub delta: f64,
    /// Cumulative Volume Delta up to this bar
    pub cvd: f64,
    /// Number of trades
    pub trade_count: u64,
    /// Best bid price at bar close
    pub bid_price: f64,
    /// Best bid size at bar close
    pub bid_size: f64,
    /// Best ask price at bar close
    pub ask_price: f64,
    /// Best ask size at bar close
    pub ask_size: f64,
    /// Spread in basis points
    pub spread_bps: f64,
    /// Order book imbalance
    pub book_imbalance: f64,

    // Liquidation aggregates (quote notional)
    pub liq_buy_usd: f64,
    pub liq_sell_usd: f64,
    pub liq_total_usd: f64,
    pub liq_count: u64,

    // L2 depth bands (base + notional, 1m snapshot)
    pub bid_depth_10bps_base: f64,
    pub ask_depth_10bps_base: f64,
    pub bid_depth_10bps_usd: f64,
    pub ask_depth_10bps_usd: f64,
    pub depth_imb_10bps: f64,
    pub bid_depth_50bps_base: f64,
    pub ask_depth_50bps_base: f64,
    pub bid_depth_50bps_usd: f64,
    pub ask_depth_50bps_usd: f64,
    pub depth_imb_50bps: f64,
    pub bid_depth_100bps_base: f64,
    pub ask_depth_100bps_base: f64,
    pub bid_depth_100bps_usd: f64,
    pub ask_depth_100bps_usd: f64,
    pub depth_imb_100bps: f64,

    // Precision
    pub price_precision: u8,
    pub size_precision: u8,
}

/// L2 depth bands (base + notional within bps of mid).
#[derive(Debug, Clone, Default)]
pub struct DepthBands {
    pub bid_depth_10bps_base: f64,
    pub ask_depth_10bps_base: f64,
    pub bid_depth_10bps_usd: f64,
    pub ask_depth_10bps_usd: f64,
    pub depth_imb_10bps: f64,
    pub bid_depth_50bps_base: f64,
    pub ask_depth_50bps_base: f64,
    pub bid_depth_50bps_usd: f64,
    pub ask_depth_50bps_usd: f64,
    pub depth_imb_50bps: f64,
    pub bid_depth_100bps_base: f64,
    pub ask_depth_100bps_base: f64,
    pub bid_depth_100bps_usd: f64,
    pub ask_depth_100bps_usd: f64,
    pub depth_imb_100bps: f64,
}

impl DepthBands {
    #[allow(dead_code)]
    pub fn scaled(self, multiplier: f64) -> Self {
        if (multiplier - 1.0).abs() < f64::EPSILON {
            return self;
        }
        Self {
            bid_depth_10bps_base: self.bid_depth_10bps_base * multiplier,
            ask_depth_10bps_base: self.ask_depth_10bps_base * multiplier,
            bid_depth_10bps_usd: self.bid_depth_10bps_usd * multiplier,
            ask_depth_10bps_usd: self.ask_depth_10bps_usd * multiplier,
            depth_imb_10bps: self.depth_imb_10bps,
            bid_depth_50bps_base: self.bid_depth_50bps_base * multiplier,
            ask_depth_50bps_base: self.ask_depth_50bps_base * multiplier,
            bid_depth_50bps_usd: self.bid_depth_50bps_usd * multiplier,
            ask_depth_50bps_usd: self.ask_depth_50bps_usd * multiplier,
            depth_imb_50bps: self.depth_imb_50bps,
            bid_depth_100bps_base: self.bid_depth_100bps_base * multiplier,
            ask_depth_100bps_base: self.ask_depth_100bps_base * multiplier,
            bid_depth_100bps_usd: self.bid_depth_100bps_usd * multiplier,
            ask_depth_100bps_usd: self.ask_depth_100bps_usd * multiplier,
            depth_imb_100bps: self.depth_imb_100bps,
        }
    }
}

impl ExtendedBar {
    /// Create an extended bar from a minute bar and additional metrics.
    #[allow(clippy::too_many_arguments)]
    pub fn from_minute_bar(
        bar: MinuteBar,
        open_interest: f64,
        oi_change: f64,
        funding_rate: f64,
        bid_price: f64,
        bid_size: f64,
        ask_price: f64,
        ask_size: f64,
        spread_bps: f64,
        book_imbalance: f64,
        cvd: f64,
        liq_buy_usd: f64,
        liq_sell_usd: f64,
        liq_total_usd: f64,
        liq_count: u64,
        depth_bands: DepthBands,
    ) -> Self {
        Self {
            instrument_id: bar.instrument_id,
            bar_type: bar.bar_type,
            open: bar.open,
            high: bar.high,
            low: bar.low,
            close: bar.close,
            volume: bar.volume,
            quote_volume: bar.quote_volume,
            ts_open_ns: bar.ts_open_ns,
            ts_close_ns: bar.ts_close_ns,
            ts_init_ns: bar.ts_init_ns,
            open_interest,
            oi_change,
            funding_rate,
            buy_volume: bar.delta.buy_volume,
            sell_volume: bar.delta.sell_volume,
            delta: bar.delta.delta(),
            cvd,
            trade_count: bar.trade_count,
            bid_price,
            bid_size,
            ask_price,
            ask_size,
            spread_bps,
            book_imbalance,
            liq_buy_usd,
            liq_sell_usd,
            liq_total_usd,
            liq_count,
            bid_depth_10bps_base: depth_bands.bid_depth_10bps_base,
            ask_depth_10bps_base: depth_bands.ask_depth_10bps_base,
            bid_depth_10bps_usd: depth_bands.bid_depth_10bps_usd,
            ask_depth_10bps_usd: depth_bands.ask_depth_10bps_usd,
            depth_imb_10bps: depth_bands.depth_imb_10bps,
            bid_depth_50bps_base: depth_bands.bid_depth_50bps_base,
            ask_depth_50bps_base: depth_bands.ask_depth_50bps_base,
            bid_depth_50bps_usd: depth_bands.bid_depth_50bps_usd,
            ask_depth_50bps_usd: depth_bands.ask_depth_50bps_usd,
            depth_imb_50bps: depth_bands.depth_imb_50bps,
            bid_depth_100bps_base: depth_bands.bid_depth_100bps_base,
            ask_depth_100bps_base: depth_bands.ask_depth_100bps_base,
            bid_depth_100bps_usd: depth_bands.bid_depth_100bps_usd,
            ask_depth_100bps_usd: depth_bands.ask_depth_100bps_usd,
            depth_imb_100bps: depth_bands.depth_imb_100bps,
            price_precision: bar.price_precision,
            size_precision: bar.size_precision,
        }
    }

    /// Get the VWAP for this bar (approximated from OHLC midpoint weighted by volume).
    ///
    /// Note: True VWAP requires tick-by-tick data. This is an approximation.
    #[allow(dead_code)]
    pub fn approx_vwap(&self) -> f64 {
        (self.open + self.high + self.low + self.close) / 4.0
    }

    /// Get the bar range (high - low).
    #[allow(dead_code)]
    pub fn range(&self) -> f64 {
        self.high - self.low
    }

    /// Get the bar body (close - open), positive for bullish.
    #[allow(dead_code)]
    pub fn body(&self) -> f64 {
        self.close - self.open
    }

    /// Check if the bar is bullish (close >= open).
    #[allow(dead_code)]
    pub fn is_bullish(&self) -> bool {
        self.close >= self.open
    }

    /// Get the buy ratio (0.0 - 1.0).
    #[allow(dead_code)]
    pub fn buy_ratio(&self) -> f64 {
        if self.volume > 0.0 {
            self.buy_volume / self.volume
        } else {
            0.5
        }
    }
}

/// Builder for extended bars that accumulates OI and funding updates.
#[derive(Debug, Default)]
pub struct ExtendedBarBuilder {
    /// Latest OI value
    pub latest_oi: Option<f64>,
    /// Last bar close OI value (for bar-to-bar delta)
    pub last_bar_oi: Option<f64>,
    /// Latest funding rate
    pub latest_funding: Option<f64>,
    /// Latest L1 bid/ask
    pub latest_bid_price: Option<f64>,
    pub latest_bid_size: Option<f64>,
    pub latest_ask_price: Option<f64>,
    pub latest_ask_size: Option<f64>,
    /// Running CVD
    pub cvd: f64,
    /// Liquidation aggregates (1m)
    pub liq_buy_usd: f64,
    pub liq_sell_usd: f64,
    pub liq_count: u64,
    /// Latest L2 depth bands snapshot
    pub depth_bands: Option<DepthBands>,
}

impl ExtendedBarBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update OI value.
    pub fn update_oi(&mut self, oi: f64) {
        self.latest_oi = Some(oi);
    }

    /// Update funding rate.
    pub fn update_funding(&mut self, rate: f64) {
        self.latest_funding = Some(rate);
    }

    /// Update L1 bid/ask snapshot.
    pub fn update_l1(&mut self, bid_price: f64, bid_size: f64, ask_price: f64, ask_size: f64) {
        self.latest_bid_price = Some(bid_price);
        self.latest_bid_size = Some(bid_size);
        self.latest_ask_price = Some(ask_price);
        self.latest_ask_size = Some(ask_size);
    }

    /// Update L2 depth bands snapshot.
    pub fn update_depth_bands(&mut self, bands: Option<DepthBands>) {
        self.depth_bands = bands;
    }

    /// Update liquidation aggregates for this minute.
    pub fn update_liquidation(&mut self, notional_usd: f64, side: Side) {
        match side {
            Side::Buy => self.liq_buy_usd += notional_usd,
            Side::Sell => self.liq_sell_usd += notional_usd,
        }
        self.liq_count += 1;
    }

    /// Build an extended bar from a minute bar.
    pub fn build(&mut self, bar: MinuteBar) -> ExtendedBar {
        // Update CVD
        let bar_delta = bar.delta.delta();
        self.cvd += bar_delta;

        let open_interest = self.latest_oi.unwrap_or(0.0);
        let oi_change = match (self.last_bar_oi, self.latest_oi) {
            (Some(prev), Some(curr)) => curr - prev,
            _ => 0.0,
        };
        let funding_rate = self.latest_funding.unwrap_or(0.0);

        let bid_price = self.latest_bid_price.unwrap_or(0.0);
        let bid_size = self.latest_bid_size.unwrap_or(0.0);
        let ask_price = self.latest_ask_price.unwrap_or(0.0);
        let ask_size = self.latest_ask_size.unwrap_or(0.0);

        let spread_bps = if bid_price > 0.0 && ask_price > 0.0 {
            let mid = (bid_price + ask_price) / 2.0;
            if mid > 0.0 {
                ((ask_price - bid_price) / mid) * 10_000.0
            } else {
                0.0
            }
        } else {
            0.0
        };

        let book_imbalance = if bid_size + ask_size > 0.0 {
            (bid_size - ask_size) / (bid_size + ask_size)
        } else {
            0.0
        };

        let liq_buy_usd = self.liq_buy_usd;
        let liq_sell_usd = self.liq_sell_usd;
        let liq_total_usd = liq_buy_usd + liq_sell_usd;
        let liq_count = self.liq_count;
        let depth_bands = self.depth_bands.clone().unwrap_or_default();

        let extended = ExtendedBar::from_minute_bar(
            bar,
            open_interest,
            oi_change,
            funding_rate,
            bid_price,
            bid_size,
            ask_price,
            ask_size,
            spread_bps,
            book_imbalance,
            self.cvd,
            liq_buy_usd,
            liq_sell_usd,
            liq_total_usd,
            liq_count,
            depth_bands,
        );

        // Advance bar-to-bar OI snapshot
        if let Some(curr) = self.latest_oi {
            self.last_bar_oi = Some(curr);
        }

        // Reset per-minute liquidation aggregates
        self.liq_buy_usd = 0.0;
        self.liq_sell_usd = 0.0;
        self.liq_count = 0;

        extended
    }

    /// Reset CVD (e.g., at day boundary).
    #[allow(dead_code)]
    pub fn reset_cvd(&mut self) {
        self.cvd = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregator::delta::DeltaTracker;

    fn make_test_bar() -> MinuteBar {
        let mut delta = DeltaTracker::new();
        delta.buy_volume = 75.0;
        delta.sell_volume = 25.0;

        MinuteBar {
            instrument_id: "BTCUSDT-PERP.BINANCE".to_string(),
            bar_type: "BTCUSDT-PERP.BINANCE-1-MINUTE-LAST-EXTERNAL".to_string(),
            open: 100_000.0,
            high: 100_100.0,
            low: 99_900.0,
            close: 100_050.0,
            volume: 100.0,
            quote_volume: 10_000_000.0,
            ts_open_ns: 1_706_540_400_000_000_000,
            ts_close_ns: 1_706_540_460_000_000_000,
            ts_init_ns: 1_706_540_460_000_100_000,
            trade_count: 50,
            delta,
            price_precision: 2,
            size_precision: 3,
        }
    }

    #[test]
    fn test_extended_bar_from_minute_bar() {
        let bar = make_test_bar();
        let extended = ExtendedBar::from_minute_bar(
            bar,
            1_000_000.0,
            50_000.0,
            0.0001,
            100_000.0,
            2.0,
            100_010.0,
            1.8,
            1.0,
            0.05,
            500.0,
            10_000.0,
            5_000.0,
            15_000.0,
            3,
            DepthBands::default(),
        );

        assert_eq!(extended.instrument_id, "BTCUSDT-PERP.BINANCE");
        assert_eq!(extended.open_interest, 1_000_000.0);
        assert_eq!(extended.oi_change, 50_000.0);
        assert_eq!(extended.funding_rate, 0.0001);
        assert_eq!(extended.buy_volume, 75.0);
        assert_eq!(extended.sell_volume, 25.0);
        assert_eq!(extended.delta, 50.0);
        assert_eq!(extended.cvd, 500.0);
        assert_eq!(extended.bid_price, 100_000.0);
        assert_eq!(extended.ask_price, 100_010.0);
    }

    #[test]
    fn test_extended_bar_metrics() {
        let bar = make_test_bar();
        let extended = ExtendedBar::from_minute_bar(
            bar,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0,
            DepthBands::default(),
        );

        assert_eq!(extended.range(), 200.0);
        assert_eq!(extended.body(), 50.0);
        assert!(extended.is_bullish());
        assert!((extended.buy_ratio() - 0.75).abs() < 0.001);
    }

    #[test]
    fn test_extended_bar_builder() {
        let mut builder = ExtendedBarBuilder::new();
        builder.update_oi(1_000_000.0);
        builder.update_funding(0.0001);
        builder.update_l1(100_000.0, 2.0, 100_010.0, 1.8);

        let bar1 = make_test_bar();
        let extended1 = builder.build(bar1);

        // First bar CVD = bar delta = 50
        assert_eq!(extended1.cvd, 50.0);
        assert_eq!(extended1.open_interest, 1_000_000.0);
        assert_eq!(extended1.bid_price, 100_000.0);

        let bar2 = make_test_bar();
        let extended2 = builder.build(bar2);

        // Second bar CVD = 50 + 50 = 100
        assert_eq!(extended2.cvd, 100.0);
    }

    #[test]
    fn test_liquidation_aggregates_reset() {
        let mut builder = ExtendedBarBuilder::new();
        builder.update_liquidation(10_000.0, Side::Buy);
        builder.update_liquidation(5_000.0, Side::Sell);

        let bar = make_test_bar();
        let extended = builder.build(bar);
        assert_eq!(extended.liq_buy_usd, 10_000.0);
        assert_eq!(extended.liq_sell_usd, 5_000.0);
        assert_eq!(extended.liq_total_usd, 15_000.0);
        assert_eq!(extended.liq_count, 2);

        // Next bar should reset liquidation aggregates if no new liqs
        let bar2 = make_test_bar();
        let extended2 = builder.build(bar2);
        assert_eq!(extended2.liq_total_usd, 0.0);
        assert_eq!(extended2.liq_count, 0);
    }

    #[test]
    fn test_ts_open_for_tradingview() {
        let bar = make_test_bar();
        let extended = ExtendedBar::from_minute_bar(
            bar,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0,
            DepthBands::default(),
        );

        // ts_open should be 60 seconds before ts_close
        assert_eq!(
            extended.ts_close_ns - extended.ts_open_ns,
            60_000_000_000 // 60 seconds in nanos
        );
    }
}
