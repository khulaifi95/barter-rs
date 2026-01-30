use barter_data::{
    error::DataError,
    event::{DataKind, MarketEvent, MarketEventEnvelope},
    snapshot::{MarketSnapshot, SnapshotPerExchangeShort, SnapshotTicker},
    streams::{builder::dynamic::DynamicStreams, consumer::MarketStreamResult, reconnect::Event},
    subscription::funding::FundingRate,
    subscription::open_interest::OpenInterest,
    books::OrderBook,
};
use barter_instrument::{
    exchange::ExchangeId,
    instrument::market_data::{MarketDataInstrument, kind::MarketDataInstrumentKind},
    Side,
};
use barter_trading_tuis::shared::{
    audit::AuditLogger,
    config::Config,
    market_state::{ConfigProvider, OptionContract, OptionsChain, Signal, TradMarketStatus, VolRegime, VolatilityEngine},
    options_state::{OptionsContext, OptionsContextBuilder},
    orchestrator::{OrchestratorResult, StateOrchestrator},
    snapshot_bridge::build_market_data_input,
    state::{Aggregator, AggregatedSnapshot, Candle1m, CandleBackfill, fetch_binance_1m_candles, ticker_to_binance_symbol},
    types::{InstrumentInfo, MarketEventMessage},
    vol_regime::VolRegimeEngine,
};
use chrono::{DateTime, TimeZone, Utc, Duration as ChronoDuration};
use futures::{SinkExt, StreamExt, stream};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use rust_decimal::prelude::ToPrimitive;
use rustls::crypto::ring::default_provider;
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, atomic::{AtomicI64, AtomicU64, Ordering}};
use std::time::Instant;
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::{broadcast, mpsc, watch},
    time::{interval, Duration},
};
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tokio_tungstenite::{accept_async, connect_async, tungstenite::Message};
use bytes::Bytes;
use tracing::{debug, error, info, warn};

// Parquet/Nautilus integration modules
mod aggregator;
mod health;
mod parquet;
mod storage;

use parquet::writer::{ParquetConfig, ParquetEvent, TradeEvent, ExtendedBarEvent, run_parquet_writer_task};
use health::heartbeat::{HeartbeatConfig, HeartbeatState, run_heartbeat_task, update_heartbeat_bar};
use aggregator::minute_bar::MinuteBarAggregator;
use aggregator::extended_bar::ExtendedBarBuilder;
use aggregator::compute_depth_bands;

// Parquet channel drop tracking
static PARQUET_DROPS: AtomicU64 = AtomicU64::new(0);

// L2 throttling per exchange (OKX is noisier, needs higher throttle)
const L2_THROTTLE_BINANCE_MS: u64 = 100;

// Metrics: trade throughput and timestamp skew tracking
// Skew = time_received - time_exchange (positive = server behind, negative = exchange ahead)
static TRADE_COUNT: AtomicU64 = AtomicU64::new(0);
static SKEW_SUM_MS: AtomicI64 = AtomicI64::new(0);
static SKEW_MAX_MS: AtomicI64 = AtomicI64::new(0);
static SKEW_MIN_MS: AtomicI64 = AtomicI64::new(i64::MAX);
static SKEW_COUNT: AtomicU64 = AtomicU64::new(0);

// Per-feed health metrics
static BINANCE_EVENT_COUNT: AtomicU64 = AtomicU64::new(0);
static OKX_EVENT_COUNT: AtomicU64 = AtomicU64::new(0);
static BYBIT_EVENT_COUNT: AtomicU64 = AtomicU64::new(0);
static IBKR_EVENT_COUNT: AtomicU64 = AtomicU64::new(0);
static AGG_CHANNEL_DROPPED: AtomicU64 = AtomicU64::new(0);
static IPC_LAGGED_FRAMES: AtomicU64 = AtomicU64::new(0);
static AGG_LAST_DROP_WARN_MS: AtomicI64 = AtomicI64::new(0);
static BINANCE_LAST_EVENT_MS: AtomicI64 = AtomicI64::new(0);
static OKX_LAST_EVENT_MS: AtomicI64 = AtomicI64::new(0);
static BYBIT_LAST_EVENT_MS: AtomicI64 = AtomicI64::new(0);
static IBKR_LAST_EVENT_MS: AtomicI64 = AtomicI64::new(0);

// Stale threshold for feed health alerts (ms)
const FEED_STALE_THRESHOLD_MS: i64 = 30_000; // 30 seconds

const L2_THROTTLE_BYBIT_MS: u64 = 100;
const L2_THROTTLE_OKX_MS: u64 = 150;
const SNAPSHOT_VERSION: u16 = 2;

/// Map ExchangeId to Nautilus-compatible venue name.
/// Nautilus expects simple venue names like "BINANCE", "OKX", "BYBIT".
fn exchange_to_venue(exchange: &barter_instrument::exchange::ExchangeId) -> &'static str {
    use barter_instrument::exchange::ExchangeId;
    match exchange {
        ExchangeId::BinanceFuturesUsd | ExchangeId::BinanceFuturesCoin |
        ExchangeId::BinanceSpot | ExchangeId::BinanceOptions |
        ExchangeId::BinancePortfolioMargin | ExchangeId::BinanceUs => "BINANCE",
        ExchangeId::Okx => "OKX",
        ExchangeId::BybitPerpetualsUsd | ExchangeId::BybitSpot => "BYBIT",
        ExchangeId::Coinbase | ExchangeId::CoinbaseInternational => "COINBASE",
        ExchangeId::Kraken => "KRAKEN",
        ExchangeId::GateioSpot | ExchangeId::GateioFuturesUsd | ExchangeId::GateioFuturesBtc |
        ExchangeId::GateioPerpetualsUsd | ExchangeId::GateioPerpetualsBtc |
        ExchangeId::GateioOptions => "GATEIO",
        ExchangeId::Htx => "HTX",
        ExchangeId::Kucoin => "KUCOIN",
        ExchangeId::Bitfinex => "BITFINEX",
        ExchangeId::Bitmex => "BITMEX",
        ExchangeId::Deribit => "DERIBIT",
        ExchangeId::Poloniex => "POLONIEX",
        ExchangeId::Bitget => "BITGET",
        ExchangeId::Gemini => "GEMINI",
        ExchangeId::Bitstamp => "BITSTAMP",
        ExchangeId::Bitmart | ExchangeId::BitmartFuturesUsd => "BITMART",
        // Default for unknown/simulated exchanges
        _ => "OTHER",
    }
}

/// OKX perpetuals report sizes in contracts. Convert to base units using ctVal.

/// Serialization config for broadcast messages (read once at startup)
/// Also caches hot-path config values to avoid env var parsing per-event
struct BroadcastConfig {
    use_envelope: bool,
    source: String,
    /// Use binary WS frames (true) or text frames (false)
    /// Binary is faster (no UTF-8 conversion) but non-TUI clients may expect text
    use_binary_frames: bool,
    /// Cached L1 throttle interval (avoids env var parsing in hot path)
    l1_throttle_ms: u64,
    /// Cached spot log threshold (avoids env var parsing in hot path)
    spot_log_threshold: f64,
}

/// Per-instrument precision configuration for Parquet + bars.
struct PrecisionConfig {
    default_price: u8,
    default_size: u8,
    map: HashMap<String, (u8, u8)>,
}

