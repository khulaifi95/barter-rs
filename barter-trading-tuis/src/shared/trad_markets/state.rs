//! Main state for ES/NQ/BTC correlation analysis

use std::collections::VecDeque;
use std::time::Instant;

use super::aggregator::{BarBuffer, MicroBar, MicroBarAggregator};
use super::calc::{calc_correlation, calc_divergence_zscore, calc_lead_lag, calc_return};

/// Computed signals for display
#[derive(Debug, Clone, Default)]
pub struct CorrelationSignals {
    // Prices
    pub es_price: f64,
    pub nq_price: f64,
    pub btc_price: f64,

    // Returns (over 60s window)
    pub es_return: f64,
    pub nq_return: f64,
    pub btc_return: f64,

    // Spreads
    pub nq_es_spread: f64,    // NQ% - ES%
    pub btc_es_spread: f64,   // BTC% - ES% (main signal)

    // Correlations (None = insufficient data)
    pub es_nq_corr: Option<f64>,
    pub es_btc_corr: Option<f64>,

    // Divergence
    pub divergence_z: Option<f64>,

    // Lead/Lag
    pub lead_lag_bars: i32,   // Positive = ES leads
    pub lead_lag_secs: i32,   // In seconds for display

    // Derived
    pub eq_sync: bool,        // ES/NQ corr > 0.85

    // Data quality
    pub es_bars_count: usize,
    pub nq_bars_count: usize,
    pub btc_bars_count: usize,
    pub es_stale: bool,       // No data for >30s
    pub nq_stale: bool,
}

/// Main state for ES/NQ/BTC correlation analysis
pub struct TradMarketState {
    // Bar aggregators (tick → 5s bar)
    es_aggregator: MicroBarAggregator,
    nq_aggregator: MicroBarAggregator,
    btc_aggregator: MicroBarAggregator,

    // Bar buffers (store last N bars)
    es_bars: BarBuffer,      // 60 bars = 5 minutes
    nq_bars: BarBuffer,
    btc_bars: BarBuffer,

    // Spread history for z-score
    spread_history: VecDeque<f64>,  // 60 samples = 5 minutes

    // Latest prices for display
    es_price: f64,
    nq_price: f64,
    btc_price: f64,

    // Last update times for staleness detection
    es_last_update: Option<Instant>,
    nq_last_update: Option<Instant>,
    btc_last_update: Option<Instant>,

    // Computed signals (updated every bar)
    pub signals: CorrelationSignals,

    // Display throttle
    last_render: Instant,
}

impl TradMarketState {
    pub fn new() -> Self {
        Self {
            es_aggregator: MicroBarAggregator::new(),
            nq_aggregator: MicroBarAggregator::new(),
            btc_aggregator: MicroBarAggregator::new(),
            es_bars: BarBuffer::new(60),
            nq_bars: BarBuffer::new(60),
            btc_bars: BarBuffer::new(60),
            spread_history: VecDeque::with_capacity(60),
            es_price: 0.0,
            nq_price: 0.0,
            btc_price: 0.0,
            es_last_update: None,
            nq_last_update: None,
            btc_last_update: None,
            signals: CorrelationSignals::default(),
            last_render: Instant::now(),
        }
    }

    /// Update with ES tick from ibkr-bridge
    pub fn update_es_tick(&mut self, price: f64, size: f64, ts: i64) {
        if price <= 0.0 {
            return;
        }
        self.es_price = price;
        self.es_last_update = Some(Instant::now());
        if let Some(bar) = self.es_aggregator.update(price, size, ts) {
            self.es_bars.push(bar);
            self.recompute_signals();
        }
    }

    /// Update with NQ tick from ibkr-bridge
    pub fn update_nq_tick(&mut self, price: f64, size: f64, ts: i64) {
        if price <= 0.0 {
            return;
        }
        self.nq_price = price;
        self.nq_last_update = Some(Instant::now());
        if let Some(bar) = self.nq_aggregator.update(price, size, ts) {
            self.nq_bars.push(bar);
        }
    }

    /// Update with BTC trade from existing crypto feed
    pub fn update_btc_trade(&mut self, price: f64, size: f64, ts: i64) {
        if price <= 0.0 {
            return;
        }
        self.btc_price = price;
        self.btc_last_update = Some(Instant::now());
        if let Some(bar) = self.btc_aggregator.update(price, size, ts) {
            self.btc_bars.push(bar);
        }
    }

