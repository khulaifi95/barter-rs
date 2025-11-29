//! Shared aggregation layer for all TUIs.
//!
//! Maintains rolling windows for trades, liquidations, OI, CVD, and orderbook data
//! so each TUI can render consistent metrics without duplicating calculations.

use crate::shared::types::{
    CvdData, LiquidationData, MarketEventMessage, OpenInterestData, OrderBookL1Data, Side,
    TradeData,
};
use chrono::{DateTime, Duration as ChronoDuration, Datelike, Timelike, Utc};
use rust_decimal::prelude::ToPrimitive;
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;

// ============================================================================
// VWAP & ATR Configuration
// ============================================================================
// ATR-14 uses EMA (not SMA) for faster response to crypto volatility changes.
// Rationale: Crypto volatility regimes shift quickly (minutes not hours).
// SMA's equal weighting of 70-minute-old data causes unacceptable lag during rapid moves.
// Future consideration: If EMA proves too noisy, revisit SMA or use shorter period (ATR-10).

/// 1-minute candle from Binance kline stream (authoritative source)
#[derive(Clone, Debug)]
pub struct Candle1m {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub start_time: DateTime<Utc>,
    pub is_complete: bool,
}

/// 5-minute candle for ATR calculation (aggregated from 1m candles)
#[derive(Clone, Debug)]
struct Candle5m {
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: f64,
    start_time: DateTime<Utc>,
}

/// VWAP accumulator state
#[derive(Clone, Debug, Default)]
struct VwapState {
    sum_pv: f64,      // Σ(price × volume)
    sum_v: f64,       // Σ(volume)
    last_reset: Option<DateTime<Utc>>,
}

impl VwapState {
    fn add_trade(&mut self, price: f64, volume: f64) {
        self.sum_pv += price * volume;
        self.sum_v += volume;
    }

    fn vwap(&self) -> Option<f64> {
        if self.sum_v > 0.0 {
            Some(self.sum_pv / self.sum_v)
        } else {
            None
        }
    }

    fn reset(&mut self, time: DateTime<Utc>) {
        self.sum_pv = 0.0;
        self.sum_v = 0.0;
        self.last_reset = Some(time);
    }
}

/// ATR-14 EMA state
#[derive(Clone, Debug, Default)]
struct AtrState {
    atr: f64,
    prev_close: Option<f64>,
    initialized: bool,
    tr_count: usize,
}

impl AtrState {
    /// EMA multiplier for 14-period: 2 / (14 + 1) = 0.1333
    const EMA_MULTIPLIER: f64 = 2.0 / 15.0;

    fn update(&mut self, candle: &Candle5m) {
        // Sanity guard: skip obviously bad candles (zero/negative, inverted, or >20% range)
        let mut high = candle.high;
        let mut low = candle.low;
        if high < low {
            std::mem::swap(&mut high, &mut low);
        }
        if high <= 0.0 || low <= 0.0 {
            return;
        }

        // Anchor for % checks uses the largest of close/open/high to reduce false positives
        let anchor = candle
            .close
            .max(candle.open)
            .max(high)
            .max(1.0);

        let tr = if let Some(prev_close) = self.prev_close {
            // True Range = max(H-L, |H-PrevClose|, |L-PrevClose|)
            (high - low)
                .max((high - prev_close).abs())
                .max((low - prev_close).abs())
        } else {
            high - low
        };

        // Drop pathological ranges that would blow up ATR (e.g., outlier candles or bad data)
        let tr_pct = tr / anchor;
        if tr_pct > 0.20 {
            return;
        }

        self.prev_close = Some(candle.close);
        self.tr_count += 1;

        if !self.initialized {
            // Bootstrap: use simple average until we have 14 periods
            if self.tr_count == 1 {
                self.atr = tr;
            } else {
                // Running average for bootstrap
                self.atr = ((self.atr * (self.tr_count - 1) as f64) + tr) / self.tr_count as f64;
            }
            if self.tr_count >= 14 {
                self.initialized = true;
            }
        } else {
            // EMA: ATR = (TR × multiplier) + (ATR_prev × (1 - multiplier))
            self.atr = (tr * Self::EMA_MULTIPLIER) + (self.atr * (1.0 - Self::EMA_MULTIPLIER));
        }
    }

    fn value(&self) -> f64 {
        self.atr
    }
}

/// Trading session identifier
#[derive(Clone, Debug, Copy, PartialEq, Eq)]
pub enum TradingSession {
    Asia,    // 00:00 - 08:00 UTC
    Europe,  // 08:00 - 14:00 UTC
    US,      // 14:00 - 22:00 UTC
    USLate,  // 22:00 - 00:00 UTC
}

impl TradingSession {
    fn from_utc_hour(hour: u32) -> Self {
        match hour {
            0..=7 => TradingSession::Asia,
            8..=13 => TradingSession::Europe,
            14..=21 => TradingSession::US,
            _ => TradingSession::USLate,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            TradingSession::Asia => "ASIA",
            TradingSession::Europe => "EU",
            TradingSession::US => "US",
            TradingSession::USLate => "US-LATE",
        }
    }

    fn start_hour(&self) -> u32 {
        match self {
            TradingSession::Asia => 0,
            TradingSession::Europe => 8,
            TradingSession::US => 14,
            TradingSession::USLate => 22,
        }
    }
}

/// Volatility trend indicator
#[derive(Clone, Debug, Copy, PartialEq, Eq, Default)]
pub enum VolTrend {
    Expanding,   // Vol increasing
    Contracting, // Vol decreasing
    #[default]
    Stable,      // Vol relatively unchanged
}

impl VolTrend {
    pub fn label(&self) -> &'static str {
        match self {
            VolTrend::Expanding => "EXPANDING",
            VolTrend::Contracting => "CONTRACTING",
            VolTrend::Stable => "STABLE",
        }
    }

    pub fn arrow(&self) -> &'static str {
        match self {
            VolTrend::Expanding => "▲",
            VolTrend::Contracting => "▼",
            VolTrend::Stable => "→",
        }
    }
}

// ============================================================================
// Backfill: Fetch historical 5m candles on startup
// ============================================================================

/// Binance kline response format
#[derive(Debug, Deserialize)]
struct BinanceKline(
    i64,    // 0: Open time
    String, // 1: Open
    String, // 2: High
    String, // 3: Low
    String, // 4: Close
    String, // 5: Volume
    i64,    // 6: Close time
    String, // 7: Quote asset volume
    i64,    // 8: Number of trades
    String, // 9: Taker buy base asset volume
    String, // 10: Taker buy quote asset volume
    String, // 11: Ignore
);

/// Backfill result for a ticker
#[derive(Debug, Default)]
pub struct BackfillResult {
    pub candles_loaded: usize,
    pub tv_vwap: Option<f64>,
    pub atr_14: Option<f64>,
}

/// Fetch 5m candles from Binance and return parsed candles
pub async fn fetch_binance_5m_candles(symbol: &str) -> Result<Vec<Candle5m>, String> {
    // Calculate start time: 00:00 UTC today
    let now = Utc::now();
    let start_of_day = now
        .with_hour(0)
        .and_then(|t| t.with_minute(0))
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(now);
    let start_ms = start_of_day.timestamp_millis();

    // Also fetch some candles from yesterday for ATR warm-up (14 candles = 70 min)
    let warmup_start_ms = start_ms - (2 * 60 * 60 * 1000); // 2 hours before midnight

    let url = format!(
        "https://fapi.binance.com/fapi/v1/klines?symbol={}&interval=5m&startTime={}&limit=500",
        symbol, warmup_start_ms
    );

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("HTTP error: {}", response.status()));
    }

    let klines: Vec<BinanceKline> = response
        .json()
        .await
        .map_err(|e| format!("JSON parse failed: {}", e))?;

    let candles: Vec<Candle5m> = klines
        .into_iter()
        .filter_map(|k| {
            let open_time_ms = k.0;
            let start_time = DateTime::from_timestamp_millis(open_time_ms)?;
            Some(Candle5m {
                open: k.1.parse().ok()?,
                high: k.2.parse().ok()?,
                low: k.3.parse().ok()?,
                close: k.4.parse().ok()?,
                volume: k.5.parse().ok()?,
                start_time,
            })
        })
        .collect();

    Ok(candles)
}

/// Fetch 1m candles from Binance futures API (authoritative source for tvVWAP/ATR/RV)
pub async fn fetch_binance_1m_candles(symbol: &str) -> Result<Vec<Candle1m>, String> {
    // Calculate start time: 00:00 UTC today
    let now = Utc::now();
    let start_of_day = now
        .with_hour(0)
        .and_then(|t| t.with_minute(0))
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(now);
    let start_ms = start_of_day.timestamp_millis();

    // Fetch from start of day (for tvVWAP) plus some warmup for ATR
    // 1m candles: fetch last 300 (5 hours) for ATR warmup and RV calculation
    let warmup_start_ms = start_ms - (60 * 60 * 1000); // 1 hour before midnight

    let url = format!(
        "https://fapi.binance.com/fapi/v1/klines?symbol={}&interval=1m&startTime={}&limit=1000",
        symbol, warmup_start_ms
    );

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("HTTP error: {}", response.status()));
    }

    let klines: Vec<BinanceKline> = response
        .json()
        .await
        .map_err(|e| format!("JSON parse failed: {}", e))?;

    let candles: Vec<Candle1m> = klines
        .into_iter()
        .filter_map(|k| {
            let open_time_ms = k.0;
            let close_time_ms = k.6;
            let start_time = DateTime::from_timestamp_millis(open_time_ms)?;
            // Mark candle as complete if close time has passed
            let is_complete = close_time_ms < now.timestamp_millis();
            Some(Candle1m {
                open: k.1.parse().ok()?,
                high: k.2.parse().ok()?,
                low: k.3.parse().ok()?,
                close: k.4.parse().ok()?,
                volume: k.5.parse().ok()?,
                start_time,
                is_complete,
            })
        })
        .collect();

    Ok(candles)
}

/// Map ticker to Binance futures symbol
pub fn ticker_to_binance_symbol(ticker: &str) -> &'static str {
    match ticker {
        "BTC" => "BTCUSDT",
        "ETH" => "ETHUSDT",
        "SOL" => "SOLUSDT",
        _ => "BTCUSDT",
    }
}

// Default constants (overridable via environment variables)
const TRADE_RETENTION_SECS: i64 = 15 * 60;
const LIQ_RETENTION_SECS: i64 = 10 * 60;
const CVD_RETENTION_SECS: i64 = 5 * 60;
const PRICE_RETENTION_SECS: i64 = 15 * 60;

/// Get whale detection threshold from WHALE_THRESHOLD env var (default: $500,000)
fn whale_threshold() -> f64 {
    static WHALE_THRESHOLD: OnceLock<f64> = OnceLock::new();
    *WHALE_THRESHOLD.get_or_init(|| {
        std::env::var("WHALE_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(500_000.0)
    })
}

/// Minimum whale threshold floor (env: WHALE_FLOOR, default: $100,000)
fn whale_floor() -> f64 {
    static WHALE_FLOOR: OnceLock<f64> = OnceLock::new();
    *WHALE_FLOOR.get_or_init(|| {
        std::env::var("WHALE_FLOOR")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100_000.0)
    })
}

/// Multiplier applied to avg trade size to derive adaptive whale threshold (env: WHALE_MULTIPLIER, default: 10x)
fn whale_multiplier() -> f64 {
    static WHALE_MULT: OnceLock<f64> = OnceLock::new();
    *WHALE_MULT.get_or_init(|| {
        std::env::var("WHALE_MULTIPLIER")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(7.0)
    })
}