impl PrecisionConfig {
    fn from_env() -> Self {
        let default_price = std::env::var("PRICE_PRECISION_DEFAULT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2);
        let default_size = std::env::var("SIZE_PRECISION_DEFAULT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);

        let mut map = HashMap::new();
        if let Ok(raw) = std::env::var("PRECISION_MAP") {
            for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
                let Some((instrument_id, prec)) = entry.split_once('=') else { continue };
                let Some((p_str, s_str)) = prec.split_once(':') else { continue };
                if let (Ok(p), Ok(s)) = (p_str.parse::<u8>(), s_str.parse::<u8>()) {
                    map.insert(instrument_id.to_string(), (p, s));
                }
            }
        }

        Self { default_price, default_size, map }
    }

    fn get(&self, instrument_id: &str) -> (u8, u8) {
        self.map
            .get(instrument_id)
            .copied()
            .unwrap_or((self.default_price, self.default_size))
    }
}

impl BroadcastConfig {
    fn from_env() -> Self {
        let use_envelope = std::env::var("WS_ENVELOPE")
            .ok()
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE"))
            .unwrap_or(false);
        let source = std::env::var("WS_SOURCE")
            .unwrap_or_else(|_| "barter-data-server".to_string());
        // Default to binary frames for performance; set WS_BINARY_FRAMES=0 for text
        let use_binary_frames = std::env::var("WS_BINARY_FRAMES")
            .ok()
            .map(|v| !matches!(v.as_str(), "0" | "false" | "FALSE"))
            .unwrap_or(true);
        // Cache hot-path config values (avoid env var parsing per-event)
        let l1_throttle_ms = std::env::var("L1_THROTTLE_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(50); // ~20 updates/sec per instrument
        let spot_log_threshold = std::env::var("SPOT_LOG_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(50_000.0);
        Self { use_envelope, source, use_binary_frames, l1_throttle_ms, spot_log_threshold }
    }
}

#[derive(Debug, Clone)]
struct UdsConfig {
    enabled: bool,
    path: String,
    buffer: usize,
}

impl UdsConfig {
    fn from_env() -> Self {
        let default_enabled = cfg!(unix);
        let enabled = std::env::var("UDS_ENABLED")
            .ok()
            .map(|v| !matches!(v.as_str(), "0" | "false" | "FALSE"))
            .unwrap_or(default_enabled);
        let path = std::env::var("UDS_PATH")
            .unwrap_or_else(|_| "/tmp/barter-data.sock".to_string());
        let buffer = std::env::var("UDS_BUFFER")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100_000_usize)
            .clamp(1_000, 500_000);
        Self { enabled, path, buffer }
    }
}

#[derive(Debug, Clone)]
struct TcpConfig {
    enabled: bool,
    addr: String,
    buffer: usize,
}

impl TcpConfig {
    fn from_env() -> Self {
        let default_enabled = !cfg!(unix);
        let enabled = std::env::var("TCP_ENABLED")
            .ok()
            .map(|v| !matches!(v.as_str(), "0" | "false" | "FALSE"))
            .unwrap_or(default_enabled);
        let addr = std::env::var("TCP_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:9102".to_string());
        let buffer = std::env::var("TCP_BUFFER")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100_000_usize)
            .clamp(1_000, 500_000);
        Self { enabled, addr, buffer }
    }
}

#[derive(Serialize)]
enum UdsMessageRef<'a> {
    Event(&'a MarketEventMessage),
}

/// Pre-serialize a message for broadcast (avoids per-client serialization)
/// Returns None on serialization failure (logs error, drops message)
/// Uses Bytes for zero-copy sharing across clients
fn serialize_for_broadcast(config: &BroadcastConfig, event: MarketEventMessage) -> Option<Bytes> {
    let result = if config.use_envelope {
        let wrapped = MarketEventEnvelope {
            schema_version: 1,
            source: config.source.clone(),
            time_sent: Utc::now(),
            payload: event,
        };
        serde_json::to_string(&wrapped)
    } else {
        serde_json::to_string(&event)
    };

    match result {
        Ok(json) => Some(Bytes::from(json)),
        Err(e) => {
            error!("Failed to serialize market event: {}", e);
            None
        }
    }
}

fn serialize_for_uds(event: &MarketEventMessage) -> Option<Bytes> {
    let payload = match rmp_serde::to_vec(&UdsMessageRef::Event(event)) {
        Ok(bytes) => bytes,
        Err(e) => {
            error!("Failed to serialize UDS message: {}", e);
            return None;
        }
    };
    if payload.len() > u32::MAX as usize {
        error!("UDS payload too large: {} bytes", payload.len());
        return None;
    }
    let mut framed = Vec::with_capacity(4 + payload.len());
    framed.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    framed.extend_from_slice(&payload);
    Some(Bytes::from(framed))
}

/// Get L2 throttle from ExchangeId directly (avoids hot-path String allocation)
fn get_l2_throttle_ms_for_exchange(exchange: &ExchangeId) -> u64 {
    use ExchangeId::*;
    match exchange {
        Okx => std::env::var("L2_THROTTLE_OKX_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(L2_THROTTLE_OKX_MS),
        BybitSpot | BybitPerpetualsUsd => std::env::var("L2_THROTTLE_BYBIT_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(L2_THROTTLE_BYBIT_MS),
        _ => std::env::var("L2_THROTTLE_BINANCE_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(L2_THROTTLE_BINANCE_MS),
    }
}

fn trad_tick_event(tick: TradMarketTick) -> MarketEventMessage {
    let exchange_time = chrono::Utc.timestamp_millis_opt(tick.ts).single().unwrap_or_else(Utc::now);
    MarketEventMessage {
        time_exchange: exchange_time,
        time_received: Utc::now(),
        exchange: "Ibkr".to_string(),
        instrument: InstrumentInfo {
            base: tick.symbol.clone(),
            quote: "USD".to_string(),
            kind: "Index".to_string(),
        },
        kind: "trad_tick".to_string(),
        data: serde_json::to_value(tick).unwrap_or_default(),
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct TradMarketTick {
    symbol: String,
    ts: i64,
    px: f64,
    #[serde(default)]
    sz: f64,
    #[serde(default)]
    bid: Option<f64>,
    #[serde(default)]
    ask: Option<f64>,
    #[serde(default)]
    vwap: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
struct OrchestratorMessage {
    ticker: String,
    result: OrchestratorResult,
}

struct VolRegimeState {
    engine: VolRegimeEngine,
    last_hour: Option<i64>,
}

struct SnapshotBuilder {
    tickers: HashMap<String, TickerDerivedState>,
}

impl SnapshotBuilder {
    fn new() -> Self {
        Self { tickers: HashMap::new() }
    }

    fn update(&mut self, event: &MarketEvent<MarketDataInstrument, DataKind>) {
        let ticker = event.instrument.base.to_string().to_uppercase();
        let state = self
            .tickers
            .entry(ticker)
            .or_insert_with(TickerDerivedState::new);
        state.update(event);
    }

    fn snapshot(&mut self) -> MarketSnapshot {
        let now = Utc::now();
        let mut tickers = HashMap::new();
        for (symbol, state) in self.tickers.iter_mut() {
            tickers.insert(symbol.clone(), state.snapshot(now));
        }
        MarketSnapshot {
            snapshot_version: SNAPSHOT_VERSION,
            timestamp: now.timestamp_millis(),
            tickers,
        }
    }
}

struct TickerDerivedState {
    last_price: f64,
    last_price_ts: DateTime<Utc>,
    trade_volumes: VecDeque<(DateTime<Utc>, f64)>,
    trade_flows_by_exchange: HashMap<String, VecDeque<(DateTime<Utc>, f64, f64)>>,
    cvd_by_exchange: HashMap<String, VecDeque<(DateTime<Utc>, f64)>>,
    oi_by_exchange: HashMap<String, VecDeque<(DateTime<Utc>, f64)>>,
    funding_by_exchange: HashMap<String, VecDeque<(DateTime<Utc>, f64)>>,
    liq_notional: VecDeque<(DateTime<Utc>, f64)>,
}

impl TickerDerivedState {
    fn new() -> Self {
        Self {
            last_price: 0.0,
            last_price_ts: Utc::now(),
            trade_volumes: VecDeque::new(),
            trade_flows_by_exchange: HashMap::new(),
            cvd_by_exchange: HashMap::new(),
            oi_by_exchange: HashMap::new(),
            funding_by_exchange: HashMap::new(),
            liq_notional: VecDeque::new(),
        }
    }

    fn update(&mut self, event: &MarketEvent<MarketDataInstrument, DataKind>) {
        let now = event.time_exchange;
        match &event.kind {
            DataKind::Trade(trade) => {
                self.last_price = trade.price;
                self.last_price_ts = now;
                let notional = trade.price * trade.amount;
                self.trade_volumes.push_back((now, notional));
                let exchange = format!("{:?}", event.exchange);
                let signed = if trade.side == Side::Buy { notional } else { -notional };
                self.trade_flows_by_exchange
                    .entry(exchange)
                    .or_insert_with(VecDeque::new)
                    .push_back((now, signed, notional));
                self.prune_trade_volumes(now);
                self.prune_trade_flows(now);
            }
            DataKind::CumulativeVolumeDelta(cvd) => {
                let entry = self
                    .cvd_by_exchange
                    .entry(format!("{:?}", event.exchange))
                    .or_insert_with(VecDeque::new);
                entry.push_back((now, cvd.delta_quote));
                self.prune_cvd(now);
            }
            DataKind::OpenInterest(oi) => {
                let value = oi.notional.unwrap_or(oi.contracts);
                let entry = self
                    .oi_by_exchange
                    .entry(format!("{:?}", event.exchange))
                    .or_insert_with(VecDeque::new);
                entry.push_back((now, value));
                self.prune_oi(now);
            }
            DataKind::FundingRate(fr) => {
                let entry = self
                    .funding_by_exchange
                    .entry(format!("{:?}", event.exchange))
                    .or_insert_with(VecDeque::new);
                entry.push_back((now, fr.rate));
                self.prune_funding(now);
            }
            DataKind::Liquidation(liq) => {
                let notional = liq.price * liq.quantity;
                self.liq_notional.push_back((now, notional));
                self.prune_liquidations(now);
            }
            _ => {}
        }
    }

    fn snapshot(&mut self, now: DateTime<Utc>) -> SnapshotTicker {
        self.prune_trade_volumes(now);
        self.prune_trade_flows(now);
        self.prune_cvd(now);
        self.prune_oi(now);
        self.prune_funding(now);
        self.prune_liquidations(now);

        let vol_5m = self.sum_window(&self.trade_volumes, now, 300);
        let vol_1h = self.sum_window(&self.trade_volumes, now, 3600);
        let rvol_5m = if vol_1h > 0.0 {
            vol_5m / (vol_1h / 12.0)
        } else {
            0.0
        };

        let cvd_5m = self.cvd_delta(now, 300);
        let cvd_15m = self.cvd_delta(now, 900);

        let oi_delta_5m = self.oi_delta(now, 300);
        let funding_rate = self.funding_latest();
        let funding_velocity = self.funding_velocity(now, 900);
        let liq_rate_usd_per_min = self.sum_window(&self.liq_notional, now, 60);
        let per_exchange_30s = self.per_exchange_short_stats(now, 30);

        SnapshotTicker {
            price: self.last_price,
            cvd_5m,
            cvd_15m,
            rvol_5m,
            oi_delta_5m,
            funding_rate,
            funding_velocity,
            liq_rate_usd_per_min,
            vol_percentile: 0.0,
            vol_regime: "unknown".to_string(),
            vol_samples: 0,
            per_exchange_30s,
        }
    }

    fn sum_window(&self, data: &VecDeque<(DateTime<Utc>, f64)>, now: DateTime<Utc>, window_secs: i64) -> f64 {
        data.iter()
            .filter(|(ts, _)| (now - *ts).num_seconds() <= window_secs)
            .map(|(_, v)| *v)
            .sum()
    }

    fn cvd_delta(&self, now: DateTime<Utc>, window_secs: i64) -> f64 {
        let mut total = 0.0;
        for points in self.cvd_by_exchange.values() {
            if points.is_empty() {
                continue;
            }
            let latest = points.back().map(|v| v.1).unwrap_or(0.0);
            let earliest = points
                .iter()
                .find(|(ts, _)| (now - *ts).num_seconds() <= window_secs)
                .map(|v| v.1)
                .unwrap_or_else(|| points.front().map(|v| v.1).unwrap_or(latest));
            total += latest - earliest;
        }
        total
    }

    fn oi_delta(&self, now: DateTime<Utc>, window_secs: i64) -> f64 {
        let mut total = 0.0;
        for points in self.oi_by_exchange.values() {
            if points.is_empty() {
                continue;
            }
            let latest = points.back().map(|v| v.1).unwrap_or(0.0);
            let earliest = points
                .iter()
                .find(|(ts, _)| (now - *ts).num_seconds() <= window_secs)
                .map(|v| v.1)
                .unwrap_or_else(|| points.front().map(|v| v.1).unwrap_or(latest));
            total += latest - earliest;
        }
        total
    }

    fn funding_latest(&self) -> f64 {
        let mut total = 0.0;
        let mut count = 0.0;
        for points in self.funding_by_exchange.values() {
            if let Some((_, rate)) = points.back() {
                total += *rate;
                count += 1.0;
            }
        }
        if count > 0.0 { total / count } else { 0.0 }
    }

    fn funding_velocity(&self, now: DateTime<Utc>, window_secs: i64) -> f64 {
        let mut total = 0.0;
        let mut count = 0.0;
        for points in self.funding_by_exchange.values() {
            if points.is_empty() {
                continue;
            }
            let latest = points.back().map(|v| v.1).unwrap_or(0.0);
            let earliest = points
                .iter()
                .find(|(ts, _)| (now - *ts).num_seconds() <= window_secs)
                .map(|v| v.1)
                .unwrap_or_else(|| points.front().map(|v| v.1).unwrap_or(latest));
            total += latest - earliest;
            count += 1.0;
        }
        if count > 0.0 { total / count } else { 0.0 }
    }

    fn per_exchange_short_stats(
        &self,
        now: DateTime<Utc>,
        window_secs: i64,
    ) -> HashMap<String, SnapshotPerExchangeShort> {
        let cutoff = now - ChronoDuration::seconds(window_secs);
        let mut out = HashMap::new();

        for (ex, trades) in &self.trade_flows_by_exchange {
            let mut signed = 0.0;
            let mut total = 0.0;
            let mut count = 0;

            for (ts, signed_usd, abs_usd) in trades.iter().rev() {
                if *ts < cutoff {
                    break;
                }
                signed += *signed_usd;
                total += *abs_usd;
                count += 1;
            }

            out.insert(
                ex.clone(),
                SnapshotPerExchangeShort {
                    cvd_30s: signed,
                    total_30s: total,
                    trades_30s: count,
                },
            );
        }

        out
    }

    fn prune_trade_volumes(&mut self, now: DateTime<Utc>) {
        Self::prune_queue(&mut self.trade_volumes, now, 3600);
    }

    fn prune_trade_flows(&mut self, now: DateTime<Utc>) {
        let cutoff = now - ChronoDuration::seconds(120);
        self.trade_flows_by_exchange.retain(|_, trades| {
            while let Some((ts, _, _)) = trades.front() {
                if *ts < cutoff {
                    trades.pop_front();
                } else {
                    break;
                }
            }
            !trades.is_empty()
        });
    }

    fn prune_cvd(&mut self, now: DateTime<Utc>) {
        for points in self.cvd_by_exchange.values_mut() {
            Self::prune_queue(points, now, 900);
        }
    }

    fn prune_oi(&mut self, now: DateTime<Utc>) {
        for points in self.oi_by_exchange.values_mut() {
            Self::prune_queue(points, now, 900);
        }
    }

    fn prune_funding(&mut self, now: DateTime<Utc>) {
        for points in self.funding_by_exchange.values_mut() {
            Self::prune_queue(points, now, 900);
        }
    }

    fn prune_liquidations(&mut self, now: DateTime<Utc>) {
        Self::prune_queue(&mut self.liq_notional, now, 60);
    }

    fn prune_queue(queue: &mut VecDeque<(DateTime<Utc>, f64)>, now: DateTime<Utc>, window_secs: i64) {
        while let Some((ts, _)) = queue.front() {
            if (now - *ts).num_seconds() > window_secs {
                queue.pop_front();
            } else {
                break;
            }
        }
    }
}

fn market_event_to_message(event: MarketEvent<MarketDataInstrument, DataKind>) -> MarketEventMessage {
    let (kind_name, data) = match &event.kind {
        DataKind::Trade(trade) => ("trade", serde_json::to_value(trade).unwrap_or_default()),
        DataKind::Liquidation(liq) => (
            "liquidation",
            serde_json::to_value(liq).unwrap_or_default(),
        ),
        DataKind::OpenInterest(oi) => (
            "open_interest",
            serde_json::to_value(oi).unwrap_or_default(),
        ),
        DataKind::FundingRate(fr) => (
            "funding_rate",
            serde_json::to_value(fr).unwrap_or_default(),
        ),
        DataKind::CumulativeVolumeDelta(cvd) => (
            "cumulative_volume_delta",
            serde_json::to_value(cvd).unwrap_or_default(),
        ),
        DataKind::OrderBookL1(ob) => (
            "order_book_l1",
            serde_json::to_value(ob).unwrap_or_default(),
        ),
        DataKind::OrderBook(ob_event) => (
            "order_book_l2",
            serde_json::to_value(ob_event).unwrap_or_default(),
        ),
        _ => ("other", serde_json::Value::Null),
    };

    MarketEventMessage {
        time_exchange: event.time_exchange,
        time_received: event.time_received,
        exchange: format!("{:?}", event.exchange),
        instrument: InstrumentInfo {
            base: event.instrument.base.to_string(),
            quote: event.instrument.quote.to_string(),
            kind: match event.instrument.kind {
                MarketDataInstrumentKind::Spot => "Spot".to_string(),
                MarketDataInstrumentKind::Perpetual => "Perpetual".to_string(),
                _ => format!("{:?}", event.instrument.kind),
            },
        },
        kind: kind_name.to_string(),
        data,
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum IbkrMessage {
    #[serde(rename = "tick")]
    Tick(TradMarketTick),
    #[serde(rename = "tick_backfill")]
    TickBackfill { #[allow(dead_code)] symbol: String, ticks: Vec<TradMarketTick> },
    #[serde(rename = "welcome")]
    Welcome { #[serde(default)] #[allow(dead_code)] message: Option<String> },
    #[serde(rename = "status")]
    Status { #[serde(default)] #[allow(dead_code)] connected: Option<bool> },
}

#[derive(Debug, Deserialize)]
struct DeribitResponse<T> {
    result: T,
}

#[derive(Debug, Deserialize)]
struct DeribitInstrument {
    instrument_name: String,
    strike: f64,
    expiration_timestamp: i64,
    option_type: String,
}

#[derive(Debug, Deserialize, Clone)]
struct DeribitBookSummary {
    instrument_name: String,
    open_interest: Option<f64>,
    mark_iv: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct DeribitGreeks {
    delta: f64,
    gamma: f64,
    vega: f64,
}

#[derive(Debug, Deserialize)]
struct DeribitTicker {
    instrument_name: String,
    #[allow(dead_code)]
    open_interest: Option<f64>,
    #[allow(dead_code)]
    mark_iv: Option<f64>,
    greeks: Option<DeribitGreeks>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
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

#[tokio::main]
async fn main() {
    // Initialize logging
    init_logging();

    // Install rustls crypto provider (required for TLS in reqwest/ws)
    if let Err(e) = default_provider().install_default() {
        warn!("Rustls crypto provider already installed or failed: {e:?}");
    }

    info!("Starting barter-data WebSocket server");

    // Separate channels for trades (hot path) and L2 (high volume, lower priority)
    // Buffer sizing: peak_msgs_per_sec × desired_burst_seconds
    // At 2-5k msgs/sec, 100k gives ~20-50s headroom for trade bursts
    let trades_buffer = std::env::var("WS_TRADES_BUFFER")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100_000_usize)
        .clamp(1_000, 500_000); // Prevent OOM from misconfiguration
    let l2_buffer = std::env::var("WS_L2_BUFFER")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50_000_usize)
        .clamp(1_000, 500_000); // Prevent OOM from misconfiguration

    info!(
        "Trade channel buffer: {}, L2 channel buffer: {}",
        trades_buffer, l2_buffer
    );

    // Broadcast config (read once at startup to avoid per-client overhead)
    let broadcast_config = Arc::new(BroadcastConfig::from_env());
    info!(
        "Broadcast config: envelope={}, source={}",
        broadcast_config.use_envelope, broadcast_config.source
    );

    let uds_config = UdsConfig::from_env();
    let tcp_config = TcpConfig::from_env();
    info!(
        "UDS config: enabled={}, path={}, buffer={}",
        uds_config.enabled, uds_config.path, uds_config.buffer
    );
    info!(
        "TCP config: enabled={}, addr={}, buffer={}",
        tcp_config.enabled, tcp_config.addr, tcp_config.buffer
    );

    // Trades channel: trades, liquidations, OI, CVD, L1 (hot path - NO L2)
    // Now broadcasts pre-serialized Arc<String> to avoid per-client JSON serialization
    let (tx_trades, _) = broadcast::channel::<Bytes>(trades_buffer);
    let tx_trades = Arc::new(tx_trades);

    // L2 channel: orderbook L2 only (high volume, can lag without affecting trades)
    let (tx_l2, _) = broadcast::channel::<Bytes>(l2_buffer);
    let tx_l2 = Arc::new(tx_l2);

    let ipc_buffer = std::cmp::max(uds_config.buffer, tcp_config.buffer);
    let (tx_uds, _) = broadcast::channel::<Bytes>(ipc_buffer);
    let tx_uds = Arc::new(tx_uds);

    let snapshot_builder = Arc::new(tokio::sync::Mutex::new(SnapshotBuilder::new()));

    // Aggregator channels: removes hot-path mutex contention
    // - agg_event_tx: send events to aggregator task (mpsc, bounded)
    // - agg_snapshot_rx: receive snapshots from aggregator (watch, latest-value)
    let agg_buffer = std::env::var("AGG_EVENT_BUFFER")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300_000_usize)
        .clamp(1_000, 500_000);
    info!("Aggregator buffer size: {}", agg_buffer);
    let (agg_event_tx, agg_event_rx) = mpsc::channel::<MarketEventMessage>(agg_buffer);
    let (agg_snapshot_tx, agg_snapshot_rx) = watch::channel(AggregatedSnapshot::default());
    let agg_event_tx = Arc::new(agg_event_tx);
    let agg_snapshot_rx = Arc::new(agg_snapshot_rx);

    let vol_states = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let spot_cache = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let options_cache = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let trad_last_ms = Arc::new(AtomicI64::new(0));
    let kline_cache = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    // Spawn dedicated aggregator task - removes lock().await from hot path
    tokio::spawn(async move {
        run_aggregator_task(agg_event_rx, agg_snapshot_tx).await;
    });

    // Parquet writer channel (mpsc for backpressure)
    // Disabled by default - enable with PARQUET_ENABLED=1
    let parquet_enabled = std::env::var("PARQUET_ENABLED")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);
    let parquet_buffer = std::env::var("PARQUET_BUFFER")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50_000_usize);

    let (parquet_tx, parquet_rx) = mpsc::channel::<ParquetEvent>(parquet_buffer);
    let parquet_tx = Arc::new(parquet_tx);
    let precision_config = Arc::new(PrecisionConfig::from_env());

    // MinuteBarAggregator for building 1m bars from trades
    // Default precision: price=2 (e.g., $100,000.00), size=3 (e.g., 0.001 BTC)
    let bar_aggregator = Arc::new(tokio::sync::Mutex::new(MinuteBarAggregator::new(2, 3)));

    // ExtendedBarBuilder per instrument for CVD tracking and extended metrics
    let extended_bar_builders: Arc<tokio::sync::Mutex<HashMap<String, ExtendedBarBuilder>>> =
        Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    // L2 order book cache per instrument (for depth bands at bar close)
    let l2_book_cache: Arc<tokio::sync::Mutex<HashMap<String, OrderBook>>> =
        Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    if parquet_enabled {
        let config = ParquetConfig::from_env();
        info!(
            "Parquet writer enabled: output_dir={:?}, flush_interval={}s, buffer={}",
            config.output_dir, config.flush_interval_secs, parquet_buffer
        );
        tokio::spawn(async move {
            run_parquet_writer_task(parquet_rx, config).await;
        });
    } else {
        info!("Parquet writer disabled (set PARQUET_ENABLED=1 to enable)");
        // Drop the receiver so the channel closes immediately
        drop(parquet_rx);
    }

    // Heartbeat task for health monitoring
    // Disabled by default - enable with HEARTBEAT_ENABLED=1
    let heartbeat_enabled = std::env::var("HEARTBEAT_ENABLED")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);

    // Counters for heartbeat (using Arc since run_heartbeat_task needs ownership)
    let heartbeat_bars_written = Arc::new(AtomicU64::new(0));
    let heartbeat_trades_processed = Arc::new(AtomicU64::new(0));

    // Heartbeat state handle for updating symbols/last_bars
    let heartbeat_state: Option<Arc<tokio::sync::RwLock<HeartbeatState>>> = if heartbeat_enabled {
        let config = HeartbeatConfig::from_env();
        info!(
            "Heartbeat enabled: file={:?}, interval={}s",
            config.file_path, config.interval_secs
        );
        let bars_counter = Arc::clone(&heartbeat_bars_written);
        let trades_counter = Arc::clone(&heartbeat_trades_processed);
        // run_heartbeat_task spawns its own task internally and returns state handle
        Some(run_heartbeat_task(config, bars_counter, trades_counter).await)
    } else {
        info!("Heartbeat disabled (set HEARTBEAT_ENABLED=1 to enable)");
        None
    };

    // Start WebSocket server
    // Configurable via WS_ADDR env var (default: 0.0.0.0:9001)
    let server_addr_str = std::env::var("WS_ADDR").unwrap_or_else(|_| "0.0.0.0:9001".to_string());
    let server_addr = server_addr_str
        .parse::<SocketAddr>()
        .unwrap_or_else(|_| "0.0.0.0:9001".parse().unwrap());
    let tx_trades_clone = tx_trades.clone();
    let tx_l2_clone = tx_l2.clone();
    let kline_cache_clone = Arc::clone(&kline_cache);
    let broadcast_cfg = Arc::clone(&broadcast_config);
    tokio::spawn(async move {
        start_websocket_server(
            server_addr,
            tx_trades_clone,
            tx_l2_clone,
            kline_cache_clone,
            broadcast_cfg,
        )
        .await;
    });

    #[cfg(unix)]
    if uds_config.enabled {
        let uds_path = uds_config.path.clone();
        let uds_tx = Arc::clone(&tx_uds);
        tokio::spawn(async move {
            start_uds_server(uds_path, uds_tx).await;
        });
    }
    #[cfg(not(unix))]
    if uds_config.enabled {
        warn!("UDS IPC requested but not supported on this platform; enable TCP IPC instead.");
    }
    if tcp_config.enabled {
        let tcp_addr = tcp_config.addr.clone();
        let tcp_tx = Arc::clone(&tx_uds);
        tokio::spawn(async move {
            start_tcp_server(tcp_addr, tcp_tx).await;
        });
    }

    // Metrics logging task: trades/sec, timestamp skew, and per-feed health every 60s
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(60));
        interval.tick().await; // Skip first immediate tick
        loop {
            interval.tick().await;
            let now_ms = Utc::now().timestamp_millis();

            // Trade metrics
            let trades = TRADE_COUNT.swap(0, Ordering::Relaxed);
            let skew_sum = SKEW_SUM_MS.swap(0, Ordering::Relaxed);
            let skew_max = SKEW_MAX_MS.swap(0, Ordering::Relaxed);
            let skew_min = SKEW_MIN_MS.swap(i64::MAX, Ordering::Relaxed);
            let skew_count = SKEW_COUNT.swap(0, Ordering::Relaxed);
            let skew_avg = if skew_count > 0 { skew_sum / skew_count as i64 } else { 0 };
            let skew_min_display = if skew_min == i64::MAX { 0 } else { skew_min };

            // Per-feed event counts (reset each interval)
            let binance_events = BINANCE_EVENT_COUNT.swap(0, Ordering::Relaxed);
            let okx_events = OKX_EVENT_COUNT.swap(0, Ordering::Relaxed);
            let bybit_events = BYBIT_EVENT_COUNT.swap(0, Ordering::Relaxed);
            let ibkr_events = IBKR_EVENT_COUNT.swap(0, Ordering::Relaxed);
            let agg_dropped = AGG_CHANNEL_DROPPED.swap(0, Ordering::Relaxed);
            let ipc_lagged = IPC_LAGGED_FRAMES.swap(0, Ordering::Relaxed);

            // Per-feed staleness check
            let binance_last = BINANCE_LAST_EVENT_MS.load(Ordering::Relaxed);
            let okx_last = OKX_LAST_EVENT_MS.load(Ordering::Relaxed);
            let bybit_last = BYBIT_LAST_EVENT_MS.load(Ordering::Relaxed);
            let ibkr_last = IBKR_LAST_EVENT_MS.load(Ordering::Relaxed);

            info!(
                "METRICS: trades/min={} ({:.1}/s), skew_avg={}ms, skew_min={}ms, skew_max={}ms",
                trades,
                trades as f64 / 60.0,
                skew_avg,
                skew_min_display,
                skew_max
            );
            info!(
                "FEEDS: binance={}/min, okx={}/min, bybit={}/min, ibkr={}/min, agg_dropped={}, ipc_lagged={}",
                binance_events, okx_events, bybit_events, ibkr_events, agg_dropped, ipc_lagged
            );

            // Stale feed alerts
            if binance_last > 0 && (now_ms - binance_last) > FEED_STALE_THRESHOLD_MS {
                warn!("STALE: Binance feed stale for {}s", (now_ms - binance_last) / 1000);
            }
            if okx_last > 0 && (now_ms - okx_last) > FEED_STALE_THRESHOLD_MS {
                warn!("STALE: OKX feed stale for {}s", (now_ms - okx_last) / 1000);
            }
            if bybit_last > 0 && (now_ms - bybit_last) > FEED_STALE_THRESHOLD_MS {
                warn!("STALE: Bybit feed stale for {}s", (now_ms - bybit_last) / 1000);
            }
            if ibkr_last > 0 && (now_ms - ibkr_last) > FEED_STALE_THRESHOLD_MS {
                warn!("STALE: IBKR feed stale for {}s", (now_ms - ibkr_last) / 1000);
            }

            // Backpressure alert
            if agg_dropped > 0 {
                warn!("BACKPRESSURE: {} events dropped from aggregator channel", agg_dropped);
            }
        }
    });

    // IBKR bridge feed (ES/NQ) -> trad_tick events
    {
        let tx_trades = tx_trades.clone();
        let tx_uds = Arc::clone(&tx_uds);
        let broadcast_cfg = Arc::clone(&broadcast_config);
        let trad_last_ms = Arc::clone(&trad_last_ms);
        tokio::spawn(async move {
            start_ibkr_bridge_feed(tx_trades, tx_uds, broadcast_cfg, trad_last_ms).await;
        });
    }

    // Deribit options feed (options_chain events)
    {
        let tx_trades = tx_trades.clone();
        let tx_uds = Arc::clone(&tx_uds);
        let spot_cache = Arc::clone(&spot_cache);
        let options_cache = Arc::clone(&options_cache);
        let broadcast_cfg = Arc::clone(&broadcast_config);
        tokio::spawn(async move {
            start_deribit_options_feed(tx_trades, tx_uds, broadcast_cfg, spot_cache, options_cache).await;
        });
    }

    let tickers: Vec<String> = std::env::var("TICKERS")
        .unwrap_or_else(|_| "BTC,ETH,SOL".to_string())
        .split(',')
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
        .collect();

    // Seed volatility regime history (7-day hourly RV from 5m klines)
    {
        let vol_states = Arc::clone(&vol_states);
        seed_vol_regime(vol_states, &tickers).await;
    }

    // Binance kline feed (authoritative 1m candles for RV/ATR/tvVWAP)
    {
        let kline_cache = Arc::clone(&kline_cache);
        let tx_uds = Arc::clone(&tx_uds);
        let agg_tx = Arc::clone(&agg_event_tx);
        seed_binance_klines(kline_cache, tx_uds, agg_tx, &tickers).await;
    }
    for ticker in &tickers {
        let tx_trades = tx_trades.clone();
        let tx_uds = Arc::clone(&tx_uds);
        let kline_cache = Arc::clone(&kline_cache);
        let agg_tx = Arc::clone(&agg_event_tx);
        let broadcast_cfg = Arc::clone(&broadcast_config);
        let ticker = ticker.clone();
        tokio::spawn(async move {
            run_binance_kline_stream(ticker, tx_trades, tx_uds, broadcast_cfg, kline_cache, agg_tx).await;
        });
    }

    // Background kline refresh task - checks for stale cache every 5 minutes
    // Prevents per-client refresh during handshake (which can stall under burst connects)
    {
        let kline_cache = Arc::clone(&kline_cache);
        let tx_uds = Arc::clone(&tx_uds);
        let agg_tx = Arc::clone(&agg_event_tx);
        let tickers = tickers.clone();
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(300)); // 5 minutes
            loop {
                interval.tick().await;
                let today = Utc::now().date_naive();
                let mut stale_tickers: Vec<String> = Vec::new();

                // Check which tickers have stale cache
                {
                    let cache = kline_cache.lock().await;
                    for ticker in &tickers {
                        if let Some(candles) = cache.get(ticker) {
                            let is_stale = candles.is_empty()
                                || candles.back().map(|c| c.start_time.date_naive() != today).unwrap_or(true);
                            if is_stale {
                                stale_tickers.push(ticker.clone());
                            }
                        } else {
                            stale_tickers.push(ticker.clone());
                        }
                    }
                }

                // Refresh stale tickers in background
                for ticker in &stale_tickers {
                    let symbol = ticker_to_binance_symbol(ticker);
                    if let Ok(fresh_candles) = fetch_binance_1m_candles(symbol).await {
                        let mut cache = kline_cache.lock().await;
                        let entry = cache.entry(ticker.clone()).or_insert_with(VecDeque::new);
                        entry.clear();
                        for candle in &fresh_candles {
                            entry.push_back(candle.clone());
                        }
                        while entry.len() > 300 {
                            entry.pop_front();
                        }
                        info!("Background refresh: {} klines for {}", entry.len(), ticker);

                        // Also update aggregator
                        let payload = CandleBackfill {
                            ticker: ticker.clone(),
                            candles: fresh_candles,
                        };
                        let event = MarketEventMessage {
                            time_exchange: Utc::now(),
                            time_received: Utc::now(),
                            exchange: "barter-data-server".to_string(),
                            instrument: InstrumentInfo {
                                base: ticker.clone(),
                                quote: "USD".to_string(),
                                kind: "Kline".to_string(),
                            },
                            kind: "candle_backfill".to_string(),
                            data: serde_json::to_value(&payload).unwrap_or_default(),
                        };
                        if let Some(frame) = serialize_for_uds(&event) {
                            let _ = tx_uds.send(frame);
                        }
                        let _ = agg_tx.send(event).await;
                    }
                }

                if !stale_tickers.is_empty() {
                    info!("Background kline refresh completed for {} tickers", stale_tickers.len());
                }
            }
        });
    }

    {
        let tx_trades = tx_trades.clone();
        let tx_uds = Arc::clone(&tx_uds);
        let broadcast_cfg = Arc::clone(&broadcast_config);
        let snapshot_builder = Arc::clone(&snapshot_builder);
        let agg_snap_rx = Arc::clone(&agg_snapshot_rx);
        let vol_states = Arc::clone(&vol_states);
        let snapshot_secs: u64 = std::env::var("SNAPSHOT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(snapshot_secs));
            loop {
                tick.tick().await;
                let mut snapshot = snapshot_builder.lock().await.snapshot();
                // Read latest snapshot from watch channel (no mutex!)
                let agg_snapshot = agg_snap_rx.borrow().clone();
                let mut engines = vol_states.lock().await;
                for (ticker, snap) in snapshot.tickers.iter_mut() {
                    let rv = agg_snapshot
                        .tickers
                        .get(ticker)
                        .and_then(|t| t.realized_vol_1h)
                        .unwrap_or(0.0);
                    let state = engines.entry(ticker.clone()).or_insert_with(|| VolRegimeState {
                        engine: VolRegimeEngine::default(),
                        last_hour: None,
                    });
                    let now_hour = Utc::now().timestamp() / 3600;
                    if rv > 0.0 && state.last_hour != Some(now_hour) {
                        state.engine.push_rv(rv);
                        state.last_hour = Some(now_hour);
                    }
                    let sample_count = state.engine.rv_sample_count().min(RV_TARGET_SAMPLES);
                    snap.vol_samples = sample_count as u16;
                    if sample_count < RV_MIN_SAMPLES || rv <= 0.0 {
                        snap.vol_percentile = 50.0;
                        snap.vol_regime = "warmup".to_string();
                    } else {
                        let pct = state.engine.percentile_for(rv);
                        snap.vol_percentile = pct;
                        snap.vol_regime = vol_regime_label(state.engine.regime_for(rv)).to_string();
                    }
                }
                let event = MarketEventMessage {
                    time_exchange: Utc::now(),
                    time_received: Utc::now(),
                    exchange: "barter-data-server".to_string(),
                    instrument: InstrumentInfo {
                        base: "ALL".to_string(),
                        quote: "USD".to_string(),
                        kind: "Snapshot".to_string(),
                    },
                    kind: "market_snapshot".to_string(),
                    data: serde_json::to_value(&snapshot).unwrap_or_default(),
                };
                if let Some(frame) = serialize_for_uds(&event) {
                    let _ = tx_uds.send(frame);
                }
                if let Some(bytes) = serialize_for_broadcast(&broadcast_cfg, event) {
                    let _ = tx_trades.send(bytes);
                }
            }
        });
    }

    // Centralized orchestrator (MarketState) - broadcasts orchestrator_result
    {
        let tx_trades = tx_trades.clone();
        let tx_uds = Arc::clone(&tx_uds);
        let broadcast_cfg = Arc::clone(&broadcast_config);
        let agg_snap_rx = Arc::clone(&agg_snapshot_rx);
        let options_cache = Arc::clone(&options_cache);
        let trad_last_ms = Arc::clone(&trad_last_ms);
        tokio::spawn(async move {
            let config = Config::load().unwrap_or_else(|e| {
                warn!("Failed to load config: {}, using defaults", e);
                Config::default()
            });
            let logger = AuditLogger::no_op();
            let trad_fresh_ms = config.freshness(Signal::TradMarkets).as_millis() as u64;
            let mut orchestrator = StateOrchestrator::new(config, logger);
            let mut tick = interval(Duration::from_millis(200));
            loop {
                tick.tick().await;
                // Read latest snapshot from watch channel (no mutex!)
                let snapshot = agg_snap_rx.borrow().clone();
                let options_snapshot = { options_cache.lock().await.clone() };
                let trad_ts = trad_last_ms.load(Ordering::Relaxed);
                let trad_status = if trad_ts <= 0 {
                    TradMarketStatus::Unavailable
                } else {
                    let now_ms = Utc::now().timestamp_millis();
                    let age_ms = (now_ms - trad_ts).max(0) as u64;
                    if age_ms > trad_fresh_ms {
                        TradMarketStatus::Stale
                    } else {
                        TradMarketStatus::Live
                    }
                };

                for (ticker, ticker_snap) in &snapshot.tickers {
                    let server_ticker = snapshot
                        .server_snapshot
                        .as_ref()
                        .and_then(|snap| snap.tickers.get(ticker));
                    let mut input = build_market_data_input(
                        ticker_snap,
                        server_ticker,
                        trad_status,
                    );
                    input.timestamps.trad_markets_ts = trad_ts;
                    if let Some(ctx) = options_snapshot.get(ticker) {
                        input.options_context = Some(ctx.clone());
                    }
                    let result = orchestrator.calculate(&input);
                    let payload = OrchestratorMessage {
                        ticker: ticker.clone(),
                        result,
                    };
                    let event = MarketEventMessage {
                        time_exchange: Utc::now(),
                        time_received: Utc::now(),
                        exchange: "barter-data-server".to_string(),
                        instrument: InstrumentInfo {
                            base: ticker.clone(),
                            quote: "USD".to_string(),
                            kind: "Orchestrator".to_string(),
                        },
                        kind: "orchestrator_result".to_string(),
                        data: serde_json::to_value(&payload).unwrap_or_default(),
                    };
                    if let Some(frame) = serialize_for_uds(&event) {
                        let _ = tx_uds.send(frame);
                    }
                    if let Some(bytes) = serialize_for_broadcast(&broadcast_cfg, event) {
                        let _ = tx_trades.send(bytes);
                    }
                }
            }
        });
    }

    // Volatility regime updater: push hourly RV samples into engines
    {
        let agg_snap_rx = Arc::clone(&agg_snapshot_rx);
        let vol_states = Arc::clone(&vol_states);
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(60));
            loop {
                tick.tick().await;
                let now_hour = Utc::now().timestamp() / 3600;
                // Read latest snapshot from watch channel (no mutex!)
                let snapshot = agg_snap_rx.borrow().clone();
                let mut states = vol_states.lock().await;
                for (ticker, snap) in &snapshot.tickers {
                    let rv = snap.realized_vol_1h.unwrap_or(0.0);
                    if rv <= 0.0 {
                        continue;
                    }
                    let state = states
                        .entry(ticker.clone())
                        .or_insert_with(|| VolRegimeState {
                            engine: VolRegimeEngine::default(),
                            last_hour: None,
                        });
                    if state.last_hour != Some(now_hour) {
                        state.engine.push_rv(rv);
                        state.last_hour = Some(now_hour);
                    }
                }
            }
        });
    }

    info!("WebSocket server listening on ws://{}", server_addr);
    info!("Clients can connect to receive real-time market data");

    // Initialize market data streams
    let streams = init_market_streams().await;

    // Combine WebSocket and REST API streams
    let combined_stream = stream::select_all(vec![
        streams
            .select_all::<MarketStreamResult<MarketDataInstrument, DataKind>>()
            .boxed(),
        binance_open_interest_stream().boxed(),
        funding_rate_stream().boxed(),
    ]);

    futures::pin_mut!(combined_stream);

    // Clone event sender for main loop (hot path - no mutex!)
    let agg_tx = Arc::clone(&agg_event_tx);
    let spot_cache = Arc::clone(&spot_cache);

    // Throttle state: per-instrument last broadcast time (L2 and Binance L1)
    let mut l2_last_broadcast: HashMap<(ExchangeId, String, String), Instant> = HashMap::new();
    let mut l1_last_broadcast: HashMap<String, Instant> = HashMap::new();

    // Process market events and broadcast to clients
    while let Some(event) = combined_stream.next().await {
        match event {
            Event::Reconnecting(exchange) => {
                warn!("Reconnecting to {:?}", exchange);
            }
            Event::Item(result) => match result {
                Ok(market_event) => {
                    snapshot_builder.lock().await.update(&market_event);
                    if let DataKind::Trade(trade) = &market_event.kind {
                        if matches!(market_event.instrument.kind, MarketDataInstrumentKind::Perpetual) {
                            let mut cache = spot_cache.lock().await;
                            cache.insert(
                                market_event.instrument.base.to_string().to_uppercase(),
                                trade.price,
                            );
                        }
                    }
                    // Debug logging for large spot trades to verify spot streams
                    // Uses cached threshold (no env var parsing in hot path)
                    if let DataKind::Trade(trade) = &market_event.kind {
                        let notional = trade.price * trade.amount;
                        let is_spot =
                            matches!(market_event.instrument.kind, MarketDataInstrumentKind::Spot);
                        if is_spot && notional >= broadcast_config.spot_log_threshold {
                            debug!(
                                "SPOT TRADE >=50k {} {}/{} @ {} qty {} notional {} side {:?}",
                                market_event.exchange,
                                market_event.instrument.base,
                                market_event.instrument.quote,
                                trade.price,
                                trade.amount,
                                notional,
                                trade.side
                            );
                        }
                    }

                    // Debug logging for liquidation events to verify flow
                    if let DataKind::Liquidation(liq) = &market_event.kind {
                        debug!(
                            "LIQ EVENT {} {}/{} @ {} qty {} side {:?}",
                            market_event.exchange,
                            market_event.instrument.base,
                            market_event.instrument.quote,
                            liq.price,
                            liq.quantity,
                            liq.side
                        );
                    }

                    // Debug logging for open interest events
                    if let DataKind::OpenInterest(oi) = &market_event.kind {
                        debug!(
                            "OI EVENT {} {}/{} contracts: {} notional: {:?}",
                            market_event.exchange,
                            market_event.instrument.base,
                            market_event.instrument.quote,
                            oi.contracts,
                            oi.notional
                        );
                    }

                    let is_liquidation = matches!(&market_event.kind, DataKind::Liquidation(_));
                    let is_open_interest = matches!(&market_event.kind, DataKind::OpenInterest(_));
                    let is_trade = matches!(&market_event.kind, DataKind::Trade(_));
                    let is_orderbook_l2 = matches!(&market_event.kind, DataKind::OrderBook(_));
                    let is_orderbook_l1 = matches!(
                        &market_event.kind,
                        DataKind::OrderBookL1(_)
                    );

                    // Extract notional value for trades
                    let trade_notional = if let DataKind::Trade(t) = &market_event.kind {
                        Some(t.price * t.amount)
                    } else {
                        None
                    };

                    // Send trades/bars to Parquet writer (if enabled).
                    // Per requirement: only persist PERP data (avoid mixing spot/perp).
                    // Uses try_send for non-blocking - drops with logging/metrics if full.
                    if parquet_enabled
                        && matches!(market_event.instrument.kind, MarketDataInstrumentKind::Perpetual)
                    {
                        // Build instrument_id in Nautilus format: {BASE}{QUOTE}-{KIND}.{VENUE}
                        // e.g., "BTCUSDT-PERP.BINANCE"
                        let venue = exchange_to_venue(&market_event.exchange);
                        let instrument_id = format!(
                            "{}{}-{}.{}",
                            market_event.instrument.base,
                            market_event.instrument.quote,
                            match market_event.instrument.kind {
                                MarketDataInstrumentKind::Perpetual => "PERP",
                                MarketDataInstrumentKind::Spot => "SPOT",
                                _ => "OTHER",
                            },
                            venue
                        ).to_uppercase();
                        let (price_precision, size_precision) = precision_config.get(&instrument_id);

                        // Update extended bar builder with latest OI/Funding/L1
                        match &market_event.kind {
                            DataKind::OpenInterest(oi) => {
                                let oi_value = oi.contracts;
                                let mut ext_builders = extended_bar_builders.lock().await;
                                let ext_builder = ext_builders
                                    .entry(instrument_id.clone())
                                    .or_insert_with(ExtendedBarBuilder::new);
                                ext_builder.update_oi(oi_value);
                            }
                            DataKind::FundingRate(fr) => {
                                let mut ext_builders = extended_bar_builders.lock().await;
                                let ext_builder = ext_builders
                                    .entry(instrument_id.clone())
                                    .or_insert_with(ExtendedBarBuilder::new);
                                ext_builder.update_funding(fr.rate);
                            }
                            DataKind::Liquidation(liq) => {
                                let notional = liq.price * liq.quantity;
                                let mut ext_builders = extended_bar_builders.lock().await;
                                let ext_builder = ext_builders
                                    .entry(instrument_id.clone())
                                    .or_insert_with(ExtendedBarBuilder::new);
                                ext_builder.update_liquidation(notional, liq.side);
                            }
                            DataKind::OrderBookL1(l1) => {
                                let bid = l1.best_bid.and_then(|lvl| lvl.price.to_f64()).unwrap_or(0.0);
                                let bid_size = l1.best_bid.and_then(|lvl| lvl.amount.to_f64()).unwrap_or(0.0);
                                let ask = l1.best_ask.and_then(|lvl| lvl.price.to_f64()).unwrap_or(0.0);
                                let ask_size = l1.best_ask.and_then(|lvl| lvl.amount.to_f64()).unwrap_or(0.0);
                                if bid > 0.0 && ask > 0.0 {
                                    let mut ext_builders = extended_bar_builders.lock().await;
                                    let ext_builder = ext_builders
                                        .entry(instrument_id.clone())
                                        .or_insert_with(ExtendedBarBuilder::new);
                                    ext_builder.update_l1(bid, bid_size, ask, ask_size);
                                }
                            }
                            DataKind::OrderBook(ob_event) => {
                                let mut cache = l2_book_cache.lock().await;
                                let book = cache
                                    .entry(instrument_id.clone())
                                    .or_insert_with(OrderBook::default);
                                book.update(ob_event);
                            }
                            _ => {}
                        }

                        if let DataKind::Trade(trade) = &market_event.kind {

                            let ts_init_ns = market_event.time_received
                                .timestamp_nanos_opt()
                                .unwrap_or(0) as u64;
                            let mut ts_event_ns = market_event.time_exchange
                                .timestamp_nanos_opt()
                                .unwrap_or(0) as u64;
                            if ts_event_ns == 0 {
                                // Fallback to receive time if exchange timestamp is missing/invalid.
                                ts_event_ns = ts_init_ns;
                            }

                            let parquet_event = ParquetEvent::Trade(TradeEvent {
                                instrument_id: instrument_id.clone(),
                                price: trade.price,
                                size: trade.amount,
                                side: Some(trade.side),
                                trade_id: trade.id.clone(),
                                ts_event_ns,
                                ts_init_ns,
                                price_precision,
                                size_precision,
                            });

                            if parquet_tx.try_send(parquet_event).is_err() {
                                // Log and increment metric - completeness is critical
                                let drops = PARQUET_DROPS.fetch_add(1, Ordering::Relaxed) + 1;
                                if drops % 1000 == 1 {
                                    warn!("Parquet channel full, trade dropped (total drops: {})", drops);
                                }
                            } else {
                                // Increment heartbeat counter on successful send
                                heartbeat_trades_processed.fetch_add(1, Ordering::Relaxed);
                            }

                            // Aggregate trades into 1-minute bars
                            // When a minute boundary is crossed, emit the completed bar
                            let completed_bar = {
                                let mut aggregator = bar_aggregator.lock().await;
                                aggregator.process_trade(
                                    &instrument_id,
                                    trade.price,
                                    trade.amount,
                                    Some(trade.side),
                                    ts_event_ns,
                                    price_precision,
                                    size_precision,
                                )
                            };

                            if let Some(completed_bar) = completed_bar {
                                // Send completed bar to Parquet writer (Nautilus-compatible core bar)
                                // BLOCKING SEND: Core bars must never be dropped per PRD
                                let bar_event = ParquetEvent::Bar(completed_bar.to_bar_event());
                                if parquet_tx.send(bar_event).await.is_err() {
                                    // Channel closed - receiver dropped
                                    error!("Parquet channel closed, cannot send bar");
                                } else {
                                    // Increment heartbeat bar counter
                                    heartbeat_bars_written.fetch_add(1, Ordering::Relaxed);

                                    // Update heartbeat state with symbol and last bar time
                                    if let Some(ref state) = heartbeat_state {
                                        let bar_time = chrono::DateTime::from_timestamp_nanos(
                                            completed_bar.ts_close_ns as i64,
                                        );
                                        update_heartbeat_bar(
                                            state,
                                            &completed_bar.instrument_id,
                                            bar_time,
                                        )
                                        .await;
                                    }

                                    debug!(
                                        "Emitted 1m bar for {} close={} vol={:.4} trades={}",
                                        completed_bar.instrument_id,
                                        completed_bar.close,
                                        completed_bar.volume,
                                        completed_bar.trade_count
                                    );
                                }

                                // Compute L2 depth bands from latest order book snapshot (if available)
                                let depth_bands = {
                                    let cache = l2_book_cache.lock().await;
                                    cache
                                        .get(&instrument_id)
                                        .and_then(|book| compute_depth_bands(book))
                                };

                                // Also create and send extended bar (Barter-only, with CVD/delta)
                                let extended_bar = {
                                    let mut ext_builders = extended_bar_builders.lock().await;
                                    let ext_builder = ext_builders
                                        .entry(instrument_id.clone())
                                        .or_insert_with(ExtendedBarBuilder::new);
                                    ext_builder.update_depth_bands(depth_bands);
                                    ext_builder.build(completed_bar)
                                };

                                let ext_bar_event = ParquetEvent::ExtendedBar(ExtendedBarEvent {
                                    instrument_id: extended_bar.instrument_id,
                                    ts_event_ns: extended_bar.ts_close_ns,
                                    ts_init_ns: extended_bar.ts_init_ns,
                                    ts_open_ns: extended_bar.ts_open_ns,
                                    open: extended_bar.open,
                                    high: extended_bar.high,
                                    low: extended_bar.low,
                                    close: extended_bar.close,
                                    volume: extended_bar.volume,
                                    quote_volume: extended_bar.quote_volume,
                                    trade_count: extended_bar.trade_count,
                                    buy_volume: extended_bar.buy_volume,
                                    sell_volume: extended_bar.sell_volume,
                                    delta: extended_bar.delta,
                                    cvd: extended_bar.cvd,
                                    open_interest: extended_bar.open_interest,
                                    oi_change: extended_bar.oi_change,
                                    funding_rate: extended_bar.funding_rate,
                                    bid_price: extended_bar.bid_price,
                                    bid_size: extended_bar.bid_size,
                                    ask_price: extended_bar.ask_price,
                                    ask_size: extended_bar.ask_size,
                                    spread_bps: extended_bar.spread_bps,
                                    book_imbalance: extended_bar.book_imbalance,
                                    liq_buy_usd: extended_bar.liq_buy_usd,
                                    liq_sell_usd: extended_bar.liq_sell_usd,
                                    liq_total_usd: extended_bar.liq_total_usd,
                                    liq_count: extended_bar.liq_count,
                                    bid_depth_10bps_base: extended_bar.bid_depth_10bps_base,
                                    ask_depth_10bps_base: extended_bar.ask_depth_10bps_base,
                                    bid_depth_10bps_usd: extended_bar.bid_depth_10bps_usd,
                                    ask_depth_10bps_usd: extended_bar.ask_depth_10bps_usd,
                                    depth_imb_10bps: extended_bar.depth_imb_10bps,
                                    bid_depth_50bps_base: extended_bar.bid_depth_50bps_base,
                                    ask_depth_50bps_base: extended_bar.ask_depth_50bps_base,
                                    bid_depth_50bps_usd: extended_bar.bid_depth_50bps_usd,
                                    ask_depth_50bps_usd: extended_bar.ask_depth_50bps_usd,
                                    depth_imb_50bps: extended_bar.depth_imb_50bps,
                                    bid_depth_100bps_base: extended_bar.bid_depth_100bps_base,
                                    ask_depth_100bps_base: extended_bar.ask_depth_100bps_base,
                                    bid_depth_100bps_usd: extended_bar.bid_depth_100bps_usd,
                                    ask_depth_100bps_usd: extended_bar.ask_depth_100bps_usd,
                                    depth_imb_100bps: extended_bar.depth_imb_100bps,
                                    price_precision: extended_bar.price_precision,
                                    size_precision: extended_bar.size_precision,
                                });

                                // BLOCKING SEND: Extended bars should also not be dropped
                                if parquet_tx.send(ext_bar_event).await.is_err() {
                                    error!("Parquet channel closed, cannot send extended bar");
                                }
                            }
                        }
                    }

                    // L2 orderbook events: apply per-exchange throttle and route to L2 channel
                    if is_orderbook_l2 {
                        debug!(
                            "L2_BOOK {} {}/{}",
                            market_event.exchange,
                            market_event.instrument.base,
                            market_event.instrument.quote
                        );

                        // Per-exchange throttling (optimized: no extra string allocation)
                        let throttle_ms = get_l2_throttle_ms_for_exchange(&market_event.exchange);
                        let now = Instant::now();

                        // Use compact key format: exchange:base:quote
                        // The key is still needed for HashMap but we avoid the throttle string format
                        let key = (
                            market_event.exchange,
                            market_event.instrument.base.to_string(),
                            market_event.instrument.quote.to_string(),
                        );

                        let should_skip = if let Some(prev) = l2_last_broadcast.get(&key) {
                            now.duration_since(*prev) < Duration::from_millis(throttle_ms)
                        } else {
                            false
                        };

                        if should_skip {
                            continue; // Skip throttled L2
                        }
                        l2_last_broadcast.insert(key, now);

                        // Send to L2 channel (separate from trades)
                        let message = market_event_to_message(market_event);
                        // Send to aggregator via channel (no mutex!)
                        // Rate-limited warning: at most once per second when drops occur
                        if agg_tx.try_send(message.clone()).is_err() {
                            let dropped = AGG_CHANNEL_DROPPED.fetch_add(1, Ordering::Relaxed) + 1;
                            let now_ms = Utc::now().timestamp_millis();
                            let last_warn = AGG_LAST_DROP_WARN_MS.load(Ordering::Relaxed);
                            if now_ms - last_warn > 1000 {
                                AGG_LAST_DROP_WARN_MS.store(now_ms, Ordering::Relaxed);
                                warn!(
                                    "Aggregator channel full - dropped {} events total (consider increasing AGG_EVENT_BUFFER)",
                                    dropped
                                );
                            }
                        }
                        if let Some(frame) = serialize_for_uds(&message) {
                            let _ = tx_uds.send(frame);
                        }
                        if let Some(bytes) = serialize_for_broadcast(&broadcast_config, message) {
                            let _ = tx_l2.send(bytes); // Ignore errors if no receivers
                        }
                        continue; // Don't fall through to trade channel
                    }

                    let message = market_event_to_message(market_event);
                    // Track per-feed metrics
                    let now_ms = Utc::now().timestamp_millis();
                    if message.exchange.contains("Binance") {
                        BINANCE_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
                        BINANCE_LAST_EVENT_MS.store(now_ms, Ordering::Relaxed);
                    } else if message.exchange.contains("Okx") {
                        OKX_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
                        OKX_LAST_EVENT_MS.store(now_ms, Ordering::Relaxed);
                    } else if message.exchange.contains("Bybit") {
                        BYBIT_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
                        BYBIT_LAST_EVENT_MS.store(now_ms, Ordering::Relaxed);
                    }
                    // Send to aggregator via channel (no mutex!)
                    // Rate-limited warning: at most once per second when drops occur
                    if agg_tx.try_send(message.clone()).is_err() {
                        let dropped = AGG_CHANNEL_DROPPED.fetch_add(1, Ordering::Relaxed) + 1;
                        let now_ms = Utc::now().timestamp_millis();
                        let last_warn = AGG_LAST_DROP_WARN_MS.load(Ordering::Relaxed);
                        if now_ms - last_warn > 1000 {
                            AGG_LAST_DROP_WARN_MS.store(now_ms, Ordering::Relaxed);
                            warn!(
                                "Aggregator channel full - dropped {} events total (consider increasing AGG_EVENT_BUFFER)",
                                dropped
                            );
                        }
                    }
                    if let Some(frame) = serialize_for_uds(&message) {
                        let _ = tx_uds.send(frame);
                    }

                    // Binance L1: apply light throttle (~100ms per instrument) to reduce flood
                    if is_orderbook_l1 {
                        let exchange_name = &message.exchange;
                        if exchange_name.contains("Binance") {
                            let key = format!(
                                "{}:{}:{}",
                                exchange_name,
                                message.instrument.base,
                                message.instrument.quote
                            );
                            // Use cached throttle value (no env var parsing in hot path)
                            let now = Instant::now();
                            let should_skip = if let Some(prev) = l1_last_broadcast.get(&key) {
                                now.duration_since(*prev) < Duration::from_millis(broadcast_config.l1_throttle_ms)
                            } else {
                                false
                            };
                            if should_skip {
                                continue; // Skip throttled Binance L1 update
                            }
                            l1_last_broadcast.insert(key, now);
                        }
                    }

                    // Debug: log broadcast attempt for all event types
                    // NOTE: Changed from info! to debug! to avoid blocking hot path
                    if is_trade {
                        // Track metrics: trade count and timestamp skew
                        TRADE_COUNT.fetch_add(1, Ordering::Relaxed);
                        let skew_ms = (message.time_received - message.time_exchange).num_milliseconds();
                        // Only count positive skew for avg (negative = exchange clock ahead)
                        if skew_ms >= 0 {
                            SKEW_SUM_MS.fetch_add(skew_ms, Ordering::Relaxed);
                            SKEW_COUNT.fetch_add(1, Ordering::Relaxed);
                        }
                        // Track max skew (positive = server behind)
                        let mut current_max = SKEW_MAX_MS.load(Ordering::Relaxed);
                        while skew_ms > current_max {
                            match SKEW_MAX_MS.compare_exchange_weak(
                                current_max, skew_ms, Ordering::Relaxed, Ordering::Relaxed
                            ) {
                                Ok(_) => break,
                                Err(x) => current_max = x,
                            }
                        }
                        // Track min skew (negative = exchange clock ahead)
                        let mut current_min = SKEW_MIN_MS.load(Ordering::Relaxed);
                        while skew_ms < current_min {
                            match SKEW_MIN_MS.compare_exchange_weak(
                                current_min, skew_ms, Ordering::Relaxed, Ordering::Relaxed
                            ) {
                                Ok(_) => break,
                                Err(x) => current_min = x,
                            }
                        }

                        let receivers = tx_trades.receiver_count();
                        debug!(
                            "TRADE→{} clients: {} {} {}/{} ${:.0}",
                            receivers,
                            message.exchange,
                            message.instrument.kind,
                            message.instrument.base,
                            message.instrument.quote,
                            trade_notional.unwrap_or(0.0)
                        );
                    }
                    if is_liquidation {
                        let receivers = tx_trades.receiver_count();
                        debug!(
                            "BROADCASTING liquidation to {} clients: {} {}/{}",
                            receivers,
                            message.exchange,
                            message.instrument.base,
                            message.instrument.quote
                        );
                    }
                    if is_open_interest {
                        let receivers = tx_trades.receiver_count();
                        debug!(
                            "BROADCASTING open_interest to {} clients: {} {}/{}",
                            receivers,
                            message.exchange,
                            message.instrument.base,
                            message.instrument.quote
                        );
                    }

                    // Broadcast to trade channel (hot path - NO L2 here)
                    // Pre-serialized to avoid per-client JSON serialization overhead
                    if let Some(bytes) = serialize_for_broadcast(&broadcast_config, message) {
                        match tx_trades.send(bytes) {
                            Ok(count) => {
                                if is_trade {
                                    debug!("Trade sent to {} receivers", count);
                                }
                                if is_liquidation {
                                    debug!("Liquidation sent to {} receivers", count);
                                }
                                if is_open_interest {
                                    debug!("OpenInterest sent to {} receivers", count);
                                }
                            }
                            Err(e) => {
                                if is_trade {
                                    warn!("Failed to broadcast trade: {:?}", e);
                                }
                                if is_liquidation {
                                    warn!("Failed to broadcast liquidation: {:?}", e);
                                }
                                if is_open_interest {
                                    warn!("Failed to broadcast open_interest: {:?}", e);
                                }
                            }
                        }
                    }
                }
                Err(error) => {
                    // Filter out known non-errors
                    let error_str = format!("{:?}", error);
                    if !error_str.contains("payload: pong")
                        && !error_str.contains("liquidation-orders|SWAP")
                    {
                        debug!("Market stream error: {:?}", error);
                    }
                }
            },
        }
    }
}