    /// Recompute all signals (called when ES bar completes)
    fn recompute_signals(&mut self) {
        let window = 12;  // 12 bars = 60 seconds

        let es_returns = self.es_bars.returns(window);
        let nq_returns = self.nq_bars.returns(window);
        let btc_returns = self.btc_bars.returns(window);

        // % returns over window
        let es_bars: Vec<_> = self.es_bars.last_n(window);
        let nq_bars: Vec<_> = self.nq_bars.last_n(window);
        let btc_bars: Vec<_> = self.btc_bars.last_n(window);

        let es_return = calc_return(&es_bars);
        let nq_return = calc_return(&nq_bars);
        let btc_return = calc_return(&btc_bars);

        // Spreads
        let nq_es_spread = nq_return - es_return;
        let btc_es_spread = btc_return - es_return;

        // Update spread history for z-score
        if self.spread_history.len() >= 60 {
            self.spread_history.pop_front();
        }
        self.spread_history.push_back(btc_es_spread);

        // Correlations
        let es_nq_corr = calc_correlation(&es_returns, &nq_returns);
        let es_btc_corr = calc_correlation(&es_returns, &btc_returns);

        // Divergence z-score
        let divergence_z = calc_divergence_zscore(btc_es_spread, &self.spread_history);

        // Lead/lag (max 6 bars = 30 seconds)
        let (lead_lag_bars, _) = if es_returns.len() >= 5 && btc_returns.len() >= 5 {
            calc_lead_lag(&es_returns, &btc_returns, 6)
        } else {
            (0, 0.0)
        };
        let lead_lag_secs = lead_lag_bars * 5;  // Convert to seconds

        // EQ sync check
        let eq_sync = es_nq_corr.map(|c| c > 0.85).unwrap_or(false);

        // Staleness check
        let now = Instant::now();
        let stale_threshold = std::time::Duration::from_secs(30);
        let es_stale = self.es_last_update
            .map(|t| now.duration_since(t) > stale_threshold)
            .unwrap_or(true);
        let nq_stale = self.nq_last_update
            .map(|t| now.duration_since(t) > stale_threshold)
            .unwrap_or(true);

        self.signals = CorrelationSignals {
            es_price: self.es_price,
            nq_price: self.nq_price,
            btc_price: self.btc_price,
            es_return,
            nq_return,
            btc_return,
            nq_es_spread,
            btc_es_spread,
            es_nq_corr,
            es_btc_corr,
            divergence_z,
            lead_lag_bars,
            lead_lag_secs,
            eq_sync,
            es_bars_count: self.es_bars.len(),
            nq_bars_count: self.nq_bars.len(),
            btc_bars_count: self.btc_bars.len(),
            es_stale,
            nq_stale,
        };
    }

    /// Check if should render (1 second throttle)
    pub fn should_render(&mut self) -> bool {
        if self.last_render.elapsed() >= std::time::Duration::from_secs(1) {
            self.last_render = Instant::now();
            true
        } else {
            false
        }
    }

    /// Get current signals snapshot with real-time prices
    pub fn get_signals(&self) -> CorrelationSignals {
        // Return signals with real-time prices (not the stale bar-completion prices)
        CorrelationSignals {
            es_price: self.es_price,
            nq_price: self.nq_price,
            btc_price: self.btc_price,
            ..self.signals.clone()
        }
    }

    /// Check if we have any ES/NQ data
    pub fn has_trad_data(&self) -> bool {
        self.es_price > 0.0 || self.nq_price > 0.0
    }

    /// Get current ES price (for display even without complete bars)
    pub fn current_es_price(&self) -> f64 {
        if self.es_price > 0.0 {
            self.es_price
        } else {
            self.es_aggregator.current_price()
        }
    }

    /// Get current NQ price
    pub fn current_nq_price(&self) -> f64 {
        if self.nq_price > 0.0 {
            self.nq_price
        } else {
            self.nq_aggregator.current_price()
        }
    }
}

impl Default for TradMarketState {
    fn default() -> Self {
        Self::new()
    }
}