/// Per-ticker mega whale thresholds (env: MEGA_WHALE_BTC/ETH/SOL, fallback MEGA_WHALE_THRESHOLD default $500k)
fn mega_whale_threshold(ticker: &str) -> f64 {
    let key = format!("MEGA_WHALE_{}", ticker);
    std::env::var(&key)
        .ok()
        .and_then(|v| v.parse().ok())
        .or_else(|| {
            std::env::var("MEGA_WHALE_THRESHOLD")
                .ok()
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(500_000.0)
}

/// Get max whales buffer size from MAX_WHALES env var (default: 500)
fn max_whales() -> usize {
    static MAX_WHALES: OnceLock<usize> = OnceLock::new();
    *MAX_WHALES.get_or_init(|| {
        std::env::var("MAX_WHALES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(500)
    })
}

/// Get liquidation danger threshold from LIQ_DANGER_THRESHOLD env var (default: $1,000,000)
fn liq_danger_threshold() -> f64 {
    static LIQ_DANGER: OnceLock<f64> = OnceLock::new();
    *LIQ_DANGER.get_or_init(|| {
        std::env::var("LIQ_DANGER_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1_000_000.0)
    })
}

#[derive(Debug, Default, Clone)]
struct WhaleCounters {
    spot: u64,
    perp: u64,
    other: u64,
    total: u64,
}

/// Snapshot returned to UI layers.
#[derive(Clone, Debug, Default)]
pub struct AggregatedSnapshot {
    pub tickers: HashMap<String, TickerSnapshot>,
    pub correlation: [[f64; 3]; 3],
    pub exchange_health: HashMap<String, bool>,
}

/// Per-ticker snapshot with pre-computed metrics.
#[derive(Clone, Debug, Default)]
pub struct TickerSnapshot {
    pub ticker: String,
    pub latest_price: Option<f64>,
    pub latest_spread_pct: Option<f64>,
    pub spot_mid: Option<f64>,
    pub perp_mid: Option<f64>,
    pub basis: Option<BasisStats>,
    // Multi-timeframe orderflow (P0: 5s/15s/30s for scalper, 1m/5m for swing)
    pub orderflow_5s: OrderflowStats,
    pub orderflow_15s: OrderflowStats,
    pub orderflow_30s: OrderflowStats,
    pub orderflow_1m: OrderflowStats,
    pub orderflow_5m: OrderflowStats,
    pub exchange_dominance: HashMap<String, f64>,
    pub vwap_30s: Option<f64>,
    pub vwap_1m: Option<f64>,
    pub vwap_5m: Option<f64>,
    pub best_bid: Option<(f64, f64)>, // (price, size)
    pub best_ask: Option<(f64, f64)>, // (price, size)
    pub exchange_prices: HashMap<String, f64>,
    pub whales: Vec<WhaleRecord>,
    pub liquidations: Vec<LiquidationCluster>,
    pub liq_rate_per_min: f64,
    pub liq_bucket: f64,
    pub cascade_risk: f64,
    pub next_cascade_level: Option<CascadeLevel>,
    pub protection_level: Option<CascadeLevel>,
    pub cvd: CvdSummary,
    // Multi-timeframe CVD (P0: 5s/15s/30s for scalper, 1m/5m/15m for swing)
    pub cvd_5s: f64,
    pub cvd_15s: f64,
    pub cvd_30s: f64,
    pub cvd_1m_total: f64,
    pub cvd_5m_total: f64,
    pub cvd_15m_total: f64,
    pub cvd_per_exchange_5m: HashMap<String, f64>,
    pub cvd_per_exchange_15m: HashMap<String, f64>,
    // CVD velocity (change per minute)
    pub cvd_velocity_1m: f64,
    pub cvd_velocity_5m: f64,
    pub cvd_velocity_15m: f64,
    pub trades_5m: usize,
    pub vol_5m: f64,
    pub avg_trade_usd_5m: f64,
    pub per_exchange_30s: HashMap<String, PerExchangeShortStats>,
    // OI with velocity (P1) - enhanced with per-exchange
    pub oi_total: f64,
    pub oi_delta_5m: f64,
    pub oi_delta_15m: f64,
    pub oi_velocity: f64,
    pub oi_per_exchange: HashMap<String, f64>,
    pub oi_delta_per_exchange_5m: HashMap<String, f64>,
    pub oi_delta_per_exchange_15m: HashMap<String, f64>,
    // Exchange health (seconds since last data)
    pub exchange_health: HashMap<String, f64>,
    pub tick_direction: TickDirection,
    pub tick_direction_5m: TickDirection,
    pub tick_direction_30s: TickDirection,
    pub trade_speed: f64,
    pub avg_trade_usd: f64,
    pub cvd_divergence: DivergenceSignal,
    /// Short-term (15s) price/CVD divergence for scalper
    pub cvd_divergence_15s: DivergenceSignal,
    // P1: Flow signal detection
    pub flow_signal: FlowSignal,
    // P1: Basis momentum
    pub basis_momentum: Option<BasisMomentum>,
    // VWAP metrics (tick-based)
    pub vwap_daily: Option<f64>,           // Since 00:00 UTC (tick-based)
    pub vwap_session: Option<f64>,         // Since session start (tick-based)
    pub current_session: Option<TradingSession>,
    pub vwap_daily_deviation: Option<f64>, // (price - vwap) / vwap * 100
    // tvVWAP (TradingView style - HLC3 on 5m candles since 00:00 UTC)
    pub tv_vwap: Option<f64>,
    pub tv_vwap_deviation: Option<f64>,    // (price - tvVWAP) / tvVWAP * 100
    pub candles_5m_len: usize,             // For guarding tvVWAP / RV display
    // ATR-14 (5m candles, EMA)
    pub atr_14: Option<f64>,               // ATR in price units
    pub atr_14_pct: Option<f64>,           // ATR as % of price
    // Realized Volatility (from 5m candles)
    pub realized_vol_30m: Option<f64>,     // RV % over last 30 min (6 candles)
    pub realized_vol_1h: Option<f64>,      // RV % over last hour (12 candles)
    pub realized_vol_trend: VolTrend,      // EXPANDING, CONTRACTING, STABLE
    // L2 Orderbook imbalance (per exchange and aggregated)
    pub per_exchange_book_imbalance: HashMap<String, f64>, // Book imbalance by exchange (0-100%, 50% = balanced)
    pub aggregated_book_imbalance: f64,                    // Combined book imbalance across all exchanges
    pub book_flip_count: usize,                             // Number of imbalance flips in last 30s
    pub book_freshness: HashMap<String, f64>,              // Seconds since last book update per exchange
}

#[derive(Clone, Debug, Default)]
pub struct OrderflowStats {
    pub buy_usd: f64,
    pub sell_usd: f64,
    pub imbalance_pct: f64,
    pub net_flow_per_min: f64,
    pub trades_per_sec: f64,
}

#[derive(Clone, Debug, Default)]
pub struct BasisStats {
    pub basis_usd: f64,
    pub basis_pct: f64,
    pub state: BasisState,
    pub steep: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum BasisState {
    #[default]
    Unknown,
    Contango,
    Backwardation,
}

#[derive(Clone, Debug, Default)]
pub struct LiquidationCluster {
    pub price_level: f64,
    pub total_usd: f64,
    pub long_count: usize,
    pub short_count: usize,
}

#[derive(Clone, Debug)]
pub struct CascadeLevel {
    pub price: f64,
    pub total_usd: f64,
    pub side: Side,
}

#[derive(Clone, Debug)]
pub struct WhaleRecord {
    pub time: DateTime<Utc>,
    pub side: Side,
    pub volume_usd: f64,
    pub price: f64,
    pub exchange: String,
    pub market_kind: String,
}

#[derive(Clone, Debug, Default)]
pub struct CvdSummary {
    pub total_quote: f64,
    pub velocity_quote: f64,
}

#[derive(Clone, Debug, Default)]
pub struct TickDirection {
    pub upticks: u64,
    pub downticks: u64,
    pub uptick_pct: f64,
}

#[derive(Clone, Debug, Copy, PartialEq, Eq)]
pub enum DivergenceSignal {
    Bullish,
    Bearish,
    Aligned,
    Neutral,
    Unknown,
}

impl Default for DivergenceSignal {
    fn default() -> Self {
        DivergenceSignal::Unknown
    }
}

/// P1: Flow signal for CVD momentum panel
#[derive(Clone, Debug, Copy, PartialEq, Eq)]
pub enum FlowSignal {
    /// Price flat, CVD rising - accumulation
    Accumulation,
    /// Price up, CVD falling - distribution (smart money selling into strength)
    Distribution,
    /// Price moving but CVD flat - exhaustion (momentum fading)
    Exhaustion,
    /// Price and CVD aligned - confirmation (trend supported by flow)
    Confirmation,
    /// No clear signal
    Neutral,
}

impl Default for FlowSignal {
    fn default() -> Self {
        FlowSignal::Neutral
    }
}

/// P1: Basis momentum for tracking basis velocity
#[derive(Clone, Debug, Default)]
pub struct BasisMomentum {
    pub delta_1m: f64,
    pub delta_5m: f64,
    pub trend: BasisTrend,
    pub signal: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct PerExchangeShortStats {
    pub cvd_30s: f64,
    pub total_30s: f64,
    pub trades_30s: usize,
}

/// P1: Basis trend direction
#[derive(Clone, Debug, Copy, PartialEq, Eq)]
pub enum BasisTrend {
    Widening,
    Narrowing,
    Stable,
}

impl Default for BasisTrend {
    fn default() -> Self {
        BasisTrend::Stable
    }
}

#[derive(Clone, Debug, Default)]
pub struct Aggregator {
    tickers: HashMap<String, TickerState>,
    exchange_last_seen: HashMap<String, DateTime<Utc>>,
    // Debug-only counters for whales per exchange (sliding window)
    whale_counts: HashMap<String, WhaleCounters>,
    last_whale_log: DateTime<Utc>,
}

impl Aggregator {
    pub fn new() -> Self {
        Self {
            tickers: HashMap::new(),
            exchange_last_seen: HashMap::new(),
            whale_counts: HashMap::new(),
            last_whale_log: Utc::now(),
        }
    }

    /// Backfill tvVWAP and ATR for all tickers on startup
    /// Fetches historical 5m candles from Binance REST API
    pub async fn backfill_all(&mut self, tickers: &[&str]) -> HashMap<String, BackfillResult> {
        let mut results = HashMap::new();

        for ticker in tickers {
            let symbol = ticker_to_binance_symbol(ticker);

            // Ensure ticker state exists
            let state = self
                .tickers
                .entry(ticker.to_string())
                .or_insert_with(|| TickerState::new(ticker.to_string()));

            match fetch_binance_5m_candles(symbol).await {
                Ok(candles) => {
                    let result = state.backfill_from_candles(candles);
                    results.insert(ticker.to_string(), result);
                }
                Err(e) => {
                    eprintln!("[backfill] {} failed: {}", ticker, e);
                    results.insert(ticker.to_string(), BackfillResult::default());
                }
            }
        }

        results
    }

    /// Backfill a single ticker
    pub async fn backfill_ticker(&mut self, ticker: &str) -> BackfillResult {
        let symbol = ticker_to_binance_symbol(ticker);

        // Ensure ticker state exists
        let state = self
            .tickers
            .entry(ticker.to_string())
            .or_insert_with(|| TickerState::new(ticker.to_string()));

        match fetch_binance_5m_candles(symbol).await {
            Ok(candles) => state.backfill_from_candles(candles),
            Err(e) => {
                eprintln!("[backfill] {} failed: {}", ticker, e);
                BackfillResult::default()
            }
        }
    }

    /// Backfill tvVWAP, ATR, and RV from authoritative Binance 1m klines
    /// This is the preferred backfill method - use this for accurate metrics
    pub async fn backfill_1m_klines(&mut self, tickers: &[&str]) -> HashMap<String, BackfillResult> {
        let mut results = HashMap::new();

        for ticker in tickers {
            let symbol = ticker_to_binance_symbol(ticker);

            // Ensure ticker state exists
            let state = self
                .tickers
                .entry(ticker.to_string())
                .or_insert_with(|| TickerState::new(ticker.to_string()));

            match fetch_binance_1m_candles(symbol).await {
                Ok(candles) => {
                    let result = state.backfill_from_1m_candles(candles);
                    results.insert(ticker.to_string(), result);
                }
                Err(e) => {
                    eprintln!("[backfill-1m] {} failed: {}", ticker, e);
                    results.insert(ticker.to_string(), BackfillResult::default());
                }
            }
        }

        results
    }

    /// Push a 1m candle update from Binance kline WebSocket
    pub fn push_1m_candle(&mut self, ticker: &str, candle: Candle1m) {
        let state = self
            .tickers
            .entry(ticker.to_string())
            .or_insert_with(|| TickerState::new(ticker.to_string()));

        state.push_1m_candle(candle);
    }

    pub fn process_event(&mut self, event: MarketEventMessage) {
        let ticker = event.instrument.base.to_uppercase();
        let kind = event.instrument.kind.to_lowercase();

        // Hardened spot/perp classifier
        let mut is_spot = kind.contains("spot");
        let mut is_perp = kind.contains("perp") || kind.contains("perpetual") || kind.contains("swap") || kind.contains("futures");

        // Fallback: use exchange name if kind is ambiguous
        if !is_spot && !is_perp {
            let exchange_lower = event.exchange.to_lowercase();
            if exchange_lower.contains("spot") {
                is_spot = true;
            }
            if exchange_lower.contains("perp") || exchange_lower.contains("futures") {
                is_perp = true;
            }
        }

        let state = self
            .tickers
            .entry(ticker.clone())
            .or_insert_with(|| TickerState::new(ticker.clone()));

        match event.kind.as_str() {
            "trade" => {
                if let Ok(trade) = serde_json::from_value::<TradeData>(event.data) {
                    // Debug: log Binance trades to verify is_perp classification
                    if event.exchange.contains("Binance") {
                        use std::io::Write;
                        if let Ok(mut file) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open("binance_trade_classification.log")
                        {
                            let _ = writeln!(file, "[{}] {} {} is_perp={} is_spot={} kind={}",
                                chrono::Utc::now(), event.exchange, ticker, is_perp, is_spot, kind);
                        }
                    }
                    state.push_trade(
                        trade,
                        &event.exchange,
                        event.time_exchange,
                        is_spot,
                        is_perp,
                    );
                }
            }
            "liquidation" => {
                if let Ok(liq) = serde_json::from_value::<LiquidationData>(event.data) {
                    let time = liq.time;
                    state.push_liquidation(liq, &event.exchange, time);
                }
            }
            "cumulative_volume_delta" => {
                if let Ok(cvd) = serde_json::from_value::<CvdData>(event.data) {
                    state.push_cvd(&event.exchange, cvd, event.time_exchange);
                }
            }
            "open_interest" => {
                if let Ok(oi) = serde_json::from_value::<OpenInterestData>(event.data) {
                    state.push_oi(&event.exchange, oi.contracts);
                }
            }
            "order_book_l1" => {
                if let Ok(ob) = serde_json::from_value::<OrderBookL1Data>(event.data) {
                    state.push_orderbook(ob, is_spot, is_perp, event.time_exchange);
                }
            }
            "order_book_l2" => {
                use crate::shared::types::OrderBookL2Data;
                if let Ok(ob_event) = serde_json::from_value::<OrderBookL2Data>(event.data) {
                    let book = ob_event.book();
                    state.push_orderbook_l2(book, &event.exchange, event.time_exchange);
                }
            }
            _ => {}
        }

        // Track exchange heartbeat
        self.exchange_last_seen
            .insert(event.exchange.clone(), Utc::now());

        // Debug: track whales per exchange when a whale was added
        if let Some(ticker_state) = self.tickers.get(&ticker) {
            if let Some(kind) = ticker_state.last_whale(&event.exchange) {
                let counters = self
                    .whale_counts
                    .entry(event.exchange.clone())
                    .or_insert_with(WhaleCounters::default);
                counters.total += 1;
                match kind {
                    "SPOT" => counters.spot += 1,
                    "PERP" => counters.perp += 1,
                    _ => counters.other += 1,
                }
            }
        }

        // Periodically log whale distribution (debug; can be removed later)
        let now = Utc::now();
        if (now - self.last_whale_log).num_seconds() >= 30 {
            let mut counts: Vec<_> = self.whale_counts.iter().collect();
            counts.sort_by(|a, b| b.1.total.cmp(&a.1.total));
            let summary: Vec<String> = counts
                .iter()
                .map(|(ex, c)| {
                    format!(
                        "{}:{} (spot {} / perp {} / other {})",
                        ex, c.total, c.spot, c.perp, c.other
                    )
                })
                .collect();
            // Log to file instead of stdout to avoid TUI interference
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("whale_debug.log")
            {
                use std::io::Write;
                let _ = writeln!(file, "[whale-debug] last 30s whales: {}", summary.join(", "));
            }
            self.whale_counts.clear();
            self.last_whale_log = now;
        }
    }

    pub fn snapshot(&self) -> AggregatedSnapshot {
        // Pre-compute exchange freshness (seconds since last event)
        let exchange_ages = self.exchange_age_seconds();

        let mut tickers_out = HashMap::new();
        for (ticker, state) in &self.tickers {
            let mut snap = state.to_snapshot();
            // Inject exchange freshness into each ticker snapshot
            snap.exchange_health = exchange_ages.clone();
            tickers_out.insert(ticker.clone(), snap);
        }

        let correlation = self.compute_correlation();

        let exchange_health = self.compute_exchange_health();

        AggregatedSnapshot {
            tickers: tickers_out,
            correlation,
            exchange_health,
        }
    }

    fn compute_correlation(&self) -> [[f64; 3]; 3] {
        let names = ["BTC", "ETH", "SOL"];
        let mut matrix = [[0.0; 3]; 3];

        for (i, a) in names.iter().enumerate() {
            for (j, b) in names.iter().enumerate() {
                if i == j {
                    matrix[i][j] = 1.0;
                } else {
                    matrix[i][j] = self
                        .tickers
                        .get(*a)
                        .and_then(|t| self.tickers.get(*b).map(|s| (t, s)))
                        .map(|(t1, t2)| correlate(&t1.price_history, &t2.price_history))
                        .unwrap_or(0.0);
                }
            }
        }

        matrix
    }

    fn compute_exchange_health(&self) -> HashMap<String, bool> {
        let now = Utc::now();
        let mut health = HashMap::new();
        for (ex, last) in &self.exchange_last_seen {
            let ok = (now - *last).num_seconds() <= 30;
            health.insert(ex.clone(), ok);
        }
        health
    }

    /// Get seconds since last event per exchange (for scalper freshness display)
    fn exchange_age_seconds(&self) -> HashMap<String, f64> {
        let now = Utc::now();
        let mut ages = HashMap::new();
        for (ex, last) in &self.exchange_last_seen {
            let age_ms = (now - *last).num_milliseconds() as f64;
            ages.insert(ex.clone(), age_ms / 1000.0);
        }
        ages
    }
}

#[derive(Clone, Debug)]
struct TradeRecord {
    time: DateTime<Utc>,
    side: Side,
    price: f64,
    amount: f64,
    exchange: String,
    usd: f64,
    is_spot: bool,
    is_perp: bool,
}

#[derive(Clone, Debug)]
struct LiquidationRecord {
    time: DateTime<Utc>,
    side: Side,
    price: f64,
    value: f64,
    exchange: String,
}

#[derive(Clone, Debug)]
struct CvdRecord {
    time: DateTime<Utc>,
    total_quote: f64,
}

/// OI retention for velocity calculation (10 minutes)
const OI_RETENTION_SECS: i64 = 10 * 60;
/// Basis retention for momentum tracking (10 minutes)
const BASIS_RETENTION_SECS: i64 = 10 * 60;

#[derive(Clone, Debug)]
struct OiRecord {
    time: DateTime<Utc>,
    total: f64,
}

#[derive(Clone, Debug)]
struct BasisRecord {
    time: DateTime<Utc>,
    basis_usd: f64,
    basis_pct: f64,
}

#[derive(Clone, Debug)]
struct TickerState {
    ticker: String,
    trades: VecDeque<TradeRecord>,
    // Per-exchange whale buffers to avoid a single venue dominating
    whales_by_exchange: HashMap<String, VecDeque<WhaleRecord>>,
    liquidations: VecDeque<LiquidationRecord>,
    // Rolling CVD deltas per exchange (time, delta_quote)
    cvd_deltas_by_exchange: HashMap<String, VecDeque<(DateTime<Utc>, f64)>>,
    cvd_history: VecDeque<CvdRecord>, // snapshots of total CVD (used for divergence)
    // P1: OI time-series for velocity calculation
    oi_by_exchange: HashMap<String, f64>,
    oi_history: VecDeque<OiRecord>,
    oi_history_by_exchange: HashMap<String, VecDeque<OiRecord>>,
    // P1: Basis history for momentum tracking
    basis_history: VecDeque<BasisRecord>,
    spot_mid: Option<f64>,
    perp_mid: Option<f64>,
    spread_pct: Option<f64>,
    best_bid: Option<(f64, f64)>,
    best_ask: Option<(f64, f64)>,
    price_history: VecDeque<(DateTime<Utc>, f64)>,
    exchange_volume: VecDeque<(DateTime<Utc>, String, f64)>,
    last_trade_by_exchange: HashMap<String, f64>,
    last_whale_exchange: Option<String>,
    last_whale_kind: Option<String>,
    // VWAP state (tick-based)
    vwap_daily: VwapState,
    vwap_session: VwapState,
    current_session: Option<TradingSession>,
    // tvVWAP state (HLC3 on 5m candles - TradingView style) - DEPRECATED: kept for fallback
    tv_vwap: VwapState,
    // ATR-14 state (5m candles, EMA) - DEPRECATED: kept for fallback
    candles_5m: VecDeque<Candle5m>,
    current_candle: Option<Candle5m>,
    atr_state: AtrState,

    // === KLINE-BASED METRICS (authoritative, from Binance 1m klines) ===
    // 1m candle buffer from Binance kline stream (keep 300 = 5 hours)
    candles_1m: VecDeque<Candle1m>,
    // tvVWAP computed from 1m klines (authoritative)
    kline_tv_vwap: VwapState,
    // ATR-14 computed from 1m klines (14 minutes lookback)
    kline_atr_state: AtrState,
    // Flag to indicate kline data is available and should be used
    use_kline_metrics: bool,
    // L2 Orderbook state per exchange
    book_imbalance_by_exchange: HashMap<String, f64>,  // Current imbalance (0-100%, 50% = balanced)
    book_last_update: HashMap<String, DateTime<Utc>>,  // Last update time per exchange
    book_flip_history: VecDeque<(DateTime<Utc>, bool)>, // (time, was_bid_heavy) for flip detection
}

impl TickerState {
    fn new(ticker: String) -> Self {
        Self {
            ticker,
            trades: VecDeque::new(),
            whales_by_exchange: HashMap::new(),
            liquidations: VecDeque::new(),
            cvd_deltas_by_exchange: HashMap::new(),
            cvd_history: VecDeque::new(),
            oi_by_exchange: HashMap::new(),
            oi_history: VecDeque::new(),
            oi_history_by_exchange: HashMap::new(),
            basis_history: VecDeque::new(),
            spot_mid: None,
            perp_mid: None,
            spread_pct: None,
            best_bid: None,
            best_ask: None,
            price_history: VecDeque::new(),
            exchange_volume: VecDeque::new(),
            last_trade_by_exchange: HashMap::new(),
            last_whale_exchange: None,
            last_whale_kind: None,
            vwap_daily: VwapState::default(),
            vwap_session: VwapState::default(),
            current_session: None,
            tv_vwap: VwapState::default(),
            candles_5m: VecDeque::with_capacity(15), // Keep 15 candles (14 for ATR + current)
            current_candle: None,
            atr_state: AtrState::default(),
            // Kline-based metrics (authoritative)
            candles_1m: VecDeque::with_capacity(300), // Keep 300 1m candles (5 hours)
            kline_tv_vwap: VwapState::default(),
            kline_atr_state: AtrState::default(),
            use_kline_metrics: false, // Will be set to true once kline data is available
            book_imbalance_by_exchange: HashMap::new(),
            book_last_update: HashMap::new(),
            book_flip_history: VecDeque::new(),
        }
    }

    /// Backfill tvVWAP and ATR from historical 5m candles
    fn backfill_from_candles(&mut self, candles: Vec<Candle5m>) -> BackfillResult {
        if candles.is_empty() {
            return BackfillResult::default();
        }

        let now = Utc::now();
        let today = now.date_naive();

        // Reset tvVWAP for today
        self.tv_vwap.reset(now);

        // Reset ATR state for fresh calculation from backfill data
        self.atr_state = AtrState::default();

        // Determine current session for session VWAP
        let current_hour = now.hour();
        self.current_session = Some(TradingSession::from_utc_hour(current_hour));

        let mut candles_for_atr = Vec::new();

        for candle in candles {
            let candle_date = candle.start_time.date_naive();

            // Skip incomplete candles (candle end time = start + 5 minutes)
            let candle_end = candle.start_time + ChronoDuration::minutes(5);
            let is_complete = candle_end <= now;

            // Only add to tvVWAP if candle is from today AND complete
            if candle_date == today && is_complete {
                let hlc3 = (candle.high + candle.low + candle.close) / 3.0;
                self.tv_vwap.add_trade(hlc3, candle.volume);
            }

            // Collect candles for ATR (use recent completed candles regardless of day)
            if is_complete {
                candles_for_atr.push(candle);
            }
        }

        // Pre-warm ATR with the last 14+ candles
        // Sort by time and take the most recent ones
        candles_for_atr.sort_by_key(|c| c.start_time);

        // Process candles for ATR (in chronological order)
        for candle in candles_for_atr.iter() {
            self.atr_state.update(candle);
        }

        // Store recent candles for ongoing ATR updates
        let recent_candles: Vec<Candle5m> = candles_for_atr
            .into_iter()
            .rev()
            .take(15)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        self.candles_5m = VecDeque::from(recent_candles);

        BackfillResult {
            candles_loaded: self.candles_5m.len(),
            tv_vwap: self.tv_vwap.vwap(),
            atr_14: if self.atr_state.initialized || self.atr_state.tr_count >= 5 {
                Some(self.atr_state.value())
            } else {
                None
            },
        }
    }

    /// Backfill tvVWAP, ATR, and RV from authoritative Binance 1m klines
    /// This is the preferred method - results will not drift from exchange data
    pub fn backfill_from_1m_candles(&mut self, candles: Vec<Candle1m>) -> BackfillResult {
        if candles.is_empty() {
            return BackfillResult::default();
        }

        let now = Utc::now();
        let today = now.date_naive();

        // Reset kline-based tvVWAP for today
        self.kline_tv_vwap.reset(now);

        // Reset kline-based ATR state
        self.kline_atr_state = AtrState::default();

        // Clear existing 1m candle buffer
        self.candles_1m.clear();

        let mut complete_candles: Vec<Candle1m> = Vec::new();

        for candle in candles {
            // Only process complete candles
            if !candle.is_complete {
                continue;
            }

            let candle_date = candle.start_time.date_naive();

            // Add to tvVWAP if candle is from today
            if candle_date == today {
                let hlc3 = (candle.high + candle.low + candle.close) / 3.0;
                self.kline_tv_vwap.add_trade(hlc3, candle.volume);
            }

            complete_candles.push(candle);
        }

        // Sort by time
        complete_candles.sort_by_key(|c| c.start_time);

        // Update ATR from 1m candles (convert to Candle5m-like for ATR update)
        for candle in complete_candles.iter() {
            // Create a temporary Candle5m for ATR calculation
            let candle_5m = Candle5m {
                open: candle.open,
                high: candle.high,
                low: candle.low,
                close: candle.close,
                volume: candle.volume,
                start_time: candle.start_time,
            };
            self.kline_atr_state.update(&candle_5m);
        }

        // Store recent candles (keep last 300 for RV calculation)
        let recent_candles: Vec<Candle1m> = complete_candles
            .into_iter()
            .rev()
            .take(300)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        self.candles_1m = VecDeque::from(recent_candles);

        // Mark kline metrics as available
        self.use_kline_metrics = true;

        BackfillResult {
            candles_loaded: self.candles_1m.len(),
            tv_vwap: self.kline_tv_vwap.vwap(),
            atr_14: if self.kline_atr_state.initialized || self.kline_atr_state.tr_count >= 5 {
                Some(self.kline_atr_state.value())
            } else {
                None
            },
        }
    }

    /// Process a single 1m kline update from WebSocket stream
    pub fn push_1m_candle(&mut self, candle: Candle1m) {
        // Only process complete candles for tvVWAP/ATR
        if !candle.is_complete {
            return;
        }

        let now = Utc::now();
        let today = now.date_naive();
        let candle_date = candle.start_time.date_naive();

        // Check if tvVWAP needs daily reset (new day at 00:00 UTC)
        let needs_reset = match self.kline_tv_vwap.last_reset {
            Some(last) => candle.start_time.date_naive() != last.date_naive(),
            None => true,
        };
        if needs_reset {
            self.kline_tv_vwap.reset(candle.start_time);
        }

        // Add to tvVWAP if candle is from today
        if candle_date == today {
            let hlc3 = (candle.high + candle.low + candle.close) / 3.0;
            self.kline_tv_vwap.add_trade(hlc3, candle.volume);
        }

        // Update ATR
        let candle_5m = Candle5m {
            open: candle.open,
            high: candle.high,
            low: candle.low,
            close: candle.close,
            volume: candle.volume,
            start_time: candle.start_time,
        };
        self.kline_atr_state.update(&candle_5m);

        // Add to buffer (avoid duplicates by checking start_time)
        if self.candles_1m.back().map(|c| c.start_time) != Some(candle.start_time) {
            self.candles_1m.push_back(candle);
            // Keep buffer size manageable
            while self.candles_1m.len() > 300 {
                self.candles_1m.pop_front();
            }
        }

        // Mark kline metrics as available
        self.use_kline_metrics = true;
    }

    /// Calculate RV from 1m klines for a given window (in minutes)
    fn calculate_rv_from_1m(&self, minutes: usize) -> Option<f64> {
        if self.candles_1m.len() < minutes {
            return None;
        }

        // Get the last N candles
        let candles: Vec<&Candle1m> = self.candles_1m.iter().rev().take(minutes).collect();
        if candles.len() < 3 {
            return None;
        }

        // Calculate close-to-close returns
        let mut returns = Vec::with_capacity(candles.len() - 1);
        for i in 0..candles.len() - 1 {
            let current = candles[i].close;
            let prev = candles[i + 1].close;
            if prev > 0.0 {
                returns.push((current - prev) / prev);
            }
        }

        if returns.is_empty() {
            return None;
        }

        // Calculate standard deviation
        let mean: f64 = returns.iter().sum::<f64>() / returns.len() as f64;
        let variance: f64 = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / returns.len() as f64;
        let std_dev = variance.sqrt();

        // Return as percentage
        Some(std_dev * 100.0)
    }

    /// Get RV30 (30-minute realized volatility from 1m candles)
    pub fn rv_30m_from_klines(&self) -> Option<f64> {
        self.calculate_rv_from_1m(30)
    }

    /// Get RV60 (60-minute realized volatility from 1m candles)
    pub fn rv_1h_from_klines(&self) -> Option<f64> {
        self.calculate_rv_from_1m(60)
    }

    fn push_trade(
        &mut self,
        trade: TradeData,
        exchange: &str,
        time: DateTime<Utc>,
        is_spot: bool,
        is_perp: bool,
    ) {
        // NORMALIZE: OKX reports trade amounts in contracts, not base currency
        // OKX contract sizes: BTC=0.01, ETH=0.1, SOL=1
        // Binance and Bybit report in base currency already
        let normalized_amount = if exchange.to_lowercase().contains("okx") && is_perp {
            match self.ticker.as_str() {
                "BTC" => trade.amount * 0.01,  // 100 contracts = 1 BTC
                "ETH" => trade.amount * 0.1,   // 10 contracts = 1 ETH
                _ => trade.amount,              // SOL and others: 1:1
            }
        } else {
            trade.amount
        };

        let usd = trade.price * normalized_amount;
        let side = trade.side.clone();
        let record = TradeRecord {
            time,
            side: side.clone(),
            price: trade.price,
            amount: normalized_amount,  // Use normalized amount for VWAP, etc.
            exchange: exchange.to_string(),
            usd,
            is_spot,
            is_perp,
        };

        self.trades.push_back(record.clone());
        self.price_history.push_back((time, trade.price));

        // Create exchange key with market type: "OKX-PERP", "BNC-SPOT", etc.
        let market_kind = if is_spot {
            "SPOT"
        } else if is_perp {
            "PERP"
        } else {
            "OTHER"
        };
        let exchange_key = format!("{}-{}", abbreviate_exchange(exchange), market_kind);

        self.exchange_volume
            .push_back((time, exchange_key.clone(), usd));
        self.last_trade_by_exchange
            .insert(exchange_key, trade.price);

        // Whale threshold (USD notional) - adaptive based on recent avg trade size
        let (_, _, avg_5m) = self.trade_stats(300);
        let adaptive_threshold = if avg_5m > 0.0 {
            (avg_5m * whale_multiplier()).max(whale_floor())
        } else {
            whale_threshold()
        };

        if usd >= adaptive_threshold {
            let market_kind_str = if is_spot {
                "SPOT"
            } else if is_perp {
                "PERP"
            } else {
                "OTHER"
            };

            let record = WhaleRecord {
                time,
                side: side.clone(),
                volume_usd: usd,
                price: trade.price,
                exchange: exchange.to_string(),
                market_kind: market_kind_str.to_string(),
            };

            // Push into per-exchange buffer; cap per exchange so one venue can't drown others
            let cap_per_exchange = (max_whales() / 3).max(50).min(max_whales());
            let deque = self
                .whales_by_exchange
                .entry(exchange.to_string())
                .or_insert_with(VecDeque::new);
            deque.push_front(record.clone());
            while deque.len() > cap_per_exchange {
                deque.pop_back();
            }

            self.last_whale_exchange = Some(exchange.to_string());
            self.last_whale_kind = Some(if is_spot {
                "SPOT".to_string()
            } else if is_perp {
                "PERP".to_string()
            } else {
                "OTHER".to_string()
            });
        }

        if is_spot {
            self.spot_mid = Some(trade.price);
        }
        if is_perp {
            self.perp_mid = Some(trade.price);
        }

        // Update CVD history based on trades (perps only), windowed
        if is_perp {
            let total = self.cvd_total(CVD_RETENTION_SECS);
            self.cvd_history.push_back(CvdRecord {
                time,
                total_quote: total,
            });
        }

        // Update VWAP and ATR (perps only for consistency)
        if is_perp {
            self.update_vwap(trade.price, normalized_amount, time);
            // Only update trade-based candles if kline metrics are NOT available (fallback only)
            // When kline metrics are active, tvVWAP/ATR/RV come from authoritative 1m klines
            if !self.use_kline_metrics {
                let is_binance = exchange.to_lowercase().contains("binance");
                if is_binance {
                    self.update_candle(trade.price, normalized_amount, time);
                }
            }
        }

        self.prune(time);
    }

    /// Update daily and session VWAP accumulators
    fn update_vwap(&mut self, price: f64, volume: f64, time: DateTime<Utc>) {
        let current_hour = time.hour();
        let current_session = TradingSession::from_utc_hour(current_hour);

        // Check if daily VWAP needs reset (new day at 00:00 UTC)
        let needs_daily_reset = match self.vwap_daily.last_reset {
            Some(last) => time.date_naive() != last.date_naive(),
            None => true,
        };
        if needs_daily_reset {
            self.vwap_daily.reset(time);
        }

        // Check if session VWAP needs reset (session changed)
        let needs_session_reset = match self.current_session {
            Some(prev) => prev != current_session,
            None => true,
        };
        if needs_session_reset {
            self.vwap_session.reset(time);
            self.current_session = Some(current_session);
        }

        // Add trade to both VWAPs
        self.vwap_daily.add_trade(price, volume);
        self.vwap_session.add_trade(price, volume);
    }

    /// Update 5m candle and ATR
    fn update_candle(&mut self, price: f64, volume: f64, time: DateTime<Utc>) {
        // Calculate candle start time (aligned to 5-minute boundaries)
        let minute = time.minute();
        let candle_minute = (minute / 5) * 5;
        let candle_start = time
            .with_minute(candle_minute)
            .and_then(|t| t.with_second(0))
            .and_then(|t| t.with_nanosecond(0))
            .unwrap_or(time);

        // Check if tvVWAP needs daily reset (00:00 UTC)
        let needs_tv_reset = match self.tv_vwap.last_reset {
            Some(last) => time.date_naive() != last.date_naive(),
            None => true,
        };
        if needs_tv_reset {
            self.tv_vwap.reset(time);
        }

        // Check if we need to start a new candle
        let need_new_candle = match &self.current_candle {
            Some(candle) => candle.start_time != candle_start,
            None => true,
        };

        if need_new_candle {
            // Finalize previous candle if exists
            if let Some(completed) = self.current_candle.take() {
                // Update ATR with completed candle
                self.atr_state.update(&completed);

                // Update tvVWAP with HLC3 (TradingView style)
                let hlc3 = (completed.high + completed.low + completed.close) / 3.0;
                self.tv_vwap.add_trade(hlc3, completed.volume);

                // Store completed candle (keep last 15)
                self.candles_5m.push_back(completed);
                while self.candles_5m.len() > 15 {
                    self.candles_5m.pop_front();
                }
            }

            // Start new candle
            self.current_candle = Some(Candle5m {
                open: price,
                high: price,
                low: price,
                close: price,
                volume,
                start_time: candle_start,
            });
        } else if let Some(candle) = &mut self.current_candle {
            // Update current candle
            candle.high = candle.high.max(price);
            candle.low = candle.low.min(price);
            candle.close = price;
            candle.volume += volume;
        }
    }

    fn push_liquidation(&mut self, liq: LiquidationData, exchange: &str, time: DateTime<Utc>) {
        let value = liq.price * liq.quantity;
        self.liquidations.push_back(LiquidationRecord {
            time,
            side: liq.side,
            price: liq.price,
            value,
            exchange: exchange.to_string(),
        });

        self.prune(time);
    }

    fn push_cvd(&mut self, exchange: &str, cvd: CvdData, time: DateTime<Utc>) {
        // We now derive CVD from trades; keep minimal pruning of history
        let cutoff = time - ChronoDuration::seconds(CVD_RETENTION_SECS);
        while let Some(front) = self.cvd_history.front() {
            if front.time < cutoff {
                self.cvd_history.pop_front();
            } else {
                break;
            }
        }
    }

    fn push_oi(&mut self, exchange: &str, contracts: f64) {
        // NORMALIZE: OKX reports raw contracts, not base currency
        // OKX contract sizes: BTC=0.01, ETH=0.1, SOL=1
        // Binance and Bybit report in base currency already
        let normalized = if exchange.to_lowercase().contains("okx") {
            match self.ticker.as_str() {
                "BTC" => contracts * 0.01,  // 100 contracts = 1 BTC
                "ETH" => contracts * 0.1,   // 10 contracts = 1 ETH
                _ => contracts,              // SOL and others: 1:1
            }
        } else {
            contracts
        };

        self.oi_by_exchange.insert(exchange.to_string(), normalized);

        // P1: Track OI time-series for velocity calculation
        let now = Utc::now();
        let total: f64 = self.oi_by_exchange.values().copied().sum();
        self.oi_history.push_back(OiRecord { time: now, total });

        // Track per-exchange history
        let deque = self
            .oi_history_by_exchange
            .entry(exchange.to_string())
            .or_insert_with(VecDeque::new);
        deque.push_back(OiRecord {
            time: now,
            total: normalized,
        });

        // Prune old OI records
        let cutoff = now - ChronoDuration::seconds(OI_RETENTION_SECS);
        while let Some(front) = self.oi_history.front() {
            if front.time < cutoff {
                self.oi_history.pop_front();
            } else {
                break;
            }
        }
        for deque in self.oi_history_by_exchange.values_mut() {
            while let Some(front) = deque.front() {
                if front.time < cutoff {
                    deque.pop_front();
                } else {
                    break;
                }
            }
        }
    }

    fn push_orderbook(
        &mut self,
        ob: OrderBookL1Data,
        is_spot: bool,
        is_perp: bool,
        time: DateTime<Utc>,
    ) {
        let mid = ob.mid_price().and_then(|m| m.to_f64());
        let spread_pct = ob.spread_percentage();
        let best_bid = ob
            .best_bid
            .as_ref()
            .and_then(|b| Some((b.price.to_f64()?, b.amount.to_f64()?)));
        let best_ask = ob
            .best_ask
            .as_ref()
            .and_then(|a| Some((a.price.to_f64()?, a.amount.to_f64()?)));

        if is_spot {
            self.spot_mid = mid;
        }
        if is_perp {
            self.perp_mid = mid;
            self.spread_pct = spread_pct;
            if let Some(b) = best_bid {
                self.best_bid = Some(b);
            }
            if let Some(a) = best_ask {
                self.best_ask = Some(a);
            }
        }

        if let Some(mid_price) = mid {
            self.price_history.push_back((time, mid_price));
        }

        // P1: Track basis history for momentum
        if let (Some(spot), Some(perp)) = (self.spot_mid, self.perp_mid) {
            if spot > 0.0 {
                let basis_usd = perp - spot;
                let basis_pct = (basis_usd / spot) * 100.0;
                self.basis_history.push_back(BasisRecord {
                    time,
                    basis_usd,
                    basis_pct,
                });

                // Prune old basis records
                let cutoff = time - ChronoDuration::seconds(BASIS_RETENTION_SECS);
                while let Some(front) = self.basis_history.front() {
                    if front.time < cutoff {
                        self.basis_history.pop_front();
                    } else {
                        break;
                    }
                }
            }
        }

        self.prune(time);
    }

    /// Process L2 orderbook data for book imbalance tracking
    fn push_orderbook_l2(
        &mut self,
        book: &crate::shared::types::OrderBook,
        exchange: &str,
        time: DateTime<Utc>,
    ) {
        // Calculate bid imbalance percentage (0-100%, 50% = balanced)
        // Use all available levels for imbalance calculation
        let levels_available = book.bids.levels.len().min(book.asks.levels.len());
        let imbalance_pct = book.bid_imbalance_pct(levels_available);

        // Check for flip (direction change)
        let is_bid_heavy = imbalance_pct > 50.0;
        if let Some((_, last_was_bid_heavy)) = self.book_flip_history.back() {
            if *last_was_bid_heavy != is_bid_heavy {
                // Direction changed - record the flip
                self.book_flip_history.push_back((time, is_bid_heavy));
            }
        } else {
            // First record
            self.book_flip_history.push_back((time, is_bid_heavy));
        }

        // Prune old flip history (keep last 60 seconds)
        let cutoff = time - ChronoDuration::seconds(60);
        while let Some((flip_time, _)) = self.book_flip_history.front() {
            if *flip_time < cutoff {
                self.book_flip_history.pop_front();
            } else {
                break;
            }
        }

        // Store per-exchange imbalance
        // Normalize exchange name to short form for display
        let exchange_short = match exchange {
            ex if ex.contains("Binance") => "BNC",
            ex if ex.contains("Bybit") => "BBT",
            ex if ex.contains("Okx") => "OKX",
            _ => exchange,
        };
        self.book_imbalance_by_exchange
            .insert(exchange_short.to_string(), imbalance_pct);
        self.book_last_update.insert(exchange_short.to_string(), time);
    }

    fn last_whale(&self, exchange: &str) -> Option<&str> {
        match &self.last_whale_exchange {
            Some(ex) if ex == exchange => self.last_whale_kind.as_deref(),
            _ => None,
        }
    }

    fn prune(&mut self, now: DateTime<Utc>) {
        let trade_cutoff = now - ChronoDuration::seconds(TRADE_RETENTION_SECS);
        while let Some(front) = self.trades.front() {
            if front.time < trade_cutoff {
                self.trades.pop_front();
            } else {
                break;
            }
        }

        while let Some(front) = self.exchange_volume.front() {
            if front.0 < trade_cutoff {
                self.exchange_volume.pop_front();
            } else {
                break;
            }
        }

        let liq_cutoff = now - ChronoDuration::seconds(LIQ_RETENTION_SECS);
        while let Some(front) = self.liquidations.front() {
            if front.time < liq_cutoff {
                self.liquidations.pop_front();
            } else {
                break;
            }
        }

        let cvd_cutoff = now - ChronoDuration::seconds(CVD_RETENTION_SECS);
        while let Some(front) = self.cvd_history.front() {
            if front.time < cvd_cutoff {
                self.cvd_history.pop_front();
            } else {
                break;
            }
        }

        let price_cutoff = now - ChronoDuration::seconds(PRICE_RETENTION_SECS);
        while let Some(front) = self.price_history.front() {
            if front.0 < price_cutoff {
                self.price_history.pop_front();
            } else {
                break;
            }
        }
    }

    fn to_snapshot(&self) -> TickerSnapshot {
        // P0: Multi-timeframe orderflow (5s, 15s, 30s for scalper; 1m, 5m for swing)
        let orderflow_5s = self.orderflow(5);
        let orderflow_15s = self.orderflow(15);
        let orderflow_30s = self.orderflow(30);
        let orderflow_1m = self.orderflow(60);
        let orderflow_5m = self.orderflow(300);
        let exchange_dominance = self.exchange_dominance(60);
        let vwap_30s = self.vwap(30);
        let vwap_1m = self.vwap(60);
        let vwap_5m = self.vwap(300);
        // Per-exchange fairness: ensure all exchanges represented in whale display
        let whales: Vec<WhaleRecord> = self.fair_whale_selection(20);
        let (clusters, cascade_risk, next_level, protection_level) = self.liquidation_clusters();
        let liq_rate_per_min = self.liquidation_rate_per_min();
        let liq_bucket = self.liquidation_bucket_size();
        let cvd = self.cvd_summary();
        // P0: Multi-timeframe CVD (5s, 15s, 30s for scalper; 1m, 5m, 15m for swing)
        let cvd_5s = self.cvd_total(5);
        let cvd_15s = self.cvd_total(15);
        let cvd_30s = self.cvd_total(30);
        let cvd_1m_total = self.cvd_total(60);
        let cvd_5m_total = self.cvd_total(300);
        let cvd_15m_total = self.cvd_total(900);
        let cvd_per_exchange_5m = self.cvd_per_exchange(300);
        let cvd_per_exchange_15m = self.cvd_per_exchange(900);
        // CVD velocity (change per minute)
        let cvd_velocity_1m = cvd_1m_total; // 1m total IS the velocity
        let cvd_velocity_5m = cvd_5m_total / 5.0; // per minute
        let cvd_velocity_15m = cvd_15m_total / 15.0; // per minute
        // P1: OI with velocity - enhanced with per-exchange
        let oi_total: f64 = self.oi_by_exchange.values().copied().sum();
        let (oi_delta_5m, oi_velocity) = self.oi_velocity(300);
        let (oi_delta_15m, _) = self.oi_velocity(900);
        let oi_per_exchange = self.oi_by_exchange.clone();
        let (oi_delta_per_exchange_5m, oi_delta_per_exchange_15m) = self.oi_delta_per_exchange();
        // P0: Multi-timeframe tick direction (30s, 1m, 5m)
        let tick_direction_30s = self.tick_direction(30);
        let tick_direction = self.tick_direction(60);
        let tick_direction_5m = self.tick_direction(300);
        let (trade_speed, avg_trade_usd) = self.trade_speed(60);
        let (trades_5m, vol_5m, avg_5m) = self.trade_stats(300);
        let basis = self.basis();
        let cvd_divergence = self.cvd_divergence();
        let cvd_divergence_15s = self.cvd_divergence_15s();
        // P1: Flow signal and basis momentum
        let flow_signal = self.detect_flow_signal();
        let basis_momentum = self.basis_momentum();

        // VWAP calculations
        let vwap_daily = self.vwap_daily.vwap();
        let vwap_session = self.vwap_session.vwap();
        let current_session = self.current_session;

        // VWAP deviation: (price - vwap) / vwap * 100
        let vwap_daily_deviation = match (self.latest_price(), vwap_daily) {
            (Some(price), Some(vwap)) if vwap > 0.0 => Some((price - vwap) / vwap * 100.0),
            _ => None,
        };

        // tvVWAP (TradingView style - HLC3 on candles)
        // Prefer kline-based metrics (authoritative) when available
        let tv_vwap = if self.use_kline_metrics {
            self.kline_tv_vwap.vwap()
        } else {
            self.tv_vwap.vwap()
        };
        let tv_vwap_deviation = match (self.latest_price(), tv_vwap) {
            (Some(price), Some(vwap)) if vwap > 0.0 => Some((price - vwap) / vwap * 100.0),
            _ => None,
        };

        // ATR-14 (candles, EMA)
        // Prefer kline-based ATR (1m candles, 14-minute lookback) when available
        let atr_14 = if self.use_kline_metrics {
            if self.kline_atr_state.initialized || self.kline_atr_state.tr_count >= 5 {
                Some(self.kline_atr_state.value())
            } else {
                None
            }
        } else if self.atr_state.initialized || self.atr_state.tr_count >= 5 {
            Some(self.atr_state.value())
        } else {
            None
        };
        let atr_14_pct = match (atr_14, self.latest_price()) {
            (Some(atr), Some(price)) if price > 0.0 => Some(atr / price * 100.0),
            _ => None,
        };

        // Realized Volatility (30m and 1h)
        // Prefer kline-based RV (1m candles) when available
        let (realized_vol_30m, realized_vol_1h, realized_vol_trend) = if self.use_kline_metrics {
            let rv_30m = self.rv_30m_from_klines();
            let rv_1h = self.rv_1h_from_klines();
            let trend = match (rv_30m, rv_1h) {
                (Some(r30), Some(r60)) if r60 > 0.0 => {
                    let ratio = r30 / r60;
                    if ratio > 1.2 {
                        VolTrend::Expanding
                    } else if ratio < 0.8 {
                        VolTrend::Contracting
                    } else {
                        VolTrend::Stable
                    }
                }
                _ => VolTrend::Stable,
            };
            (rv_30m, rv_1h, trend)
        } else {
            self.calculate_realized_volatility()
        };

        TickerSnapshot {
            ticker: self.ticker.clone(),
            latest_price: self.latest_price(),
            latest_spread_pct: self.spread_pct,
            spot_mid: self.spot_mid,
            perp_mid: self.perp_mid,
            basis,
            orderflow_5s,
            orderflow_15s,
            orderflow_30s,
            orderflow_1m,
            orderflow_5m,
            exchange_dominance,
            vwap_30s,
            vwap_1m,
            vwap_5m,
            whales,
            liquidations: clusters,
            liq_rate_per_min,
            liq_bucket,
            cascade_risk,
            next_cascade_level: next_level,
            protection_level,
            cvd,
            cvd_5s,
            cvd_15s,
            cvd_30s,
            cvd_1m_total,
            cvd_5m_total,
            cvd_15m_total,
            cvd_per_exchange_5m,
            cvd_per_exchange_15m,
            cvd_velocity_1m,
            cvd_velocity_5m,
            cvd_velocity_15m,
            per_exchange_30s: self.per_exchange_short_stats(30),
            oi_total,
            oi_delta_5m,
            oi_delta_15m,
            oi_velocity,
            oi_per_exchange,
            oi_delta_per_exchange_5m,
            oi_delta_per_exchange_15m,
            exchange_health: HashMap::new(), // TODO: populate from aggregator
            tick_direction,
            tick_direction_5m,
            tick_direction_30s,
            trade_speed,
            avg_trade_usd,
            trades_5m,
            vol_5m,
            avg_trade_usd_5m: avg_5m,
            cvd_divergence,
            cvd_divergence_15s,
            flow_signal,
            basis_momentum,
            best_bid: self.best_bid,
            best_ask: self.best_ask,
            exchange_prices: self.last_trade_by_exchange.clone(),
            vwap_daily,
            vwap_session,
            current_session,
            vwap_daily_deviation,
            tv_vwap,
            tv_vwap_deviation,
            // For UI gating: when using klines, report 1m buffer length; otherwise 5m buffer
            candles_5m_len: if self.use_kline_metrics {
                self.candles_1m.len()
            } else {
                self.candles_5m.len()
            },
            atr_14,
            atr_14_pct,
            realized_vol_30m,
            realized_vol_1h,
            realized_vol_trend,
            // L2 Orderbook imbalance
            per_exchange_book_imbalance: self.book_imbalance_by_exchange.clone(),
            aggregated_book_imbalance: self.aggregated_book_imbalance(),
            book_flip_count: self.book_flip_count_30s(),
            book_freshness: self.book_freshness_seconds(),
        }
    }

    /// Calculate aggregated book imbalance across all exchanges
    fn aggregated_book_imbalance(&self) -> f64 {
        if self.book_imbalance_by_exchange.is_empty() {
            return 50.0; // Neutral
        }
        let sum: f64 = self.book_imbalance_by_exchange.values().sum();
        sum / self.book_imbalance_by_exchange.len() as f64
    }

    /// Count book flips in the last 30 seconds
    fn book_flip_count_30s(&self) -> usize {
        let now = Utc::now();
        let cutoff = now - ChronoDuration::seconds(30);
        self.book_flip_history
            .iter()
            .filter(|(t, _)| *t >= cutoff)
            .count()
            .saturating_sub(1) // Don't count the initial state as a flip
    }

    /// Get seconds since last book update per exchange
    fn book_freshness_seconds(&self) -> HashMap<String, f64> {
        let now = Utc::now();
        self.book_last_update
            .iter()
            .map(|(ex, last)| {
                let age = (now - *last).num_milliseconds() as f64 / 1000.0;
                (ex.clone(), age)
            })
            .collect()
    }

    /// Calculate realized volatility for a given number of candles
    /// Returns volatility as percentage
    fn calculate_rv_for_window(&self, num_candles: usize) -> Option<f64> {
        if self.candles_5m.len() < num_candles {
            return None;
        }

        let candles: Vec<&Candle5m> = self.candles_5m.iter().rev().take(num_candles).collect();
        if candles.len() < 3 {
            return None;
        }

        // Calculate close-to-close returns
        let mut returns: Vec<f64> = Vec::new();
        for i in 0..candles.len() - 1 {
            let current = candles[i].close;
            let prev = candles[i + 1].close;
            if prev > 0.0 {
                let ret = (current - prev) / prev;
                returns.push(ret);
            }
        }

        if returns.len() < 2 {
            return None;
        }

        // Calculate standard deviation of returns
        let mean: f64 = returns.iter().sum::<f64>() / returns.len() as f64;
        let variance: f64 = returns.iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>() / returns.len() as f64;
        let std_dev = variance.sqrt();

        Some(std_dev * 100.0)
    }

    /// Calculate 30m and 1h realized volatility, plus trend
    /// Returns (rv_30m, rv_1h, trend)
    fn calculate_realized_volatility(&self) -> (Option<f64>, Option<f64>, VolTrend) {
        let rv_30m = self.calculate_rv_for_window(6);   // 6 × 5m = 30 min
        let rv_1h = self.calculate_rv_for_window(12);   // 12 × 5m = 60 min

        // Determine trend by comparing 30m vs 1h
        // If 30m > 1h → volatility expanding (recent is hotter)
        // If 30m < 1h → volatility contracting (recent is calmer)
        let trend = match (rv_30m, rv_1h) {
            (Some(short), Some(long)) if long > 0.0 => {
                let ratio = short / long;
                if ratio > 1.2 {
                    VolTrend::Expanding
                } else if ratio < 0.8 {
                    VolTrend::Contracting
                } else {
                    VolTrend::Stable
                }
            }
            _ => VolTrend::Stable,
        };

        (rv_30m, rv_1h, trend)
    }

    fn latest_price(&self) -> Option<f64> {
        if let Some((_, p)) = self.price_history.back() {
            Some(*p)
        } else {
            self.perp_mid.or(self.spot_mid)
        }
    }

    fn orderflow(&self, window_secs: i64) -> OrderflowStats {
        let now = Utc::now();
        let cutoff = now - ChronoDuration::seconds(window_secs);
        let mut buy = 0.0;
        let mut sell = 0.0;
        let mut trades = 0u64;

        for t in self.trades.iter().rev() {
            if t.time < cutoff {
                break;
            }
            trades += 1;
            match t.side {
                Side::Buy => buy += t.usd,
                Side::Sell => sell += t.usd,
            }
        }

        let total = buy + sell;
        let imbalance_pct = if total > 0.0 {
            buy / total * 100.0
        } else {
            50.0
        };
        let net_flow_per_min = if window_secs > 0 {
            (buy - sell) * 60.0 / window_secs as f64
        } else {
            0.0
        };
        let trades_per_sec = if window_secs > 0 {
            trades as f64 / window_secs as f64
        } else {
            0.0
        };

        OrderflowStats {
            buy_usd: buy,
            sell_usd: sell,
            imbalance_pct,
            net_flow_per_min,
            trades_per_sec,
        }
    }

    fn exchange_dominance(&self, window_secs: i64) -> HashMap<String, f64> {
        let now = Utc::now();
        let cutoff = now - ChronoDuration::seconds(window_secs);
        let mut totals: HashMap<String, f64> = HashMap::new();
        for (time, exch, usd) in self.exchange_volume.iter().rev() {
            if *time < cutoff {
                break;
            }
            *totals.entry(exch.clone()).or_insert(0.0) += *usd;
        }

        let total_vol: f64 = totals.values().copied().sum();
        if total_vol > 0.0 {
            totals
                .iter_mut()
                .for_each(|(_, v)| *v = (*v / total_vol) * 100.0);
        }

        totals
    }

    fn vwap(&self, window_secs: i64) -> Option<f64> {
        let now = Utc::now();
        let cutoff = now - ChronoDuration::seconds(window_secs);
        let mut sum_pv = 0.0;
        let mut sum_v = 0.0;

        for t in self.trades.iter().rev() {
            if t.time < cutoff {
                break;
            }
            sum_pv += t.price * t.amount;
            sum_v += t.amount;
        }

        if sum_v > 0.0 {
            Some(sum_pv / sum_v)
        } else {
            None
        }
    }

    fn liquidation_clusters(
        &self,
    ) -> (
        Vec<LiquidationCluster>,
        f64,
        Option<CascadeLevel>,
        Option<CascadeLevel>,
    ) {
        let cutoff = Utc::now() - ChronoDuration::seconds(LIQ_RETENTION_SECS);
        // Bucket size by asset class: BTC ~$100, ETH ~$50, SOL/others ~$10
        let bucket_size = match self.ticker.as_str() {
            "BTC" => 100.0,
            "ETH" => 50.0,
            _ => 10.0,
        };
        let mut buckets: HashMap<i64, Vec<&LiquidationRecord>> = HashMap::new();

        for liq in self.liquidations.iter().rev() {
            if liq.time < cutoff {
                break;
            }
            let bucket = (liq.price / bucket_size).floor() as i64;
            buckets.entry(bucket).or_default().push(liq);
        }

        let mut clusters: Vec<LiquidationCluster> = buckets
            .iter()
            .map(|(bucket, entries)| {
                let total_usd: f64 = entries.iter().map(|l| l.value).sum();
                let long_count = entries.iter().filter(|l| l.side == Side::Buy).count();
                let short_count = entries.iter().filter(|l| l.side == Side::Sell).count();

                LiquidationCluster {
                    price_level: (*bucket as f64) * bucket_size,
                    total_usd,
                    long_count,
                    short_count,
                }
            })
            .collect();

        clusters.sort_by(|a, b| b.total_usd.partial_cmp(&a.total_usd).unwrap());

        let cascade_risk = clusters
            .first()
            .map(|c| ((c.total_usd / 50_000_000.0) * 100.0).min(100.0))
            .unwrap_or(0.0);

        let current_price = self.latest_price().unwrap_or(0.0);
        let mut next_level: Option<CascadeLevel> = None;
        let mut protection_level: Option<CascadeLevel> = None;

        for c in clusters.iter() {
            if current_price == 0.0 {
                break;
            }

            if c.price_level < current_price {
                let longs_usd = c.long_count as f64
                    * (c.total_usd / (c.long_count + c.short_count).max(1) as f64);
                if longs_usd > liq_danger_threshold() {
                    if let Some(existing) = &next_level {
                        if c.total_usd > existing.total_usd {
                            next_level = Some(CascadeLevel {
                                price: c.price_level,
                                total_usd: c.total_usd,
                                side: Side::Buy,
                            });
                        }
                    } else {
                        next_level = Some(CascadeLevel {
                            price: c.price_level,
                            total_usd: c.total_usd,
                            side: Side::Buy,
                        });
                    }
                }
            } else if c.price_level > current_price {
                let shorts_usd = c.short_count as f64
                    * (c.total_usd / (c.long_count + c.short_count).max(1) as f64);
                if shorts_usd > liq_danger_threshold() {
                    if let Some(existing) = &protection_level {
                        if c.total_usd > existing.total_usd {
                            protection_level = Some(CascadeLevel {
                                price: c.price_level,
                                total_usd: c.total_usd,
                                side: Side::Sell,
                            });
                        }
                    } else {
                        protection_level = Some(CascadeLevel {
                            price: c.price_level,
                            total_usd: c.total_usd,
                            side: Side::Sell,
                        });
                    }
                }
            }
        }

        (clusters, cascade_risk, next_level, protection_level)
    }

    fn liquidation_bucket_size(&self) -> f64 {
        match self.ticker.as_str() {
            "BTC" => 100.0,
            "ETH" => 50.0,
            _ => 10.0,
        }
    }

    fn liquidation_rate_per_min(&self) -> f64 {
        let now = Utc::now();
        let cutoff = now - ChronoDuration::seconds(LIQ_RETENTION_SECS);
        let count = self
            .liquidations
            .iter()
            .rev()
            .take_while(|liq| liq.time >= cutoff)
            .count() as f64;
        count / (LIQ_RETENTION_SECS as f64 / 60.0)
    }

    fn cvd_summary(&self) -> CvdSummary {
        let total = self.cvd_total(CVD_RETENTION_SECS);

        let velocity = if let (Some(first), Some(last)) = (self.cvd_history.front(), self.cvd_history.back()) {
            if last.time > first.time {
                (last.total_quote - first.total_quote)
                    / (last.time - first.time).num_seconds().max(1) as f64
            } else {
                0.0
            }
        } else {
            0.0
        };

        CvdSummary {
            total_quote: total,
            velocity_quote: velocity,
        }
    }

    fn tick_direction(&self, window_secs: i64) -> TickDirection {
        let now = Utc::now();
        let cutoff = now - ChronoDuration::seconds(window_secs);
        let mut upticks = 0u64;
        let mut downticks = 0u64;

        let mut prev: Option<f64> = None;
        for (time, price) in self.price_history.iter().rev() {
            if *time < cutoff {
                break;
            }
            if let Some(prev_price) = prev {
                if price > &prev_price {
                    upticks += 1;
                } else if price < &prev_price {
                    downticks += 1;
                }
            }
            prev = Some(*price);
        }

        let total = upticks + downticks;
        let uptick_pct = if total > 0 {
            upticks as f64 / total as f64 * 100.0
        } else {
            50.0
        };

        TickDirection {
            upticks,
            downticks,
            uptick_pct,
        }
    }

    fn trade_speed(&self, window_secs: i64) -> (f64, f64) {
        let now = Utc::now();
        let cutoff = now - ChronoDuration::seconds(window_secs);
        let mut trades = 0u64;
        let mut total_usd = 0.0;

        for t in self.trades.iter().rev() {
            if t.time < cutoff {
                break;
            }
            trades += 1;
            total_usd += t.usd;
        }

        let trade_speed = if window_secs > 0 {
            trades as f64 / window_secs as f64
        } else {
            0.0
        };
        let avg_trade_usd = if trades > 0 {
            total_usd / trades as f64
        } else {
            0.0
        };

        (trade_speed, avg_trade_usd)
    }

    /// Trade stats over a window: count, volume (usd), avg usd
    fn trade_stats(&self, window_secs: i64) -> (usize, f64, f64) {
        let now = Utc::now();
        let cutoff = now - ChronoDuration::seconds(window_secs);
        let mut count = 0usize;
        let mut vol_usd = 0.0;
        for t in self.trades.iter().rev() {
            if t.time < cutoff {
                break;
            }
            count += 1;
            vol_usd += t.usd;
        }
        let avg = if count > 0 { vol_usd / count as f64 } else { 0.0 };
        (count, vol_usd, avg)
    }

    fn basis(&self) -> Option<BasisStats> {
        let spot = self.spot_mid?;
        let perp = self.perp_mid?;
        if spot <= 0.0 {
            return None;
        }

        let basis_usd = perp - spot;
        let raw_pct = (basis_usd / spot) * 100.0;
        // Smooth small flips around zero to avoid flicker
        let basis_pct = (raw_pct * 100.0).round() / 100.0; // 2 decimal places
        let neutral_band = 0.05; // 5 bps deadband

        let state = if basis_pct.abs() < neutral_band {
            BasisState::Unknown
        } else if basis_pct > 0.0 {
            BasisState::Contango
        } else {
            BasisState::Backwardation
        };

        let steep = basis_pct.abs() > 0.5;

        Some(BasisStats {
            basis_usd,
            basis_pct,
            state,
            steep,
        })
    }

    /// Fair whale selection: distribute display slots across exchanges
    /// to prevent high-volume exchanges from drowning others
    fn fair_whale_selection(&self, limit: usize) -> Vec<WhaleRecord> {
        let exchange_count = self.whales_by_exchange.len();
        if exchange_count == 0 {
            return Vec::new();
        }

        // Allocate slots per exchange (min 3 per exchange if possible)
        let slots_per_exchange = (limit / exchange_count).max(3);

        // Take top N whales from each exchange fairly
        let mut result = Vec::with_capacity(limit);
        for deque in self.whales_by_exchange.values() {
            result.extend(deque.iter().take(slots_per_exchange).cloned());
        }

        // Sort by time (most recent first) and limit
        result.sort_by(|a, b| b.time.cmp(&a.time));
        result.truncate(limit);
        result
    }

    /// Rolling CVD total over a window (seconds)
    fn cvd_total(&self, window_secs: i64) -> f64 {
        let now = Utc::now();
        let cutoff = now - ChronoDuration::seconds(window_secs);
        self.trades
            .iter()
            .rev()
            .take_while(|t| t.time >= cutoff)
            .filter(|t| t.is_perp)
            .map(|t| match t.side {
                Side::Buy => t.usd,
                Side::Sell => -t.usd,
            })
            .sum()
    }

    /// Per-exchange CVD totals over a window
    fn cvd_per_exchange(&self, window_secs: i64) -> HashMap<String, f64> {
        let now = Utc::now();
        let cutoff = now - ChronoDuration::seconds(window_secs);
        let mut totals: HashMap<String, f64> = HashMap::new();
        for t in self.trades.iter().rev() {
            if t.time < cutoff {
                break;
            }
            if t.is_perp {
                let signed = match t.side {
                    Side::Buy => t.usd,
                    Side::Sell => -t.usd,
                };
                *totals.entry(t.exchange.clone()).or_insert(0.0) += signed;
            }
        }
        totals
    }

    /// Per-exchange short-window stats (CVD/volume/trades) for scalper
    fn per_exchange_short_stats(&self, window_secs: i64) -> HashMap<String, PerExchangeShortStats> {
        let now = Utc::now();
        let cutoff = now - ChronoDuration::seconds(window_secs);
        let mut acc: HashMap<String, (f64, f64, usize)> = HashMap::new(); // exchange -> (buy_usd, sell_usd, trades)

        for t in self.trades.iter().rev() {
            if t.time < cutoff {
                break;
            }
            if t.is_perp {
                let entry = acc.entry(t.exchange.clone()).or_insert((0.0, 0.0, 0));
                match t.side {
                    Side::Buy => entry.0 += t.usd,
                    Side::Sell => entry.1 += t.usd,
                }
                entry.2 += 1;
            }
        }

        acc.into_iter()
            .map(|(ex, (buy, sell, trades))| {
                let total = buy + sell;
                let cvd = buy - sell;
                (
                    ex,
                    PerExchangeShortStats {
                        cvd_30s: cvd,
                        total_30s: total,
                        trades_30s: trades,
                    },
                )
            })
            .collect()
    }

    fn cvd_divergence(&self) -> DivergenceSignal {
        if self.price_history.len() < 2 || self.cvd_history.len() < 2 {
            return DivergenceSignal::Unknown;
        }

        let price_trend = self.price_history.back().map(|(_, p)| *p).unwrap_or(0.0)
            - self.price_history.front().map(|(_, p)| *p).unwrap_or(0.0);

        let cvd_trend = self
            .cvd_history
            .back()
            .map(|c| c.total_quote)
            .unwrap_or(0.0)
            - self
                .cvd_history
                .front()
                .map(|c| c.total_quote)
                .unwrap_or(0.0);

        // P0: Tighten thresholds for more actionable signals
        // Price threshold: 0.05% instead of 0.1% (more sensitive)
        let price_threshold = self.latest_price().unwrap_or(1.0) * 0.0005;
        // CVD threshold scaled by asset: BTC needs higher, alts lower
        let cvd_threshold = match self.ticker.as_str() {
            "BTC" => 50_000.0,  // $50K for BTC (was $1K - too sensitive)
            "ETH" => 20_000.0,  // $20K for ETH
            _ => 5_000.0,       // $5K for alts
        };

        let price_up = price_trend > price_threshold;
        let price_down = price_trend < -price_threshold;
        let cvd_up = cvd_trend > cvd_threshold;
        let cvd_down = cvd_trend < -cvd_threshold;

        match (price_up, price_down, cvd_up, cvd_down) {
            (false, true, true, false) => DivergenceSignal::Bullish,
            (true, false, false, true) => DivergenceSignal::Bearish,
            (true, false, true, false) => DivergenceSignal::Aligned,
            (false, true, false, true) => DivergenceSignal::Aligned,
            (false, false, false, false) => DivergenceSignal::Neutral,
            _ => DivergenceSignal::Neutral,
        }
    }

    /// Short-term (15s) price/CVD divergence for scalper
    /// Uses real tick data from last 15 seconds to detect early divergence signals
    fn cvd_divergence_15s(&self) -> DivergenceSignal {
        let now = Utc::now();
        let cutoff = now - ChronoDuration::seconds(15);

        // Get price change over 15s from price_history
        let prices_15s: Vec<f64> = self
            .price_history
            .iter()
            .filter(|(t, _)| *t >= cutoff)
            .map(|(_, p)| *p)
            .collect();

        if prices_15s.len() < 2 {
            return DivergenceSignal::Unknown;
        }

        let price_start = *prices_15s.first().unwrap();
        let price_end = *prices_15s.last().unwrap();
        let price_change = price_end - price_start;

        // Get CVD change over 15s (already computed as cvd_15s in caller)
        let cvd_15s = self.cvd_total(15);

        // Tighter thresholds for 15s window (more sensitive for scalping)
        let price_threshold = price_end * 0.0003; // 0.03% price move in 15s
        let cvd_threshold = match self.ticker.as_str() {
            "BTC" => 10_000.0,  // $10K CVD for BTC in 15s
            "ETH" => 5_000.0,   // $5K for ETH
            _ => 2_000.0,       // $2K for alts
        };

        let price_up = price_change > price_threshold;
        let price_down = price_change < -price_threshold;
        let cvd_up = cvd_15s > cvd_threshold;
        let cvd_down = cvd_15s < -cvd_threshold;

        match (price_up, price_down, cvd_up, cvd_down) {
            // Bullish divergence: price down but buying pressure (CVD up)
            (false, true, true, false) => DivergenceSignal::Bullish,
            // Bearish divergence: price up but selling pressure (CVD down)
            (true, false, false, true) => DivergenceSignal::Bearish,
            // Aligned: price and CVD moving together
            (true, false, true, false) => DivergenceSignal::Aligned,
            (false, true, false, true) => DivergenceSignal::Aligned,
            // Neutral: no significant movement
            (false, false, false, false) => DivergenceSignal::Neutral,
            _ => DivergenceSignal::Neutral,
        }
    }

    /// P1: OI velocity calculation - returns (delta over window, velocity in $/sec)
    ///
    /// This function guards against the "bootstrap effect" where exchanges report their
    /// entire OI at different times during startup, causing false large deltas.
    fn oi_velocity(&self, window_secs: i64) -> (f64, f64) {
        if self.oi_history_by_exchange.is_empty() {
            return (0.0, 0.0);
        }

        let cutoff = Utc::now() - ChronoDuration::seconds(window_secs);

        // Sum per-exchange deltas so joins/leaves don't produce a fake spike
        let mut total_delta = 0.0;
        let mut total_time = 0.0;

        for deque in self.oi_history_by_exchange.values() {
            if deque.len() < 2 {
                continue;
            }
            // Find first and last within window
            let first = deque.iter().find(|r| r.time >= cutoff);
            let last = deque.back();
            if let (Some(f), Some(l)) = (first, last) {
                if l.time <= f.time {
                    continue;
                }
                let delta = l.total - f.total;
                let span = (l.time - f.time).num_seconds().max(1) as f64;
                total_delta += delta;
                total_time += span;
            }
        }

        if total_time == 0.0 {
            (0.0, 0.0)
        } else {
            (total_delta, total_delta / total_time)
        }
    }

    /// Per-exchange OI delta calculation
    /// Returns (5m deltas per exchange, 15m deltas per exchange)
    /// Note: Currently returns current OI values as we don't track per-exchange history yet
    fn oi_delta_per_exchange(&self) -> (HashMap<String, f64>, HashMap<String, f64>) {
        // Calculate total OI for percentage calculation
        let total_oi: f64 = self.oi_by_exchange.values().copied().sum();

        // For now, we use the current OI values
        // TODO: Track per-exchange OI history for true delta calculation
        let mut delta_5m: HashMap<String, f64> = HashMap::new();
        let mut delta_15m: HashMap<String, f64> = HashMap::new();

        // Get the overall delta and distribute proportionally
        let (total_delta_5m, _) = self.oi_velocity(300);
        let (total_delta_15m, _) = self.oi_velocity(900);

        for (exchange, &oi) in &self.oi_by_exchange {
            let share = if total_oi > 0.0 { oi / total_oi } else { 0.0 };
            // Abbreviate exchange name
            let abbrev = abbreviate_exchange(exchange);
            delta_5m.insert(abbrev.to_string(), total_delta_5m * share);
            delta_15m.insert(abbrev.to_string(), total_delta_15m * share);
        }

        (delta_5m, delta_15m)
    }

    /// P1: Flow signal detection based on price/CVD relationship
    fn detect_flow_signal(&self) -> FlowSignal {
        if self.price_history.len() < 10 || self.cvd_history.len() < 10 {
            return FlowSignal::Neutral;
        }

        // Use 1-minute window for flow signal
        let now = Utc::now();
        let cutoff_1m = now - ChronoDuration::seconds(60);

        // Calculate price change over 1m
        let recent_prices: Vec<f64> = self
            .price_history
            .iter()
            .rev()
            .take_while(|(t, _)| *t >= cutoff_1m)
            .map(|(_, p)| *p)
            .collect();

        if recent_prices.len() < 2 {
            return FlowSignal::Neutral;
        }

        let price_start = *recent_prices.last().unwrap_or(&0.0);
        let price_end = *recent_prices.first().unwrap_or(&0.0);
        let price_pct_change = if price_start > 0.0 {
            ((price_end - price_start) / price_start) * 100.0
        } else {
            0.0
        };

        // Calculate CVD change over 1m
        let cvd_1m = self.cvd_total(60);

        // Thresholds for signal detection (tightened from original)
        // P0: Use 48-52% band (±2% from neutral) instead of 45-55%
        let price_flat_threshold = 0.02; // ±0.02% considered flat
        let cvd_significant = match self.ticker.as_str() {
            "BTC" => 100_000.0,  // $100K CVD significant for BTC
            "ETH" => 50_000.0,   // $50K for ETH
            _ => 10_000.0,       // $10K for alts
        };

        let price_flat = price_pct_change.abs() < price_flat_threshold;
        let price_up = price_pct_change > price_flat_threshold;
        let price_down = price_pct_change < -price_flat_threshold;
        let cvd_up = cvd_1m > cvd_significant;
        let cvd_down = cvd_1m < -cvd_significant;
        let cvd_flat = cvd_1m.abs() < cvd_significant;

        match (price_flat, price_up, price_down, cvd_up, cvd_down, cvd_flat) {
            // Accumulation: price flat but CVD rising (buying absorbed)
            (true, _, _, true, _, _) => FlowSignal::Accumulation,
            // Distribution: price up but CVD falling (smart money selling into strength)
            (_, true, _, _, true, _) => FlowSignal::Distribution,
            // Exhaustion: price moving but CVD flat (momentum fading)
            (_, true, _, _, _, true) => FlowSignal::Exhaustion,
            (_, _, true, _, _, true) => FlowSignal::Exhaustion,
            // Confirmation: price and CVD aligned
            (_, true, _, true, _, _) => FlowSignal::Confirmation,
            (_, _, true, _, true, _) => FlowSignal::Confirmation,
            _ => FlowSignal::Neutral,
        }
    }

    /// P1: Basis momentum calculation with trend detection
    /// Uses higher thresholds and requires consistent direction to avoid flickering
    fn basis_momentum(&self) -> Option<BasisMomentum> {
        if self.basis_history.len() < 10 {
            return None;
        }

        let now = Utc::now();
        let cutoff_1m = now - ChronoDuration::seconds(60);
        let cutoff_5m = now - ChronoDuration::seconds(300);

        // Get current basis (average of last 5 to smooth)
        let recent: Vec<f64> = self
            .basis_history
            .iter()
            .rev()
            .take(5)
            .map(|r| r.basis_usd)
            .collect();
        let current_avg = if recent.is_empty() {
            return None;
        } else {
            recent.iter().sum::<f64>() / recent.len() as f64
        };

        // Find basis 1m ago (also average a few samples)
        let samples_1m: Vec<f64> = self
            .basis_history
            .iter()
            .rev()
            .filter(|r| r.time <= cutoff_1m && r.time > cutoff_1m - ChronoDuration::seconds(10))
            .take(5)
            .map(|r| r.basis_usd)
            .collect();
        let basis_1m_ago = if samples_1m.is_empty() {
            current_avg
        } else {
            samples_1m.iter().sum::<f64>() / samples_1m.len() as f64
        };

        // Find basis 5m ago (also average)
        let samples_5m: Vec<f64> = self
            .basis_history
            .iter()
            .rev()
            .filter(|r| r.time <= cutoff_5m && r.time > cutoff_5m - ChronoDuration::seconds(30))
            .take(5)
            .map(|r| r.basis_usd)
            .collect();
        let basis_5m_ago = if samples_5m.is_empty() {
            current_avg
        } else {
            samples_5m.iter().sum::<f64>() / samples_5m.len() as f64
        };

        let delta_1m = current_avg - basis_1m_ago;
        let delta_5m = current_avg - basis_5m_ago;

        // Higher thresholds to avoid flickering - require significant moves
        let threshold = match self.ticker.as_str() {
            "BTC" => 50.0,   // $50 change significant for BTC (was $5)
            "ETH" => 5.0,    // $5 for ETH (was $1)
            _ => 0.5,        // $0.50 for alts (was $0.10)
        };

        // Signal threshold even higher - only show signal for strong moves
        let signal_threshold = threshold * 2.0;

        let trend = if delta_5m.abs() < threshold {
            BasisTrend::Stable
        } else if delta_5m > 0.0 {
            BasisTrend::Widening
        } else {
            BasisTrend::Narrowing
        };

        // Generate actionable signal only for strong, consistent moves
        // Require BOTH 1m and 5m to agree on direction
        let basis_state = self.basis().map(|b| b.state).unwrap_or(BasisState::Unknown);
        let strong_move = delta_5m.abs() > signal_threshold;
        let consistent = (delta_1m > 0.0 && delta_5m > 0.0) || (delta_1m < 0.0 && delta_5m < 0.0);

        let signal = if strong_move && consistent {
            match (&basis_state, &trend) {
                (BasisState::Contango, BasisTrend::Widening) => Some("Long risk".to_string()),
                (BasisState::Backwardation, BasisTrend::Narrowing) => Some("Squeeze setup".to_string()),
                (BasisState::Contango, BasisTrend::Narrowing) => Some("Longs unwinding".to_string()),
                (BasisState::Backwardation, BasisTrend::Widening) => Some("Shorts pressing".to_string()),
                _ => None,
            }
        } else {
            None
        };

        Some(BasisMomentum {
            delta_1m,
            delta_5m,
            trend,
            signal,
        })
    }
}

fn correlate(a: &VecDeque<(DateTime<Utc>, f64)>, b: &VecDeque<(DateTime<Utc>, f64)>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let n = a.len().min(b.len()).min(100);
    if n < 10 {
        return 0.0;
    }

    let a_slice: Vec<f64> = a.iter().rev().take(n).map(|(_, v)| *v).collect();
    let b_slice: Vec<f64> = b.iter().rev().take(n).map(|(_, v)| *v).collect();

    let mean_a = a_slice.iter().sum::<f64>() / n as f64;
    let mean_b = b_slice.iter().sum::<f64>() / n as f64;

    let mut num = 0.0;
    let mut denom_a = 0.0;
    let mut denom_b = 0.0;

    for i in 0..n {
        let da = a_slice[i] - mean_a;
        let db = b_slice[i] - mean_b;
        num += da * db;
        denom_a += da * da;
        denom_b += db * db;
    }

    let denom = (denom_a * denom_b).sqrt();
    if denom > 0.0 {
        (num / denom).clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// Abbreviate exchange name to 3-char code
fn abbreviate_exchange(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.contains("binance") {
        "BNC"
    } else if lower.contains("bybit") {
        "BBT"
    } else if lower.contains("okx") {
        "OKX"
    } else {
        "OTH"
    }
}