/// Start WebSocket server that broadcasts market events to connected clients
async fn start_websocket_server(
    addr: SocketAddr,
    tx_trades: Arc<broadcast::Sender<Bytes>>,
    tx_l2: Arc<broadcast::Sender<Bytes>>,
    kline_cache: Arc<tokio::sync::Mutex<HashMap<String, VecDeque<Candle1m>>>>,
    broadcast_config: Arc<BroadcastConfig>,
) {
    let listener = TcpListener::bind(&addr)
        .await
        .expect("Failed to bind WebSocket server");

    info!("WebSocket server bound to {}", addr);

    while let Ok((stream, peer_addr)) = listener.accept().await {
        info!("New WebSocket connection from {}", peer_addr);
        let tx_trades = tx_trades.clone();
        let tx_l2 = tx_l2.clone();
        let kline_cache = Arc::clone(&kline_cache);
        let broadcast_config = Arc::clone(&broadcast_config);
        tokio::spawn(handle_client(
            stream,
            peer_addr,
            tx_trades,
            tx_l2,
            kline_cache,
            broadcast_config,
        ));
    }
}

#[cfg(unix)]
async fn start_uds_server(path: String, tx_uds: Arc<broadcast::Sender<Bytes>>) {
    if Path::new(&path).exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            warn!("Failed to remove existing UDS socket {}: {}", path, e);
        }
    }

    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(e) => {
            error!("Failed to bind UDS socket {}: {}", path, e);
            return;
        }
    };

    info!("UDS server bound to {}", path);

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let tx_uds = tx_uds.clone();
                tokio::spawn(handle_uds_client(stream, tx_uds));
            }
            Err(e) => {
                warn!("UDS accept error: {}", e);
            }
        }
    }
}

