//! Shared aggregation layer for all TUIs.
//!
//! Maintains rolling windows for trades, liquidations, OI, CVD, and orderbook data
//! so each TUI can render consistent metrics without duplicating calculations.

use crate::shared::types::{
    CvdData, LiquidationData, MarketEventMessage, OpenInterestData, OrderBookL1Data, Side,
    TradeData,
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rust_decimal::prelude::ToPrimitive;
use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;

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
    pub orderflow_1m: OrderflowStats,
    pub orderflow_5m: OrderflowStats,
    pub exchange_dominance: HashMap<String, f64>,
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
    pub cvd_1m_total: f64,
    pub cvd_per_exchange_5m: HashMap<String, f64>,
    pub trades_5m: usize,
    pub vol_5m: f64,
    pub avg_trade_usd_5m: f64,
    pub oi_total: f64,
    pub tick_direction: TickDirection,
    pub tick_direction_5m: TickDirection,
    pub trade_speed: f64,
    pub avg_trade_usd: f64,
    pub cvd_divergence: DivergenceSignal,
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
        let mut tickers_out = HashMap::new();
        for (ticker, state) in &self.tickers {
            tickers_out.insert(ticker.clone(), state.to_snapshot());
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
    oi_by_exchange: HashMap<String, f64>,
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
        }
    }

    fn push_trade(
        &mut self,
        trade: TradeData,
        exchange: &str,
        time: DateTime<Utc>,
        is_spot: bool,
        is_perp: bool,
    ) {
        let usd = trade.price * trade.amount;
        let side = trade.side.clone();
        let record = TradeRecord {
            time,
            side: side.clone(),
            price: trade.price,
            amount: trade.amount,
            exchange: exchange.to_string(),
            usd,
            is_spot,
            is_perp,
        };

        self.trades.push_back(record.clone());
        self.price_history.push_back((time, trade.price));
        self.exchange_volume
            .push_back((time, exchange.to_string(), usd));
        self.last_trade_by_exchange
            .insert(exchange.to_string(), trade.price);

        // Whale threshold (USD notional)
        if usd >= whale_threshold() {
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

        self.prune(time);
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
        self.oi_by_exchange.insert(exchange.to_string(), contracts);
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
            self.prune(time);
        }
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
        let orderflow_1m = self.orderflow(60);
        let orderflow_5m = self.orderflow(300);
        let exchange_dominance = self.exchange_dominance(60);
        let vwap_1m = self.vwap(60);
        let vwap_5m = self.vwap(300);
        // Per-exchange fairness: ensure all exchanges represented in whale display
        let whales: Vec<WhaleRecord> = self.fair_whale_selection(20);
        let (clusters, cascade_risk, next_level, protection_level) = self.liquidation_clusters();
        let liq_rate_per_min = self.liquidation_rate_per_min();
        let liq_bucket = self.liquidation_bucket_size();
        let cvd = self.cvd_summary();
        let cvd_1m_total = self.cvd_total(60);
        let oi_total: f64 = self.oi_by_exchange.values().copied().sum();
        let tick_direction = self.tick_direction(60);
        let tick_direction_5m = self.tick_direction(300);
        let (trade_speed, avg_trade_usd) = self.trade_speed(60);
        let (trades_5m, vol_5m, avg_5m) = self.trade_stats(300);
        let basis = self.basis();
        let cvd_divergence = self.cvd_divergence();
        let cvd_per_exchange_5m = self.cvd_per_exchange(300);

        TickerSnapshot {
            ticker: self.ticker.clone(),
            latest_price: self.latest_price(),
            latest_spread_pct: self.spread_pct,
            spot_mid: self.spot_mid,
            perp_mid: self.perp_mid,
            basis,
            orderflow_1m,
            orderflow_5m,
            exchange_dominance,
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
            cvd_1m_total,
            cvd_per_exchange_5m,
            oi_total,
            tick_direction,
            tick_direction_5m,
            trade_speed,
            avg_trade_usd,
            trades_5m,
            vol_5m,
            avg_trade_usd_5m: avg_5m,
            cvd_divergence,
            best_bid: self.best_bid,
            best_ask: self.best_ask,
            exchange_prices: self.last_trade_by_exchange.clone(),
        }
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

        // Thresholds
        let price_threshold = self.latest_price().unwrap_or(1.0) * 0.001; // 0.1%
        let cvd_threshold = 1000.0;

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