#[cfg(unix)]
async fn handle_uds_client(mut stream: UnixStream, tx_uds: Arc<broadcast::Sender<Bytes>>) {
    let mut rx = tx_uds.subscribe();
    loop {
        match rx.recv().await {
            Ok(frame) => {
                if stream.write_all(&frame).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                IPC_LAGGED_FRAMES.fetch_add(skipped as u64, Ordering::Relaxed);
                debug!("UDS client lagged, skipped {} frames", skipped);
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn start_tcp_server(addr: String, tx_uds: Arc<broadcast::Sender<Bytes>>) {
    let listener = match TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(e) => {
            error!("Failed to bind TCP IPC {}: {}", addr, e);
            return;
        }
    };

    info!("TCP IPC server bound to {}", addr);

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let tx_uds = tx_uds.clone();
                tokio::spawn(handle_tcp_client(stream, tx_uds));
            }
            Err(e) => {
                warn!("TCP IPC accept error: {}", e);
            }
        }
    }
}

async fn handle_tcp_client(mut stream: TcpStream, tx_uds: Arc<broadcast::Sender<Bytes>>) {
    let mut rx = tx_uds.subscribe();
    loop {
        match rx.recv().await {
            Ok(frame) => {
                if stream.write_all(&frame).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                IPC_LAGGED_FRAMES.fetch_add(skipped as u64, Ordering::Relaxed);
                debug!("TCP IPC client lagged, skipped {} frames", skipped);
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn start_ibkr_bridge_feed(
    tx_trades: Arc<broadcast::Sender<Bytes>>,
    tx_uds: Arc<broadcast::Sender<Bytes>>,
    broadcast_config: Arc<BroadcastConfig>,
    trad_last_ms: Arc<AtomicI64>,
) {
    if std::env::var("IBKR_BRIDGE_ENABLED")
        .ok()
        .map(|v| matches!(v.as_str(), "0" | "false" | "FALSE"))
        .unwrap_or(false)
    {
        info!("IBKR bridge feed disabled (IBKR_BRIDGE_ENABLED=0)");
        return;
    }

    let url = std::env::var("IBKR_BRIDGE_WS_URL")
        .unwrap_or_else(|_| "ws://127.0.0.1:8765/ws".to_string());
    info!("IBKR bridge feed enabled: {}", url);

    loop {
        match connect_async(&url).await {
            Ok((ws_stream, _)) => {
                info!("Connected to ibkr-bridge at {}", url);
                let (_, mut read) = ws_stream.split();

                let mut tick_count: u64 = 0;
                let mut tick_log = interval(Duration::from_secs(60));
                tick_log.tick().await;

                loop {
                    tokio::select! {
                        _ = tick_log.tick() => {
                            info!("ibkr-bridge: {} ticks forwarded in last 60s", tick_count);
                            tick_count = 0;
                        }
                        msg = read.next() => {
                            match msg {
                                Some(Ok(Message::Text(text))) => {
                                    match serde_json::from_str::<IbkrMessage>(&text) {
                                        Ok(IbkrMessage::Tick(tick)) => {
                                            if tick.ts > 0 {
                                                trad_last_ms.store(tick.ts, Ordering::Relaxed);
                                            }
                                            // Track IBKR feed health
                                            IBKR_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
                                            IBKR_LAST_EVENT_MS.store(Utc::now().timestamp_millis(), Ordering::Relaxed);
                                            let event = trad_tick_event(tick);
                                            if let Some(bytes) = serialize_for_broadcast(&broadcast_config, event.clone()) {
                                                let _ = tx_trades.send(bytes);
                                            }
                                            if let Some(frame) = serialize_for_uds(&event) {
                                                let _ = tx_uds.send(frame);
                                            }
                                            tick_count += 1;
                                        }
                                        Ok(IbkrMessage::TickBackfill { ticks, .. }) => {
                                            for tick in ticks {
                                                if tick.ts > 0 {
                                                    trad_last_ms.store(tick.ts, Ordering::Relaxed);
                                                }
                                                let event = trad_tick_event(tick);
                                                if let Some(bytes) = serialize_for_broadcast(&broadcast_config, event.clone()) {
                                                    let _ = tx_trades.send(bytes);
                                                }
                                                if let Some(frame) = serialize_for_uds(&event) {
                                                    let _ = tx_uds.send(frame);
                                                }
                                            }
                                        }
                                        Ok(IbkrMessage::Welcome { .. }) => {
                                            debug!("IBKR bridge welcome received");
                                        }
                                        Ok(IbkrMessage::Status { .. }) => {
                                            debug!("IBKR bridge status received");
                                        }
                                        Err(e) => {
                                            debug!("IBKR parse error: {}", e);
                                        }
                                    }
                                }
                                Some(Ok(Message::Close(_))) => {
                                    warn!("ibkr-bridge connection closed");
                                    break;
                                }
                                Some(Ok(Message::Ping(_)) | Ok(Message::Pong(_))) => {}
                                Some(Err(e)) => {
                                    warn!("ibkr-bridge websocket error: {}", e);
                                    break;
                                }
                                Some(Ok(_)) => {}
                                None => {
                                    warn!("ibkr-bridge stream ended (no close frame), reconnecting...");
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to connect to ibkr-bridge at {}: {}", url, e);
            }
        }

        info!("ibkr-bridge: reconnecting in 5 seconds...");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn start_deribit_options_feed(
    tx_trades: Arc<broadcast::Sender<Bytes>>,
    tx_uds: Arc<broadcast::Sender<Bytes>>,
    broadcast_config: Arc<BroadcastConfig>,
    spot_cache: Arc<tokio::sync::Mutex<HashMap<String, f64>>>,
    options_cache: Arc<tokio::sync::Mutex<HashMap<String, OptionsContext>>>,
) {
    if std::env::var("DERIBIT_ENABLED")
        .ok()
        .map(|v| matches!(v.as_str(), "0" | "false" | "FALSE"))
        .unwrap_or(false)
    {
        info!("Deribit options feed disabled (DERIBIT_ENABLED=0)");
        return;
    }

    let base_url = std::env::var("DERIBIT_API_BASE")
        .unwrap_or_else(|_| "https://www.deribit.com/api/v2/public".to_string());
    let tickers = std::env::var("DERIBIT_TICKERS").unwrap_or_else(|_| "BTC,ETH".to_string());
    let top_n: usize = std::env::var("DERIBIT_GREEKS_TOP_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    let refresh_secs: u64 = std::env::var("DERIBIT_REFRESH_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);

    let client = Client::new();
    let options_builder = OptionsContextBuilder::new();
    let ticker_list: Vec<String> = tickers
        .split(',')
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
        .collect();

    info!(
        "Deribit options feed enabled: {} (tickers: {:?}, refresh: {}s, greeks: top {})",
        base_url, ticker_list, refresh_secs, top_n
    );

    let mut interval = interval(Duration::from_secs(refresh_secs));
    loop {
        interval.tick().await;
        for ticker in &ticker_list {
            match fetch_deribit_options_chain(&client, &base_url, ticker, top_n).await {
                Ok(chain) => {
                    let event = MarketEventMessage {
                        time_exchange: Utc::now(),
                        time_received: Utc::now(),
                        exchange: "Deribit".to_string(),
                        instrument: InstrumentInfo {
                            base: ticker.clone(),
                            quote: "USD".to_string(),
                            kind: "Options".to_string(),
                        },
                        kind: "options_chain".to_string(),
                        data: serde_json::to_value(&chain).unwrap_or_default(),
                    };
                    if let Some(bytes) = serialize_for_broadcast(&broadcast_config, event.clone()) {
                        let _ = tx_trades.send(bytes);
                    }
                    if let Some(frame) = serialize_for_uds(&event) {
                        let _ = tx_uds.send(frame);
                    }

                    let spot = spot_cache
                        .lock()
                        .await
                        .get(ticker)
                        .copied()
                        .unwrap_or(0.0);
                    if spot > 0.0 {
                        let ctx = options_builder.build(&chain, spot);
                        options_cache.lock().await.insert(ticker.clone(), ctx.clone());
                        let ctx_event = MarketEventMessage {
                            time_exchange: Utc::now(),
                            time_received: Utc::now(),
                            exchange: "Deribit".to_string(),
                            instrument: InstrumentInfo {
                                base: ticker.clone(),
                                quote: "USD".to_string(),
                                kind: "Options".to_string(),
                            },
                            kind: "options_context".to_string(),
                            data: serde_json::to_value(&ctx).unwrap_or_default(),
                        };
                        if let Some(bytes) = serialize_for_broadcast(&broadcast_config, ctx_event.clone()) {
                            let _ = tx_trades.send(bytes);
                        }
                        if let Some(frame) = serialize_for_uds(&ctx_event) {
                            let _ = tx_uds.send(frame);
                        }
                    }
                }
                Err(e) => {
                    warn!("Deribit options fetch failed for {}: {}", ticker, e);
                }
            }
        }
    }
}

async fn seed_binance_klines(
    kline_cache: Arc<tokio::sync::Mutex<HashMap<String, VecDeque<Candle1m>>>>,
    tx_uds: Arc<broadcast::Sender<Bytes>>,
    agg_tx: Arc<mpsc::Sender<MarketEventMessage>>,
    tickers: &[String],
) {
    for ticker in tickers {
        let symbol = ticker_to_binance_symbol(ticker);
        match fetch_binance_1m_candles(symbol).await {
            Ok(candles) => {
                let mut cache = kline_cache.lock().await;
                let entry = cache.entry(ticker.clone()).or_insert_with(VecDeque::new);
                entry.clear();
                for candle in &candles {
                    entry.push_back(candle.clone());
                }
                while entry.len() > 300 {
                    entry.pop_front();
                }
                info!("Seeded {} klines for {}", entry.len(), ticker);

                // Warm the server-side aggregator using the same backfill
                let payload = CandleBackfill {
                    ticker: ticker.clone(),
                    candles,
                };
                let event = MarketEventMessage {
                    time_exchange: Utc::now(),
                    time_received: Utc::now(),
                    exchange: "barter-data-server".to_string(),
                    instrument: InstrumentInfo {
                        base: ticker.clone(),
                        quote: "USD".to_string(),
                        kind: "Kline".to_string(),
                    },
                    kind: "candle_backfill".to_string(),
                    data: serde_json::to_value(&payload).unwrap_or_default(),
                };
                if let Some(frame) = serialize_for_uds(&event) {
                    let _ = tx_uds.send(frame);
                }
                // Send to aggregator via channel (no mutex!)
                let _ = agg_tx.send(event).await;
            }
            Err(e) => {
                warn!("Failed to seed klines for {}: {}", ticker, e);
            }
        }
    }
}

const RV_HISTORY_DAYS: i64 = 7;
const RV_MIN_SAMPLES: usize = 24;
const RV_TARGET_SAMPLES: usize = 168;

async fn fetch_binance_5m_history(symbol: &str, days: i64) -> Result<Vec<(i64, f64)>, String> {
    let client = reqwest::Client::new();
    let mut start_ms = (Utc::now() - ChronoDuration::days(days)).timestamp_millis();
    let end_ms = Utc::now().timestamp_millis();
    let mut output: Vec<(i64, f64)> = Vec::new();

    loop {
        let url = format!(
            "https://fapi.binance.com/fapi/v1/klines?symbol={}&interval=5m&startTime={}&limit=1000",
            symbol, start_ms
        );
        let resp = client
            .get(&url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("HTTP error: {}", resp.status()));
        }

        let klines: Vec<BinanceKline> = resp
            .json()
            .await
            .map_err(|e| format!("JSON parse failed: {}", e))?;

        if klines.is_empty() {
            break;
        }

        for k in &klines {
            if let Ok(close) = k.4.parse::<f64>() {
                output.push((k.0, close));
            }
        }

        let last_open = klines.last().map(|k| k.0).unwrap_or(start_ms);
        let next_start = last_open + 5 * 60 * 1000;
        if next_start <= start_ms || next_start >= end_ms {
            break;
        }
        start_ms = next_start;

        if klines.len() < 1000 {
            break;
        }
    }

    output.sort_by_key(|(ts, _)| *ts);
    Ok(output)
}

fn realized_vol_from_closes(closes: &[f64]) -> Option<f64> {
    if closes.len() < 2 {
        return None;
    }
    let mut returns = Vec::with_capacity(closes.len() - 1);
    for i in 1..closes.len() {
        let prev = closes[i - 1];
        let cur = closes[i];
        if prev > 0.0 {
            returns.push((cur - prev) / prev);
        }
    }
    if returns.len() < 2 {
        return None;
    }
    let mean = returns.iter().sum::<f64>() / returns.len() as f64;
    let variance = returns
        .iter()
        .map(|r| {
            let diff = r - mean;
            diff * diff
        })
        .sum::<f64>()
        / returns.len() as f64;
    Some(variance.sqrt() * 100.0)
}

fn hourly_rv_from_5m(candles: &[(i64, f64)]) -> Vec<f64> {
    let mut samples = Vec::new();
    let start_idx = candles
        .iter()
        .position(|(ts, _)| ts % (60 * 60 * 1000) == 0)
        .unwrap_or(0);

    let mut idx = start_idx;
    while idx + 12 <= candles.len() {
        let closes: Vec<f64> = candles[idx..idx + 12].iter().map(|(_, c)| *c).collect();
        if let Some(rv) = realized_vol_from_closes(&closes) {
            samples.push(rv);
        }
        idx += 12;
    }

    samples
}

async fn seed_vol_regime(
    vol_states: Arc<tokio::sync::Mutex<HashMap<String, VolRegimeState>>>,
    tickers: &[String],
) {
    for ticker in tickers {
        let symbol = ticker_to_binance_symbol(ticker);
        match fetch_binance_5m_history(symbol, RV_HISTORY_DAYS).await {
            Ok(candles) => {
                let samples = hourly_rv_from_5m(&candles);
                if samples.is_empty() {
                    warn!("vol-regime seed: no samples for {}", ticker);
                    continue;
                }
                let mut state = VolRegimeState {
                    engine: VolRegimeEngine::default(),
                    last_hour: Some(Utc::now().timestamp() / 3600),
                };
                for rv in samples {
                    state.engine.push_rv(rv);
                }
                let sample_count = state.engine.rv_sample_count();
                info!(
                    "vol-regime seed: {} samples for {}",
                    sample_count, ticker
                );
                vol_states.lock().await.insert(ticker.clone(), state);
            }
            Err(e) => {
                warn!("vol-regime seed failed for {}: {}", ticker, e);
            }
        }
    }
}

fn vol_regime_label(regime: VolRegime) -> &'static str {
    match regime {
        VolRegime::Low => "low",
        VolRegime::Normal => "normal",
        VolRegime::High => "high",
        VolRegime::Extreme => "extreme",
    }
}

async fn run_binance_kline_stream(
    ticker: String,
    tx_trades: Arc<broadcast::Sender<Bytes>>,
    tx_uds: Arc<broadcast::Sender<Bytes>>,
    broadcast_config: Arc<BroadcastConfig>,
    kline_cache: Arc<tokio::sync::Mutex<HashMap<String, VecDeque<Candle1m>>>>,
    agg_tx: Arc<mpsc::Sender<MarketEventMessage>>,
) {
    let symbol = ticker_to_binance_symbol(&ticker).to_lowercase();
    let url = format!("wss://fstream.binance.com/ws/{}@kline_1m", symbol);

    loop {
        match connect_async(&url).await {
            Ok((ws_stream, _)) => {
                let (_, mut read) = ws_stream.split();
                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                                if let Some(k) = v.get("k") {
                                    let is_final = k.get("x").and_then(|b| b.as_bool()).unwrap_or(false);
                                    if !is_final {
                                        continue;
                                    }
                                    if let (Some(start_ms), Some(open), Some(high), Some(low), Some(close), Some(vol)) = (
                                        k.get("t").and_then(|v| v.as_i64()),
                                        k.get("o").and_then(|v| v.as_str()),
                                        k.get("h").and_then(|v| v.as_str()),
                                        k.get("l").and_then(|v| v.as_str()),
                                        k.get("c").and_then(|v| v.as_str()),
                                        k.get("v").and_then(|v| v.as_str()),
                                    ) {
                                        if let Some(start_time) = chrono::DateTime::from_timestamp_millis(start_ms) {
                                            if let (Ok(o), Ok(h), Ok(l), Ok(c), Ok(volume)) =
                                                (open.parse::<f64>(), high.parse::<f64>(), low.parse::<f64>(), close.parse::<f64>(), vol.parse::<f64>())
                                            {
                                                let candle = Candle1m {
                                                    open: o,
                                                    high: h,
                                                    low: l,
                                                    close: c,
                                                    volume,
                                                    start_time,
                                                    is_complete: true,
                                                };
                                                {
                                                    let mut cache = kline_cache.lock().await;
                                                    let entry = cache.entry(ticker.clone()).or_insert_with(VecDeque::new);
                                                    entry.push_back(candle.clone());
                                                    while entry.len() > 300 {
                                                        entry.pop_front();
                                                    }
                                                }

                                                let event = MarketEventMessage {
                                                    time_exchange: Utc::now(),
                                                    time_received: Utc::now(),
                                                    exchange: "BinanceFuturesUsd".to_string(),
                                                    instrument: InstrumentInfo {
                                                        base: ticker.clone(),
                                                        quote: "USDT".to_string(),
                                                        kind: "Perpetual".to_string(),
                                                    },
                                                    kind: "candle_1m".to_string(),
                                                    data: serde_json::to_value(&candle).unwrap_or_default(),
                                                };
                                                if let Some(bytes) = serialize_for_broadcast(&broadcast_config, event.clone()) {
                                                    let _ = tx_trades.send(bytes);
                                                }
                                                if let Some(frame) = serialize_for_uds(&event) {
                                                    let _ = tx_uds.send(frame);
                                                }
                                                // Send to aggregator via channel (no mutex!)
                                                let _ = agg_tx.try_send(event);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
                        Ok(Message::Close(_)) => break,
                        Err(_) => break,
                        _ => {}
                    }
                }
            }
            Err(e) => {
                warn!("[kline-ws] {} connect error: {}", ticker, e);
            }
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn fetch_deribit_options_chain(
    client: &Client,
    base_url: &str,
    currency: &str,
    top_n: usize,
) -> Result<OptionsChain, String> {
    let instruments = fetch_deribit_instruments(client, base_url, currency).await?;
    let summaries = fetch_deribit_book_summaries(client, base_url, currency).await?;

    let mut summary_map: HashMap<String, DeribitBookSummary> = HashMap::new();
    for summary in summaries {
        summary_map.insert(summary.instrument_name.clone(), summary);
    }

    let mut contracts: Vec<OptionContract> = Vec::with_capacity(instruments.len());
    for instrument in instruments {
        let summary = summary_map
            .get(&instrument.instrument_name)
            .cloned()
            .unwrap_or(DeribitBookSummary {
                instrument_name: instrument.instrument_name.clone(),
                open_interest: Some(0.0),
                mark_iv: Some(0.0),
            });

        let is_call = instrument.option_type.to_lowercase() == "call";
        contracts.push(OptionContract {
            instrument_name: instrument.instrument_name,
            strike: instrument.strike,
            expiry: instrument.expiration_timestamp,
            is_call,
            open_interest: summary.open_interest.unwrap_or(0.0),
            mark_iv: summary.mark_iv.unwrap_or(0.0),
            delta: 0.0,
            gamma: 0.0,
            vega: 0.0,
        });
    }

    contracts.sort_by(|a, b| b.open_interest.partial_cmp(&a.open_interest).unwrap_or(std::cmp::Ordering::Equal));
    let top_instruments: Vec<String> = contracts
        .iter()
        .take(top_n)
        .map(|c| c.instrument_name.clone())
        .collect();

    let mut greeks_map: HashMap<String, DeribitGreeks> = HashMap::new();
    let mut futures = Vec::with_capacity(top_instruments.len());
    for instrument_name in top_instruments {
        futures.push(fetch_deribit_ticker(client, base_url, instrument_name));
    }

    for result in futures::future::join_all(futures).await {
        if let Ok(ticker) = result {
            if let Some(greeks) = ticker.greeks {
                greeks_map.insert(ticker.instrument_name, greeks);
            }
        }
    }

    for contract in &mut contracts {
        if let Some(greeks) = greeks_map.get(&contract.instrument_name) {
            contract.delta = greeks.delta;
            contract.gamma = greeks.gamma;
            contract.vega = greeks.vega;
        }
    }

    Ok(OptionsChain {
        contracts,
        timestamp: Utc::now().timestamp_millis(),
    })
}

async fn fetch_deribit_instruments(
    client: &Client,
    base_url: &str,
    currency: &str,
) -> Result<Vec<DeribitInstrument>, String> {
    let url = format!(
        "{}/get_instruments?currency={}&kind=option&expired=false",
        base_url, currency
    );
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json::<DeribitResponse<Vec<DeribitInstrument>>>()
        .await
        .map_err(|e| e.to_string())?;
    Ok(resp.result)
}

async fn fetch_deribit_book_summaries(
    client: &Client,
    base_url: &str,
    currency: &str,
) -> Result<Vec<DeribitBookSummary>, String> {
    let url = format!(
        "{}/get_book_summary_by_currency?currency={}&kind=option",
        base_url, currency
    );
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json::<DeribitResponse<Vec<DeribitBookSummary>>>()
        .await
        .map_err(|e| e.to_string())?;
    Ok(resp.result)
}

async fn fetch_deribit_ticker(
    client: &Client,
    base_url: &str,
    instrument_name: String,
) -> Result<DeribitTicker, String> {
    let url = format!(
        "{}/ticker?instrument_name={}",
        base_url, instrument_name
    );
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json::<DeribitResponse<DeribitTicker>>()
        .await
        .map_err(|e| e.to_string())?;
    Ok(resp.result)
}

/// Handle individual WebSocket client connection
async fn handle_client(
    stream: TcpStream,
    peer_addr: SocketAddr,
    tx_trades: Arc<broadcast::Sender<Bytes>>,
    tx_l2: Arc<broadcast::Sender<Bytes>>,
    kline_cache: Arc<tokio::sync::Mutex<HashMap<String, VecDeque<Candle1m>>>>,
    broadcast_config: Arc<BroadcastConfig>,
) {
    let ws_stream = match accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            error!("WebSocket handshake failed for {}: {}", peer_addr, e);
            return;
        }
    };

    info!("WebSocket handshake completed for {}", peer_addr);

    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    let mut rx_trades = tx_trades.subscribe();
    let mut rx_l2 = tx_l2.subscribe();
    // Note: envelope/source config is now handled at broadcast time (serialize_for_broadcast)

    // Send welcome message (respects WS_BINARY_FRAMES config)
    let welcome = serde_json::json!({
        "type": "welcome",
        "message": "Connected to barter-data market feed",
        "timestamp": Utc::now()
    });
    if let Ok(msg) = serde_json::to_string(&welcome) {
        let welcome_msg = if broadcast_config.use_binary_frames {
            Message::Binary(Bytes::from(msg.into_bytes()))
        } else {
            Message::Text(msg.into())
        };
        let _ = ws_sender.send(welcome_msg).await;
    }

    // Send kline backfill from cache (refresh is done by background task)
    // Uses whatever is in cache - no blocking network calls during handshake
    {
        let cache = kline_cache.lock().await;
        for (ticker, candles) in cache.iter() {
            let payload = CandleBackfill {
                ticker: ticker.clone(),
                candles: candles.iter().cloned().collect(),
            };
            let event = MarketEventMessage {
                time_exchange: Utc::now(),
                time_received: Utc::now(),
                exchange: "barter-data-server".to_string(),
                instrument: InstrumentInfo {
                    base: ticker.clone(),
                    quote: "USD".to_string(),
                    kind: "Kline".to_string(),
                },
                kind: "candle_backfill".to_string(),
                data: serde_json::to_value(&payload).unwrap_or_default(),
            };
            if let Some(bytes) = serialize_for_broadcast(&broadcast_config, event) {
                let backfill_msg = if broadcast_config.use_binary_frames {
                    Message::Binary(bytes)
                } else {
                    // Safe: serialize_for_broadcast produces valid UTF-8
                    Message::Text(String::from_utf8_lossy(&bytes).into_owned().into())
                };
                let _ = ws_sender.send(backfill_msg).await;
            }
        }
    }

    // Spawn task to send market events to this client
    // Uses biased select! to prioritize trades over L2
    let use_binary = broadcast_config.use_binary_frames;
    let mut send_task = tokio::spawn(async move {
        loop {
            // Biased select: trades always checked first (hot path priority)
            tokio::select! {
                biased;

                // PRIORITY 1: Trades, liquidations, OI, CVD, L1 (hot path)
                // Messages are pre-serialized at broadcast time - just forward them
                result = rx_trades.recv() => {
                    match result {
                        Ok(bytes) => {
                            // Respects WS_BINARY_FRAMES config
                            let msg = if use_binary {
                                Message::Binary(bytes)
                            } else {
                                Message::Text(String::from_utf8_lossy(&bytes).into_owned().into())
                            };
                            if ws_sender.send(msg).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            // Trade lag is concerning - log at warn level
                            warn!("Client {} trade channel lagged, skipped {} messages", peer_addr, skipped);
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("Trade channel closed for {}", peer_addr);
                            break;
                        }
                    }
                }

                // PRIORITY 2: L2 orderbook (lower priority, can lag)
                result = rx_l2.recv() => {
                    match result {
                        Ok(bytes) => {
                            // Respects WS_BINARY_FRAMES config
                            let msg = if use_binary {
                                Message::Binary(bytes)
                            } else {
                                Message::Text(String::from_utf8_lossy(&bytes).into_owned().into())
                            };
                            if ws_sender.send(msg).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            // L2 lag is OK - just log at debug level
                            debug!("Client {} L2 channel lagged, skipped {} messages", peer_addr, skipped);
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            // L2 channel closed but trades still work
                            debug!("L2 channel closed for {}", peer_addr);
                            // Don't break - continue receiving trades
                        }
                    }
                }
            }
        }
    });

    // Handle incoming messages from client (e.g., ping/pong)
    let mut recv_task = tokio::spawn(async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Close(_)) => break,
                Ok(Message::Ping(_)) => {
                    // Tungstenite handles pong automatically, but log it
                    debug!("Received ping from {}", peer_addr);
                }
                Ok(Message::Text(text)) => {
                    debug!("Received text from {}: {}", peer_addr, text);
                }
                Err(e) => {
                    error!("WebSocket error for {}: {}", peer_addr, e);
                    break;
                }
                _ => {}
            }
        }
    });

    // Wait for either task to complete
    tokio::select! {
        _ = &mut send_task => {
            info!("Send task completed for {}", peer_addr);
        }
        _ = &mut recv_task => {
            info!("Receive task completed for {}", peer_addr);
        }
    }

    info!("WebSocket connection closed for {}", peer_addr);
}

/// Initialize market data streams (same as the example)
async fn init_market_streams() -> DynamicStreams<MarketDataInstrument> {
    use ExchangeId::*;
    use MarketDataInstrumentKind::*;
    use SubKind::*;
    use barter_data::subscription::SubKind;

    DynamicStreams::init([
        // === SPOT SUBSCRIPTIONS (for basis calculation) ===
        // Bybit Spot
        vec![
            (BybitSpot, "btc", "usdt", Spot, OrderBooksL1),
            (BybitSpot, "eth", "usdt", Spot, OrderBooksL1),
            (BybitSpot, "sol", "usdt", Spot, OrderBooksL1),
        ],
        vec![
            (BybitSpot, "btc", "usdt", Spot, PublicTrades),
            (BybitSpot, "eth", "usdt", Spot, PublicTrades),
            (BybitSpot, "sol", "usdt", Spot, PublicTrades),
        ],
        // Binance Spot
        vec![
            (BinanceSpot, "btc", "usdt", Spot, OrderBooksL1),
            (BinanceSpot, "eth", "usdt", Spot, OrderBooksL1),
            (BinanceSpot, "sol", "usdt", Spot, OrderBooksL1),
        ],
        vec![
            (BinanceSpot, "btc", "usdt", Spot, PublicTrades),
            (BinanceSpot, "eth", "usdt", Spot, PublicTrades),
            (BinanceSpot, "sol", "usdt", Spot, PublicTrades),
        ],
        // OKX Spot (OrderBooksL1 unsupported) -> skip L1, keep trades for basis estimation
        vec![
            (Okx, "btc", "usdt", Spot, PublicTrades),
            (Okx, "eth", "usdt", Spot, PublicTrades),
            (Okx, "sol", "usdt", Spot, PublicTrades),
        ],
        // === PERPETUAL SUBSCRIPTIONS ===
        // BTC Perpetuals
        vec![(BybitPerpetualsUsd, "btc", "usdt", Perpetual, OpenInterest)],
        vec![(BybitPerpetualsUsd, "btc", "usdt", Perpetual, Liquidations)],
        vec![(
            BybitPerpetualsUsd,
            "btc",
            "usdt",
            Perpetual,
            CumulativeVolumeDelta,
        )],
        vec![(BinanceFuturesUsd, "btc", "usdt", Perpetual, Liquidations)],
        vec![(
            BinanceFuturesUsd,
            "btc",
            "usdt",
            Perpetual,
            CumulativeVolumeDelta,
        )],
        vec![(Okx, "btc", "usdt", Perpetual, OpenInterest)],
        vec![(Okx, "btc", "usdt", Perpetual, Liquidations)],
        vec![(Okx, "btc", "usdt", Perpetual, CumulativeVolumeDelta)],
        vec![(BinanceFuturesUsd, "btc", "usdt", Perpetual, OrderBooksL1)],
        vec![(BybitPerpetualsUsd, "btc", "usdt", Perpetual, OrderBooksL1)],
        // BTC L2 Orderbook (separate WS connections due to high volume)
        vec![(BinanceFuturesUsd, "btc", "usdt", Perpetual, OrderBooksL2)],
        vec![(BybitPerpetualsUsd, "btc", "usdt", Perpetual, OrderBooksL2)],
        vec![(Okx, "btc", "usdt", Perpetual, OrderBooksL2)],
        vec![(BinanceFuturesUsd, "btc", "usdt", Perpetual, PublicTrades)],
        vec![(BybitPerpetualsUsd, "btc", "usdt", Perpetual, PublicTrades)],
        vec![(Okx, "btc", "usdt", Perpetual, PublicTrades)],
        // ETH Perpetuals
        vec![(BybitPerpetualsUsd, "eth", "usdt", Perpetual, OpenInterest)],
        vec![(BybitPerpetualsUsd, "eth", "usdt", Perpetual, Liquidations)],
        vec![(
            BybitPerpetualsUsd,
            "eth",
            "usdt",
            Perpetual,
            CumulativeVolumeDelta,
        )],
        vec![(BinanceFuturesUsd, "eth", "usdt", Perpetual, Liquidations)],
        vec![(
            BinanceFuturesUsd,
            "eth",
            "usdt",
            Perpetual,
            CumulativeVolumeDelta,
        )],
        vec![(Okx, "eth", "usdt", Perpetual, OpenInterest)],
        vec![(Okx, "eth", "usdt", Perpetual, Liquidations)],
        vec![(Okx, "eth", "usdt", Perpetual, CumulativeVolumeDelta)],
        vec![(BinanceFuturesUsd, "eth", "usdt", Perpetual, OrderBooksL1)],
        vec![(BybitPerpetualsUsd, "eth", "usdt", Perpetual, OrderBooksL1)],
        // ETH L2 Orderbook
        vec![(BinanceFuturesUsd, "eth", "usdt", Perpetual, OrderBooksL2)],
        vec![(BybitPerpetualsUsd, "eth", "usdt", Perpetual, OrderBooksL2)],
        vec![(Okx, "eth", "usdt", Perpetual, OrderBooksL2)],
        vec![(BinanceFuturesUsd, "eth", "usdt", Perpetual, PublicTrades)],
        vec![(BybitPerpetualsUsd, "eth", "usdt", Perpetual, PublicTrades)],
        vec![(Okx, "eth", "usdt", Perpetual, PublicTrades)],
        // SOL Perpetuals
        vec![(BybitPerpetualsUsd, "sol", "usdt", Perpetual, OpenInterest)],
        vec![(BybitPerpetualsUsd, "sol", "usdt", Perpetual, Liquidations)],
        vec![(
            BybitPerpetualsUsd,
            "sol",
            "usdt",
            Perpetual,
            CumulativeVolumeDelta,
        )],
        vec![(BinanceFuturesUsd, "sol", "usdt", Perpetual, Liquidations)],
        vec![(
            BinanceFuturesUsd,
            "sol",
            "usdt",
            Perpetual,
            CumulativeVolumeDelta,
        )],
        vec![(Okx, "sol", "usdt", Perpetual, OpenInterest)],
        vec![(Okx, "sol", "usdt", Perpetual, Liquidations)],
        vec![(Okx, "sol", "usdt", Perpetual, CumulativeVolumeDelta)],
        vec![(BinanceFuturesUsd, "sol", "usdt", Perpetual, OrderBooksL1)],
        vec![(BybitPerpetualsUsd, "sol", "usdt", Perpetual, OrderBooksL1)],
        // SOL L2 Orderbook
        vec![(BinanceFuturesUsd, "sol", "usdt", Perpetual, OrderBooksL2)],
        vec![(BybitPerpetualsUsd, "sol", "usdt", Perpetual, OrderBooksL2)],
        vec![(Okx, "sol", "usdt", Perpetual, OrderBooksL2)],
        vec![(BinanceFuturesUsd, "sol", "usdt", Perpetual, PublicTrades)],
        vec![(BybitPerpetualsUsd, "sol", "usdt", Perpetual, PublicTrades)],
        vec![(Okx, "sol", "usdt", Perpetual, PublicTrades)],
    ])
    .await
    .expect("Failed to initialize market streams")
}

/// (unused) dedicated liquidation stream builder -- kept for reference
/// NOTE: not used in the main pipeline; DynamicStreams already carries liquidations.
// async fn init_liquidation_streams()
// -> Streams<MarketEvent<MarketDataInstrument, barter_data::subscription::liquidation::Liquidation>> {
//     use ExchangeId::*;
//     use MarketDataInstrumentKind::*;
//
//     Streams::builder::<MarketDataInstrument, Liquidations>()
//         .subscribe([
//             (
//                 BinanceFuturesUsd::default(),
//                 "btc",
//                 "usdt",
//                 Perpetual,
//                 Liquidations,
//             ),
//             (
//                 BybitPerpetualsUsd::default(),
//                 "btc",
//                 "usdt",
//                 Perpetual,
//                 Liquidations,
//             ),
//             (Okx::default(), "btc", "usdt", Perpetual, Liquidations),
//         ])
//         .subscribe([
//             (
//                 BinanceFuturesUsd::default(),
//                 "eth",
//                 "usdt",
//                 Perpetual,
//                 Liquidations,
//             ),
//             (
//                 BybitPerpetualsUsd::default(),
//                 "eth",
//                 "usdt",
//                 Perpetual,
//                 Liquidations,
//             ),
//             (Okx::default(), "eth", "usdt", Perpetual, Liquidations),
//         ])
//         .subscribe([
//             (
//                 BinanceFuturesUsd::default(),
//                 "sol",
//                 "usdt",
//                 Perpetual,
//                 Liquidations,
//             ),
//             (
//                 BybitPerpetualsUsd::default(),
//                 "sol",
//                 "usdt",
//                 Perpetual,
//                 Liquidations,
//             ),
//             (Okx::default(), "sol", "usdt", Perpetual, Liquidations),
//         ])
//         .init()
//         .await
//         .expect("Failed to init liquidation streams")
// }

/// Binance REST API response for open interest
#[derive(Debug, Deserialize)]
struct BinanceOpenInterestResponse {
    #[serde(
        rename = "openInterest",
        deserialize_with = "barter_integration::de::de_str"
    )]
    open_interest: f64,
    time: i64,
}

#[derive(Debug, Deserialize)]
struct BinanceFundingRateResponse {
    #[serde(rename = "lastFundingRate")]
    last_funding_rate: String,
    #[serde(rename = "nextFundingTime")]
    next_funding_time: i64,
    #[serde(rename = "time")]
    time: i64,
}

#[derive(Debug, Deserialize)]
struct BybitFundingRateResponse {
    #[serde(rename = "retCode")]
    ret_code: i64,
    result: BybitFundingRateResult,
    time: i64,
}

#[derive(Debug, Deserialize)]
struct BybitFundingRateResult {
    #[serde(default)]
    #[allow(dead_code)]
    category: String,
    list: Vec<BybitFundingRateEntry>,
}

#[derive(Debug, Deserialize)]
struct BybitFundingRateEntry {
    #[serde(rename = "fundingRate")]
    funding_rate: String,
    #[serde(rename = "nextFundingTime")]
    next_funding_time: String,
    symbol: String,
}

#[derive(Debug, Deserialize)]
struct OkxFundingRateResponse {
    data: Vec<OkxFundingRateEntry>,
}

#[derive(Debug, Deserialize)]
struct OkxFundingRateEntry {
    #[serde(rename = "fundingRate")]
    funding_rate: String,
    #[serde(rename = "fundingTime")]
    funding_time: String,
    #[serde(rename = "nextFundingTime")]
    next_funding_time: String,
}

fn funding_poll_interval() -> Duration {
    Duration::from_secs(
        std::env::var("FUNDING_POLL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30),
    )
}

fn parse_f64(value: &str) -> Result<f64, DataError> {
    value
        .parse::<f64>()
        .map_err(|err| DataError::Socket(format!("Funding rate parse failed: {err}")))
}

fn parse_i64(value: &str) -> Result<i64, DataError> {
    value
        .parse::<i64>()
        .map_err(|err| DataError::Socket(format!("Funding time parse failed: {err}")))
}

/// Build a combined Stream of Binance open-interest polling events (REST fallback)
fn binance_open_interest_stream()
-> impl futures::Stream<Item = MarketStreamResult<MarketDataInstrument, DataKind>> {
    let specs = vec![
        (
            "BTCUSDT",
            MarketDataInstrument::from(("btc", "usdt", MarketDataInstrumentKind::Perpetual)),
        ),
        (
            "ETHUSDT",
            MarketDataInstrument::from(("eth", "usdt", MarketDataInstrumentKind::Perpetual)),
        ),
        (
            "SOLUSDT",
            MarketDataInstrument::from(("sol", "usdt", MarketDataInstrumentKind::Perpetual)),
        ),
        (
            "XRPUSDT",
            MarketDataInstrument::from(("xrp", "usdt", MarketDataInstrumentKind::Perpetual)),
        ),
    ];

    stream::select_all(
        specs
            .into_iter()
            .map(|(symbol, instrument)| binance_open_interest_poller(symbol, instrument).boxed())
            .collect::<Vec<_>>(),
    )
}

/// Build a combined Stream of funding-rate polling events (REST)
fn funding_rate_stream()
-> impl futures::Stream<Item = MarketStreamResult<MarketDataInstrument, DataKind>> {
    let specs = vec![
        (
            "BTCUSDT",
            MarketDataInstrument::from(("btc", "usdt", MarketDataInstrumentKind::Perpetual)),
        ),
        (
            "ETHUSDT",
            MarketDataInstrument::from(("eth", "usdt", MarketDataInstrumentKind::Perpetual)),
        ),
        (
            "SOLUSDT",
            MarketDataInstrument::from(("sol", "usdt", MarketDataInstrumentKind::Perpetual)),
        ),
    ];

    let mut streams: Vec<
        futures::stream::BoxStream<
            'static,
            MarketStreamResult<MarketDataInstrument, DataKind>,
        >,
    > = Vec::new();

    for (symbol, instrument) in &specs {
        streams.push(binance_funding_rate_poller(symbol, instrument.clone()).boxed());
        streams.push(bybit_funding_rate_poller(symbol, instrument.clone()).boxed());
    }

    let okx_specs = vec![
        ("BTC-USDT-SWAP", "btc"),
        ("ETH-USDT-SWAP", "eth"),
        ("SOL-USDT-SWAP", "sol"),
    ];
    for (symbol, base) in okx_specs {
        let instrument =
            MarketDataInstrument::from((base, "usdt", MarketDataInstrumentKind::Perpetual));
        streams.push(okx_funding_rate_poller(symbol, instrument).boxed());
    }

    stream::select_all(streams)
}

fn binance_funding_rate_poller(
    symbol: &'static str,
    instrument: MarketDataInstrument,
) -> impl futures::Stream<Item = MarketStreamResult<MarketDataInstrument, DataKind>> + Send {
    let client = Client::new();
    let url = format!(
        "https://fapi.binance.com/fapi/v1/premiumIndex?symbol={}",
        symbol
    );

    stream::unfold(
        (client, url, interval(funding_poll_interval()), instrument),
        move |(client, url, mut timer, instrument)| async move {
            timer.tick().await;

            let instrument_clone = instrument.clone();
            let result: Result<MarketEvent<MarketDataInstrument, DataKind>, DataError> =
                match client.get(&url).send().await {
                    Ok(response) => {
                        if let Err(status_err) = response.error_for_status_ref() {
                            Err(DataError::Socket(format!(
                                "Binance funding poll failed ({symbol}): {status_err}"
                            )))
                        } else {
                            match response.json::<BinanceFundingRateResponse>().await {
                                Ok(data) => {
                                    match parse_f64(&data.last_funding_rate) {
                                        Ok(rate) => {
                                            let time_exchange =
                                                DateTime::from_timestamp_millis(data.time)
                                                    .unwrap_or_else(Utc::now);
                                            let next_time =
                                                DateTime::from_timestamp_millis(data.next_funding_time);
                                            debug!("Binance funding {symbol}: rate={rate}");
                                            Ok(MarketEvent {
                                                time_exchange,
                                                time_received: Utc::now(),
                                                exchange: ExchangeId::BinanceFuturesUsd,
                                                instrument: instrument_clone,
                                                kind: DataKind::FundingRate(FundingRate {
                                                    rate,
                                                    time: Some(time_exchange),
                                                    next_time,
                                                }),
                                            })
                                        }
                                        Err(e) => Err(DataError::Socket(format!(
                                            "Binance funding rate parse error ({symbol}): {e:?}"
                                        ))),
                                    }
                                }
                                Err(parse_err) => Err(DataError::Socket(format!(
                                    "Binance funding parse failed ({symbol}): {parse_err}"
                                ))),
                            }
                        }
                    }
                    Err(request_err) => Err(DataError::Socket(format!(
                        "Binance funding request failed ({symbol}): {request_err}"
                    ))),
                };

            Some((Event::Item(result), (client, url, timer, instrument)))
        },
    )
}

fn bybit_funding_rate_poller(
    symbol: &'static str,
    instrument: MarketDataInstrument,
) -> impl futures::Stream<Item = MarketStreamResult<MarketDataInstrument, DataKind>> + Send {
    let client = Client::new();
    let url = format!(
        "https://api.bybit.com/v5/market/tickers?category=linear&symbol={}",
        symbol
    );

    stream::unfold(
        (client, url, interval(funding_poll_interval()), instrument),
        move |(client, url, mut timer, instrument)| async move {
            timer.tick().await;

            let instrument_clone = instrument.clone();
            let result: Result<MarketEvent<MarketDataInstrument, DataKind>, DataError> =
                match client.get(&url).send().await {
                    Ok(response) => {
                        if let Err(status_err) = response.error_for_status_ref() {
                            Err(DataError::Socket(format!(
                                "Bybit funding poll failed ({symbol}): {status_err}"
                            )))
                        } else {
                            match response.json::<BybitFundingRateResponse>().await {
                                Ok(data) => {
                                    if data.ret_code != 0 {
                                        return Some((
                                            Event::Item(Err(DataError::Socket(format!(
                                                "Bybit funding error ({symbol}): retCode {}",
                                                data.ret_code
                                            )))),
                                            (client, url, timer, instrument),
                                        ));
                                    }
                                    let entry = data
                                        .result
                                        .list
                                        .iter()
                                        .find(|item| item.symbol == symbol)
                                        .or_else(|| data.result.list.first());
                                    if let Some(entry) = entry {
                                        match (parse_f64(&entry.funding_rate), parse_i64(&entry.next_funding_time)) {
                                            (Ok(rate), Ok(next_time_ms)) => {
                                                let time_exchange =
                                                    DateTime::from_timestamp_millis(data.time)
                                                        .unwrap_or_else(Utc::now);
                                                let next_time =
                                                    DateTime::from_timestamp_millis(next_time_ms);
                                                debug!("Bybit funding {symbol}: rate={rate}");
                                                Ok(MarketEvent {
                                                    time_exchange,
                                                    time_received: Utc::now(),
                                                    exchange: ExchangeId::BybitPerpetualsUsd,
                                                    instrument: instrument_clone,
                                                    kind: DataKind::FundingRate(FundingRate {
                                                        rate,
                                                        time: Some(time_exchange),
                                                        next_time,
                                                    }),
                                                })
                                            }
                                            _ => Err(DataError::Socket(format!(
                                                "Bybit funding parse error ({symbol})"
                                            ))),
                                        }
                                    } else {
                                        Err(DataError::Socket(format!(
                                            "Bybit funding missing data ({symbol})"
                                        )))
                                    }
                                }
                                Err(parse_err) => Err(DataError::Socket(format!(
                                    "Bybit funding parse failed ({symbol}): {parse_err}"
                                ))),
                            }
                        }
                    }
                    Err(request_err) => Err(DataError::Socket(format!(
                        "Bybit funding request failed ({symbol}): {request_err}"
                    ))),
                };

            Some((Event::Item(result), (client, url, timer, instrument)))
        },
    )
}

fn okx_funding_rate_poller(
    symbol: &'static str,
    instrument: MarketDataInstrument,
) -> impl futures::Stream<Item = MarketStreamResult<MarketDataInstrument, DataKind>> + Send {
    let client = Client::new();
    let url = format!(
        "https://www.okx.com/api/v5/public/funding-rate?instId={}",
        symbol
    );

    stream::unfold(
        (client, url, interval(funding_poll_interval()), instrument),
        move |(client, url, mut timer, instrument)| async move {
            timer.tick().await;

            let instrument_clone = instrument.clone();
            let result: Result<MarketEvent<MarketDataInstrument, DataKind>, DataError> =
                match client.get(&url).send().await {
                    Ok(response) => {
                        if let Err(status_err) = response.error_for_status_ref() {
                            Err(DataError::Socket(format!(
                                "OKX funding poll failed ({symbol}): {status_err}"
                            )))
                        } else {
                            match response.json::<OkxFundingRateResponse>().await {
                                Ok(data) => {
                                    if let Some(entry) = data.data.first() {
                                        match (parse_f64(&entry.funding_rate), parse_i64(&entry.funding_time), parse_i64(&entry.next_funding_time)) {
                                            (Ok(rate), Ok(funding_time_ms), Ok(next_time_ms)) => {
                                                let time_exchange =
                                                    DateTime::from_timestamp_millis(funding_time_ms)
                                                        .unwrap_or_else(Utc::now);
                                                let next_time =
                                                    DateTime::from_timestamp_millis(next_time_ms);
                                                debug!("OKX funding {symbol}: rate={rate}");
                                                Ok(MarketEvent {
                                                    time_exchange,
                                                    time_received: Utc::now(),
                                                    exchange: ExchangeId::Okx,
                                                    instrument: instrument_clone,
                                                    kind: DataKind::FundingRate(FundingRate {
                                                        rate,
                                                        time: Some(time_exchange),
                                                        next_time,
                                                    }),
                                                })
                                            }
                                            _ => Err(DataError::Socket(format!(
                                                "OKX funding parse error ({symbol})"
                                            ))),
                                        }
                                    } else {
                                        Err(DataError::Socket(format!(
                                            "OKX funding missing data ({symbol})"
                                        )))
                                    }
                                }
                                Err(parse_err) => Err(DataError::Socket(format!(
                                    "OKX funding parse failed ({symbol}): {parse_err}"
                                ))),
                            }
                        }
                    }
                    Err(request_err) => Err(DataError::Socket(format!(
                        "OKX funding request failed ({symbol}): {request_err}"
                    ))),
                };

            Some((Event::Item(result), (client, url, timer, instrument)))
        },
    )
}

/// Poll Binance REST API for open interest every 10 seconds
fn binance_open_interest_poller(
    symbol: &'static str,
    instrument: MarketDataInstrument,
) -> impl futures::Stream<Item = MarketStreamResult<MarketDataInstrument, DataKind>> + Send {
    let client = Client::new();
    let url = format!(
        "https://fapi.binance.com/fapi/v1/openInterest?symbol={}",
        symbol
    );

    stream::unfold(
        (
            client,
            url,
            interval(std::time::Duration::from_secs(5)), // Poll every 5s for fresher OI data
            instrument,
        ),
        move |(client, url, mut timer, instrument)| async move {
            timer.tick().await;

            let instrument_clone = instrument.clone();

            let result: Result<MarketEvent<MarketDataInstrument, DataKind>, DataError> =
                match client.get(&url).send().await {
                    Ok(response) => {
                        if let Err(status_err) = response.error_for_status_ref() {
                            Err(DataError::Socket(format!(
                                "Binance open interest poll failed ({symbol}): {status_err}"
                            )))
                        } else {
                            match response.json::<BinanceOpenInterestResponse>().await {
                                Ok(data) => {
                                    let time_exchange = DateTime::from_timestamp_millis(data.time)
                                        .unwrap_or_else(Utc::now);

                                    Ok(MarketEvent {
                                        time_exchange,
                                        time_received: Utc::now(),
                                        exchange: ExchangeId::BinanceFuturesUsd,
                                        instrument: instrument_clone,
                                        kind: DataKind::OpenInterest(OpenInterest {
                                            contracts: data.open_interest,
                                            notional: None,
                                            time: Some(time_exchange),
                                        }),
                                    })
                                }
                                Err(parse_err) => Err(DataError::Socket(format!(
                                    "Binance open interest parse failed ({symbol}): {parse_err}"
                                ))),
                            }
                        }
                    }
                    Err(request_err) => Err(DataError::Socket(format!(
                        "Binance open interest request failed ({symbol}): {request_err}"
                    ))),
                };

            Some((Event::Item(result), (client, url, timer, instrument)))
        },
    )
}

/// Dedicated aggregator task - processes events without blocking the hot path
/// Publishes snapshots to watch channel for consumers
async fn run_aggregator_task(
    mut event_rx: mpsc::Receiver<MarketEventMessage>,
    snapshot_tx: watch::Sender<AggregatedSnapshot>,
) {
    let mut aggregator = Aggregator::new();
    let mut snapshot_interval = interval(Duration::from_millis(100)); // Publish snapshots at 10Hz
    snapshot_interval.tick().await; // Skip first immediate tick

    loop {
        tokio::select! {
            biased;

            // Process incoming events (priority)
            Some(event) = event_rx.recv() => {
                aggregator.process_event(event);
            }

            // Periodically publish snapshot
            _ = snapshot_interval.tick() => {
                let snapshot = aggregator.snapshot();
                // send() only fails if all receivers dropped - that's OK
                let _ = snapshot_tx.send(snapshot);
            }
        }
    }
}

/// Initialize logging
fn init_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_event() -> MarketEventMessage {
        MarketEventMessage {
            time_exchange: Utc::now(),
            time_received: Utc::now(),
            exchange: "TestExchange".to_string(),
            instrument: InstrumentInfo {
                base: "BTC".to_string(),
                quote: "USD".to_string(),
                kind: "Perpetual".to_string(),
            },
            kind: "trade".to_string(),
            data: serde_json::json!({"price": 50000.0, "amount": 1.5}),
        }
    }

    #[test]
    fn test_serialize_for_broadcast_without_envelope() {
        let config = BroadcastConfig {
            use_envelope: false,
            source: "test-source".to_string(),
            use_binary_frames: true,
            l1_throttle_ms: 50,
            spot_log_threshold: 50_000.0,
        };
        let event = test_event();

        let result = serialize_for_broadcast(&config, event.clone());
        assert!(result.is_some(), "Serialization should succeed");

        let bytes = result.unwrap();
        let json_str = String::from_utf8(bytes.to_vec()).unwrap();

        // Verify it's valid JSON and contains expected fields
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["exchange"], "TestExchange");
        assert_eq!(parsed["kind"], "trade");
        assert!(parsed.get("schema_version").is_none(), "Should not have envelope fields");
    }

    #[test]
    fn test_serialize_for_broadcast_with_envelope() {
        let config = BroadcastConfig {
            use_envelope: true,
            source: "test-source".to_string(),
            use_binary_frames: true,
            l1_throttle_ms: 50,
            spot_log_threshold: 50_000.0,
        };
        let event = test_event();

        let result = serialize_for_broadcast(&config, event);
        assert!(result.is_some(), "Serialization should succeed");

        let bytes = result.unwrap();
        let json_str = String::from_utf8(bytes.to_vec()).unwrap();

        // Verify envelope structure
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["schema_version"], 1);
        assert_eq!(parsed["source"], "test-source");
        assert!(parsed.get("time_sent").is_some(), "Should have time_sent");
        assert!(parsed.get("payload").is_some(), "Should have payload");
        assert_eq!(parsed["payload"]["exchange"], "TestExchange");
    }

    #[test]
    fn test_serialize_for_broadcast_produces_valid_utf8() {
        let config = BroadcastConfig {
            use_envelope: false,
            source: "test".to_string(),
            use_binary_frames: true,
            l1_throttle_ms: 50,
            spot_log_threshold: 50_000.0,
        };
        let event = test_event();

        let result = serialize_for_broadcast(&config, event);
        let bytes = result.unwrap();

        // Verify the bytes are valid UTF-8
        assert!(String::from_utf8(bytes.to_vec()).is_ok(), "Should produce valid UTF-8");
    }

    #[test]
    fn test_serialize_for_uds_frame_roundtrip() {
        #[derive(Deserialize)]
        enum UdsMessage {
            Event(MarketEventMessage),
        }

        let event = test_event();
        let frame = serialize_for_uds(&event).expect("UDS serialization should succeed");
        let bytes = frame.to_vec();

        assert!(bytes.len() >= 4, "Frame should include length prefix");
        let mut len_buf = [0_u8; 4];
        len_buf.copy_from_slice(&bytes[..4]);
        let payload_len = u32::from_be_bytes(len_buf) as usize;
        assert_eq!(payload_len, bytes.len() - 4, "Length prefix should match payload");

        let payload = &bytes[4..];
        let decoded = rmp_serde::from_slice::<UdsMessage>(payload)
            .expect("UDS payload should decode");
        match decoded {
            UdsMessage::Event(decoded_event) => {
                assert_eq!(decoded_event.exchange, "TestExchange");
                assert_eq!(decoded_event.kind, "trade");
            }
        }
    }

    #[test]
    fn test_broadcast_config_from_env_defaults() {
        // Clear env vars to test defaults
        // SAFETY: Only modifying test-specific env vars in single-threaded test
        unsafe {
            std::env::remove_var("WS_ENVELOPE");
            std::env::remove_var("WS_SOURCE");
        }

        let config = BroadcastConfig::from_env();
        assert!(!config.use_envelope, "Default should be no envelope");
        assert_eq!(config.source, "barter-data-server", "Default source");
    }

    #[test]
    fn test_bytes_channel_type() {
        // Verify Bytes can be sent through broadcast channel
        let (tx, mut rx) = broadcast::channel::<Bytes>(10);

        let test_bytes = Bytes::from("test message");
        tx.send(test_bytes.clone()).unwrap();

        let received = rx.blocking_recv().unwrap();
        assert_eq!(received, test_bytes);
    }

    #[test]
    fn test_aggregator_mpsc_channel() {
        // Verify mpsc channel works for aggregator events
        let (tx, mut rx) = mpsc::channel::<MarketEventMessage>(100);

        let event = test_event();
        tx.blocking_send(event.clone()).unwrap();

        let received = rx.blocking_recv().unwrap();
        assert_eq!(received.exchange, "TestExchange");
        assert_eq!(received.kind, "trade");
    }

    #[test]
    fn test_aggregator_watch_channel() {
        // Verify watch channel works for snapshot publishing
        let (tx, rx) = watch::channel(AggregatedSnapshot::default());

        // Initial value should be empty
        let initial = rx.borrow().clone();
        assert!(initial.tickers.is_empty());

        // Create a snapshot with some data
        let mut snapshot = AggregatedSnapshot::default();
        snapshot.tickers.insert("BTC".to_string(), Default::default());

        // Send new snapshot
        tx.send(snapshot).unwrap();

        // Receiver should see new value
        let received = rx.borrow().clone();
        assert!(received.tickers.contains_key("BTC"));
    }

    #[test]
    fn test_try_send_non_blocking() {
        // Verify try_send doesn't block (critical for hot-path)
        let (tx, _rx) = mpsc::channel::<MarketEventMessage>(1);

        let event1 = test_event();
        let event2 = test_event();

        // First send should succeed
        assert!(tx.try_send(event1).is_ok());

        // Second send should fail (channel full) but not block
        assert!(tx.try_send(event2).is_err());
    }

    #[test]
    fn test_feed_health_atomics() {
        // Verify atomic counters work for feed health tracking
        use std::sync::atomic::Ordering;

        // Reset counters
        BINANCE_EVENT_COUNT.store(0, Ordering::Relaxed);
        AGG_CHANNEL_DROPPED.store(0, Ordering::Relaxed);

        // Increment
        BINANCE_EVENT_COUNT.fetch_add(10, Ordering::Relaxed);
        AGG_CHANNEL_DROPPED.fetch_add(2, Ordering::Relaxed);

        // Swap (like metrics logging does)
        let binance = BINANCE_EVENT_COUNT.swap(0, Ordering::Relaxed);
        let dropped = AGG_CHANNEL_DROPPED.swap(0, Ordering::Relaxed);

        assert_eq!(binance, 10);
        assert_eq!(dropped, 2);

        // After swap, should be 0
        assert_eq!(BINANCE_EVENT_COUNT.load(Ordering::Relaxed), 0);
        assert_eq!(AGG_CHANNEL_DROPPED.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_stale_threshold_constant() {
        // Verify stale threshold is reasonable (30s = 30000ms)
        assert_eq!(FEED_STALE_THRESHOLD_MS, 30_000);
    }

    // ========== CRITICAL: Aggregator backpressure tests ==========

    #[test]
    fn test_aggregator_backpressure_counter_increments() {
        use std::sync::atomic::Ordering;

        // Reset the counter
        AGG_CHANNEL_DROPPED.store(0, Ordering::Relaxed);

        // Create a small channel that will overflow
        let (tx, _rx) = mpsc::channel::<MarketEventMessage>(2);

        // Fill the channel
        for _ in 0..2 {
            let _ = tx.try_send(test_event());
        }

        // Now simulate the backpressure handling code pattern
        let event = test_event();
        if tx.try_send(event).is_err() {
            // This is exactly what the hot path does
            AGG_CHANNEL_DROPPED.fetch_add(1, Ordering::Relaxed);
        }

        // Verify counter incremented
        let dropped = AGG_CHANNEL_DROPPED.load(Ordering::Relaxed);
        assert_eq!(dropped, 1, "Should have incremented dropped counter");
    }

    #[test]
    fn test_aggregator_channel_recovers_after_drain() {
        // Note: This test verifies channel behavior, not global counter
        // (global counter is tested separately in test_aggregator_backpressure_counter_increments)

        // Create a small channel
        let (tx, mut rx) = mpsc::channel::<MarketEventMessage>(2);

        // Fill it
        assert!(tx.try_send(test_event()).is_ok(), "First send should succeed");
        assert!(tx.try_send(test_event()).is_ok(), "Second send should succeed");

        // This should fail (channel full)
        assert!(tx.try_send(test_event()).is_err(), "Third send should fail (channel full)");

        // Drain one message
        let _ = rx.try_recv();

        // Now send should succeed (channel has space again)
        assert!(tx.try_send(test_event()).is_ok(), "Should succeed after drain");
    }

    #[test]
    fn test_rate_limited_warning_logic() {
        use std::sync::atomic::Ordering;

        // Test the rate limiting logic for drop warnings
        AGG_LAST_DROP_WARN_MS.store(0, Ordering::Relaxed);

        let now_ms = chrono::Utc::now().timestamp_millis();

        // First warning should trigger (0 is far in the past)
        let last_warn = AGG_LAST_DROP_WARN_MS.load(Ordering::Relaxed);
        let should_warn = now_ms - last_warn > 1000;
        assert!(should_warn, "Should warn on first drop");

        // Update the last warn time
        AGG_LAST_DROP_WARN_MS.store(now_ms, Ordering::Relaxed);

        // Immediate second warning should NOT trigger (< 1 second)
        let now_ms_2 = chrono::Utc::now().timestamp_millis();
        let last_warn_2 = AGG_LAST_DROP_WARN_MS.load(Ordering::Relaxed);
        let should_warn_2 = now_ms_2 - last_warn_2 > 1000;
        assert!(!should_warn_2, "Should NOT warn within 1 second");
    }

    #[test]
    fn test_cached_config_values() {
        // Verify hot-path config values are properly cached
        let config = BroadcastConfig::from_env();

        // These should be the defaults (no env vars set in test)
        assert_eq!(config.l1_throttle_ms, 50, "L1 throttle should be cached");
        assert_eq!(config.spot_log_threshold, 50_000.0, "Spot log threshold should be cached");
    }

    // ========== CRITICAL: Feed staleness detection tests ==========

    #[test]
    fn test_feed_staleness_detection_logic() {
        use std::sync::atomic::Ordering;

        // Simulate the staleness check logic from the metrics task
        let now_ms = chrono::Utc::now().timestamp_millis();

        // Recent event (5 seconds ago) - should NOT be stale
        let recent_event_ms = now_ms - 5_000;
        let is_recent_stale = recent_event_ms > 0 && (now_ms - recent_event_ms) > FEED_STALE_THRESHOLD_MS;
        assert!(!is_recent_stale, "5s old event should NOT be stale (threshold is 30s)");

        // Old event (45 seconds ago) - should BE stale
        let old_event_ms = now_ms - 45_000;
        let is_old_stale = old_event_ms > 0 && (now_ms - old_event_ms) > FEED_STALE_THRESHOLD_MS;
        assert!(is_old_stale, "45s old event should BE stale (threshold is 30s)");

        // Never received event (0) - should NOT trigger stale (special case)
        let never_received_ms = 0_i64;
        let is_never_stale = never_received_ms > 0 && (now_ms - never_received_ms) > FEED_STALE_THRESHOLD_MS;
        assert!(!is_never_stale, "Never-received event should NOT trigger stale alert");
    }

    #[test]
    fn test_feed_event_timestamp_tracking() {
        use std::sync::atomic::Ordering;

        // Reset timestamps
        BINANCE_LAST_EVENT_MS.store(0, Ordering::Relaxed);

        // Verify initial state
        assert_eq!(BINANCE_LAST_EVENT_MS.load(Ordering::Relaxed), 0);

        // Simulate event arrival
        let now_ms = chrono::Utc::now().timestamp_millis();
        BINANCE_LAST_EVENT_MS.store(now_ms, Ordering::Relaxed);

        // Verify timestamp updated
        let stored = BINANCE_LAST_EVENT_MS.load(Ordering::Relaxed);
        assert!(stored > 0, "Timestamp should be updated after event");
        assert!((stored - now_ms).abs() < 1000, "Timestamp should be recent");
    }

    #[test]
    fn test_backpressure_alert_threshold() {
        use std::sync::atomic::Ordering;

        // Reset counters
        AGG_CHANNEL_DROPPED.store(0, Ordering::Relaxed);
        AGG_LAST_DROP_WARN_MS.store(0, Ordering::Relaxed);

        // Simulate burst of drops
        for _ in 0..100 {
            AGG_CHANNEL_DROPPED.fetch_add(1, Ordering::Relaxed);
        }

        let total_dropped = AGG_CHANNEL_DROPPED.load(Ordering::Relaxed);
        assert_eq!(total_dropped, 100, "Should track all dropped events");

        // In production, this would trigger a warning like:
        // "Aggregator channel full - dropped 100 events total (consider increasing AGG_EVENT_BUFFER)"
        // The rate limiting ensures we don't spam warnings (tested in test_rate_limited_warning_logic)
    }

    #[test]
    fn test_metrics_calculation_with_events() {
        use std::sync::atomic::Ordering;

        // Reset all metrics
        TRADE_COUNT.store(0, Ordering::Relaxed);
        SKEW_SUM_MS.store(0, Ordering::Relaxed);
        SKEW_COUNT.store(0, Ordering::Relaxed);
        SKEW_MAX_MS.store(0, Ordering::Relaxed);
        SKEW_MIN_MS.store(i64::MAX, Ordering::Relaxed);

        // Simulate 10 trades with various skews
        for skew in [5, 10, 15, 20, 25, 30, 35, 40, 45, 50_i64] {
            TRADE_COUNT.fetch_add(1, Ordering::Relaxed);
            SKEW_SUM_MS.fetch_add(skew, Ordering::Relaxed);
            SKEW_COUNT.fetch_add(1, Ordering::Relaxed);

            // Update max
            let mut current_max = SKEW_MAX_MS.load(Ordering::Relaxed);
            while skew > current_max {
                match SKEW_MAX_MS.compare_exchange_weak(
                    current_max, skew, Ordering::Relaxed, Ordering::Relaxed
                ) {
                    Ok(_) => break,
                    Err(x) => current_max = x,
                }
            }

            // Update min
            let mut current_min = SKEW_MIN_MS.load(Ordering::Relaxed);
            while skew < current_min {
                match SKEW_MIN_MS.compare_exchange_weak(
                    current_min, skew, Ordering::Relaxed, Ordering::Relaxed
                ) {
                    Ok(_) => break,
                    Err(x) => current_min = x,
                }
            }
        }

        // Verify metrics
        let trades = TRADE_COUNT.load(Ordering::Relaxed);
        let skew_sum = SKEW_SUM_MS.load(Ordering::Relaxed);
        let skew_count = SKEW_COUNT.load(Ordering::Relaxed);
        let skew_max = SKEW_MAX_MS.load(Ordering::Relaxed);
        let skew_min = SKEW_MIN_MS.load(Ordering::Relaxed);

        assert_eq!(trades, 10, "Should have 10 trades");
        assert_eq!(skew_sum, 275, "Sum should be 5+10+...+50 = 275");
        assert_eq!(skew_count, 10, "Should have 10 skew samples");
        assert_eq!(skew_max, 50, "Max skew should be 50ms");
        assert_eq!(skew_min, 5, "Min skew should be 5ms");

        // Calculate average like the metrics task does
        let skew_avg = if skew_count > 0 { skew_sum / skew_count as i64 } else { 0 };
        assert_eq!(skew_avg, 27, "Average skew should be 27ms (275/10)");
    }

    // ========== END-TO-END: Integration tests ==========

    #[tokio::test]
    async fn test_e2e_broadcast_channel_data_flow() {
        // This tests the complete data flow:
        // Event -> serialize_for_broadcast -> broadcast channel -> receive

        let config = BroadcastConfig {
            use_envelope: false,
            source: "test".to_string(),
            use_binary_frames: true,
            l1_throttle_ms: 50,
            spot_log_threshold: 50_000.0,
        };

        // Create broadcast channel (simulates server)
        let (tx, mut rx) = broadcast::channel::<Bytes>(100);

        // Create test event
        let event = test_event();

        // Serialize (simulates hot path)
        let bytes = serialize_for_broadcast(&config, event.clone())
            .expect("Serialization should succeed");

        // Broadcast
        tx.send(bytes.clone()).expect("Broadcast should succeed");

        // Receive (simulates client)
        let received = rx.recv().await.expect("Should receive broadcast");

        // Verify received data
        assert_eq!(received, bytes, "Received bytes should match sent bytes");

        // Parse received data (simulates client parsing)
        let parsed: MarketEventMessage = serde_json::from_slice(&received)
            .expect("Should parse as MarketEventMessage");

        assert_eq!(parsed.exchange, "TestExchange");
        assert_eq!(parsed.kind, "trade");
        assert_eq!(parsed.instrument.base, "BTC");
    }

    #[tokio::test]
    async fn test_e2e_multiple_clients_receive_same_data() {
        // Test that multiple clients receive the same broadcast

        let config = BroadcastConfig {
            use_envelope: false,
            source: "test".to_string(),
            use_binary_frames: true,
            l1_throttle_ms: 50,
            spot_log_threshold: 50_000.0,
        };

        // Create broadcast channel with multiple receivers
        let (tx, _rx1) = broadcast::channel::<Bytes>(100);
        let mut rx1 = tx.subscribe();
        let mut rx2 = tx.subscribe();
        let mut rx3 = tx.subscribe();

        // Broadcast event
        let event = test_event();
        let bytes = serialize_for_broadcast(&config, event).unwrap();
        tx.send(bytes.clone()).unwrap();

        // All clients should receive the same data
        let recv1 = rx1.recv().await.unwrap();
        let recv2 = rx2.recv().await.unwrap();
        let recv3 = rx3.recv().await.unwrap();

        assert_eq!(recv1, recv2, "Client 1 and 2 should receive same data");
        assert_eq!(recv2, recv3, "Client 2 and 3 should receive same data");
    }

    #[tokio::test]
    async fn test_e2e_envelope_format_data_flow() {
        // Test envelope format end-to-end

        let config = BroadcastConfig {
            use_envelope: true,
            source: "test-server".to_string(),
            use_binary_frames: true,
            l1_throttle_ms: 50,
            spot_log_threshold: 50_000.0,
        };

        let (tx, mut rx) = broadcast::channel::<Bytes>(100);

        let event = test_event();
        let bytes = serialize_for_broadcast(&config, event).unwrap();
        tx.send(bytes).unwrap();

        let received = rx.recv().await.unwrap();

        // Parse as JSON value to verify envelope structure
        let envelope: serde_json::Value = serde_json::from_slice(&received)
            .expect("Should parse as JSON");

        assert_eq!(envelope["schema_version"], 1);
        assert_eq!(envelope["source"], "test-server");
        assert_eq!(envelope["payload"]["exchange"], "TestExchange");
    }

    #[tokio::test]
    async fn test_e2e_aggregator_channel_data_flow() {
        // Test aggregator channel receives events correctly

        let (tx, mut rx) = mpsc::channel::<MarketEventMessage>(100);

        // Send multiple events
        for i in 0..10 {
            let mut event = test_event();
            event.instrument.base = format!("TEST{}", i);
            tx.send(event).await.expect("Send should succeed");
        }

        // Receive and verify
        for i in 0..10 {
            let received = rx.recv().await.expect("Should receive event");
            assert_eq!(received.instrument.base, format!("TEST{}", i));
        }
    }
}
