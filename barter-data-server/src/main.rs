use barter_data::{
    error::DataError,
    event::{DataKind, MarketEvent, MarketEventEnvelope},
    snapshot::{MarketSnapshot, SnapshotPerExchangeShort, SnapshotTicker},
    streams::{builder::dynamic::DynamicStreams, consumer::MarketStreamResult, reconnect::Event},
    subscription::funding::FundingRate,
    subscription::open_interest::OpenInterest,
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
    state::{Aggregator, Candle1m, CandleBackfill, fetch_binance_1m_candles, ticker_to_binance_symbol},
    types::{InstrumentInfo, MarketEventMessage},
    vol_regime::VolRegimeEngine,
};
use chrono::{DateTime, TimeZone, Utc, Duration as ChronoDuration};
use futures::{SinkExt, StreamExt, stream};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, atomic::{AtomicI64, Ordering}};
use std::time::Instant;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::broadcast,
    time::{interval, Duration},
};
use tokio_tungstenite::{accept_async, connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

// L2 throttling per exchange (OKX is noisier, needs higher throttle)
const L2_THROTTLE_BINANCE_MS: u64 = 100;
const L2_THROTTLE_BYBIT_MS: u64 = 100;
const L2_THROTTLE_OKX_MS: u64 = 150;
const SNAPSHOT_VERSION: u16 = 2;

/// Get L2 throttle interval for a given exchange
fn get_l2_throttle_ms(exchange: &str) -> u64 {
    if exchange.contains("Okx") {
        std::env::var("L2_THROTTLE_OKX_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(L2_THROTTLE_OKX_MS)
    } else if exchange.contains("Bybit") {
        std::env::var("L2_THROTTLE_BYBIT_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(L2_THROTTLE_BYBIT_MS)
    } else {
        std::env::var("L2_THROTTLE_BINANCE_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(L2_THROTTLE_BINANCE_MS)
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
    TickBackfill { symbol: String, ticks: Vec<TradMarketTick> },
    #[serde(rename = "welcome")]
    Welcome { #[serde(default)] message: Option<String> },
    #[serde(rename = "status")]
    Status { #[serde(default)] connected: Option<bool> },
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
    open_interest: Option<f64>,
    mark_iv: Option<f64>,
    greeks: Option<DeribitGreeks>,
}

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

#[tokio::main]
async fn main() {
    // Initialize logging
    init_logging();

    info!("Starting barter-data WebSocket server");

    // Separate channels for trades (hot path) and L2 (high volume, lower priority)
    let trades_buffer = std::env::var("WS_TRADES_BUFFER")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);
    let l2_buffer = std::env::var("WS_L2_BUFFER")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50_000);

    info!(
        "Trade channel buffer: {}, L2 channel buffer: {}",
        trades_buffer, l2_buffer
    );

    // Trades channel: trades, liquidations, OI, CVD, L1 (hot path - NO L2)
    let (tx_trades, _) = broadcast::channel::<MarketEventMessage>(trades_buffer);
    let tx_trades = Arc::new(tx_trades);

    // L2 channel: orderbook L2 only (high volume, can lag without affecting trades)
    let (tx_l2, _) = broadcast::channel::<MarketEventMessage>(l2_buffer);
    let tx_l2 = Arc::new(tx_l2);

    let snapshot_builder = Arc::new(tokio::sync::Mutex::new(SnapshotBuilder::new()));
    let shared_agg = Arc::new(tokio::sync::Mutex::new(Aggregator::new()));
    let vol_states = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let spot_cache = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let options_cache = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let trad_last_ms = Arc::new(AtomicI64::new(0));
    let kline_cache = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    // Start WebSocket server
    // Configurable via WS_ADDR env var (default: 0.0.0.0:9001)
    let server_addr_str = std::env::var("WS_ADDR").unwrap_or_else(|_| "0.0.0.0:9001".to_string());
    let server_addr = server_addr_str
        .parse::<SocketAddr>()
        .unwrap_or_else(|_| "0.0.0.0:9001".parse().unwrap());
    let tx_trades_clone = tx_trades.clone();
    let tx_l2_clone = tx_l2.clone();
    let kline_cache_clone = Arc::clone(&kline_cache);
    tokio::spawn(async move {
        start_websocket_server(server_addr, tx_trades_clone, tx_l2_clone, kline_cache_clone).await;
    });

    // IBKR bridge feed (ES/NQ) -> trad_tick events
    {
        let tx_trades = tx_trades.clone();
        let trad_last_ms = Arc::clone(&trad_last_ms);
        tokio::spawn(async move {
            start_ibkr_bridge_feed(tx_trades, trad_last_ms).await;
        });
    }

    // Deribit options feed (options_chain events)
    {
        let tx_trades = tx_trades.clone();
        let spot_cache = Arc::clone(&spot_cache);
        let options_cache = Arc::clone(&options_cache);
        tokio::spawn(async move {
            start_deribit_options_feed(tx_trades, spot_cache, options_cache).await;
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
        let shared_agg = Arc::clone(&shared_agg);
        seed_binance_klines(kline_cache, shared_agg, &tickers).await;
    }
    for ticker in &tickers {
        let tx_trades = tx_trades.clone();
        let kline_cache = Arc::clone(&kline_cache);
        let shared_agg = Arc::clone(&shared_agg);
        let ticker = ticker.clone();
        tokio::spawn(async move {
            run_binance_kline_stream(ticker, tx_trades, kline_cache, shared_agg).await;
        });
    }

    {
        let tx_trades = tx_trades.clone();
        let snapshot_builder = Arc::clone(&snapshot_builder);
        let shared_agg = Arc::clone(&shared_agg);
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
                let agg_snapshot = {
                    let guard = shared_agg.lock().await;
                    guard.snapshot()
                };
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
                let _ = tx_trades.send(event);
            }
        });
    }

    // Centralized orchestrator (MarketState) - broadcasts orchestrator_result
    {
        let tx_trades = tx_trades.clone();
        let shared_agg = Arc::clone(&shared_agg);
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
                let snapshot = {
                    let guard = shared_agg.lock().await;
                    guard.snapshot()
                };
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
                    let _ = tx_trades.send(event);
                }
            }
        });
    }

    // Volatility regime updater: push hourly RV samples into engines
    {
        let shared_agg = Arc::clone(&shared_agg);
        let vol_states = Arc::clone(&vol_states);
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(60));
            loop {
                tick.tick().await;
                let now_hour = Utc::now().timestamp() / 3600;
                let snapshot = {
                    let guard = shared_agg.lock().await;
                    guard.snapshot()
                };
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

    let shared_agg = Arc::clone(&shared_agg);
    let spot_cache = Arc::clone(&spot_cache);

    // Throttle state: per-instrument last broadcast time (L2 and Binance L1)
    let mut l2_last_broadcast: HashMap<String, Instant> = HashMap::new();
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
                    // Threshold configurable via SPOT_LOG_THRESHOLD env var (default: $50,000)
                    if let DataKind::Trade(trade) = &market_event.kind {
                        let spot_log_threshold = std::env::var("SPOT_LOG_THRESHOLD")
                            .ok()
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(50_000.0);
                        let notional = trade.price * trade.amount;
                        let is_spot =
                            matches!(market_event.instrument.kind, MarketDataInstrumentKind::Spot);
                        if is_spot && notional >= spot_log_threshold {
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

                    // L2 orderbook events: apply per-exchange throttle and route to L2 channel
                    if is_orderbook_l2 {
                        debug!(
                            "L2_BOOK {} {}/{}",
                            market_event.exchange,
                            market_event.instrument.base,
                            market_event.instrument.quote
                        );

                        // Per-exchange throttling
                        let key = format!(
                            "{}:{}:{}",
                            market_event.exchange,
                            market_event.instrument.base,
                            market_event.instrument.quote
                        );
                        let exchange_str = format!("{:?}", market_event.exchange);
                        let throttle_ms = get_l2_throttle_ms(&exchange_str);
                        let now = Instant::now();

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
                        shared_agg.lock().await.process_event(message.clone());
                        let _ = tx_l2.send(message); // Ignore errors if no receivers
                        continue; // Don't fall through to trade channel
                    }

                    let message = market_event_to_message(market_event);
                    shared_agg.lock().await.process_event(message.clone());

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
                            let throttle_ms: u64 = std::env::var("L1_THROTTLE_MS")
                                .ok()
                                .and_then(|v| v.parse().ok())
                                .unwrap_or(50); // ~20 updates/sec per instrument
                            let now = Instant::now();
                            let should_skip = if let Some(prev) = l1_last_broadcast.get(&key) {
                                now.duration_since(*prev) < Duration::from_millis(throttle_ms)
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
                    match tx_trades.send(message) {
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
    tx_trades: Arc<broadcast::Sender<MarketEventMessage>>,
    tx_l2: Arc<broadcast::Sender<MarketEventMessage>>,
    kline_cache: Arc<tokio::sync::Mutex<HashMap<String, VecDeque<Candle1m>>>>,
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
        tokio::spawn(handle_client(
            stream,
            peer_addr,
            tx_trades,
            tx_l2,
            kline_cache,
        ));
    }
}

async fn start_ibkr_bridge_feed(
    tx_trades: Arc<broadcast::Sender<MarketEventMessage>>,
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
                                            let _ = tx_trades.send(trad_tick_event(tick));
                                            tick_count += 1;
                                        }
                                        Ok(IbkrMessage::TickBackfill { ticks, .. }) => {
                                            for tick in ticks {
                                                if tick.ts > 0 {
                                                    trad_last_ms.store(tick.ts, Ordering::Relaxed);
                                                }
                                                let _ = tx_trades.send(trad_tick_event(tick));
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
    tx_trades: Arc<broadcast::Sender<MarketEventMessage>>,
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
                    let _ = tx_trades.send(event);

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
                        let _ = tx_trades.send(ctx_event);
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
    shared_agg: Arc<tokio::sync::Mutex<Aggregator>>,
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
                shared_agg.lock().await.process_event(event);
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
    tx_trades: Arc<broadcast::Sender<MarketEventMessage>>,
    kline_cache: Arc<tokio::sync::Mutex<HashMap<String, VecDeque<Candle1m>>>>,
    shared_agg: Arc<tokio::sync::Mutex<Aggregator>>,
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
                                                let _ = tx_trades.send(event.clone());
                                                shared_agg.lock().await.process_event(event);
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
    tx_trades: Arc<broadcast::Sender<MarketEventMessage>>,
    tx_l2: Arc<broadcast::Sender<MarketEventMessage>>,
    kline_cache: Arc<tokio::sync::Mutex<HashMap<String, VecDeque<Candle1m>>>>,
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
    let use_envelope = std::env::var("WS_ENVELOPE")
        .ok()
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE"))
        .unwrap_or(false);
    let envelope_source =
        std::env::var("WS_SOURCE").unwrap_or_else(|_| "barter-data-server".to_string());

    // Send welcome message
    let welcome = serde_json::json!({
        "type": "welcome",
        "message": "Connected to barter-data market feed",
        "timestamp": Utc::now()
    });
    if let Ok(msg) = serde_json::to_string(&welcome) {
        let _ = ws_sender.send(Message::Text(msg.into())).await;
    }

    // Send kline backfill to align RV/ATR across clients
    // First check for stale cache (last candle date != today) and refresh if needed
    {
        let today = Utc::now().date_naive();
        let mut stale_tickers: Vec<String> = Vec::new();

        // Check which tickers have stale cache
        {
            let cache = kline_cache.lock().await;
            for (ticker, candles) in cache.iter() {
                let is_stale = candles.is_empty()
                    || candles.back().map(|c| c.start_time.date_naive() != today).unwrap_or(true);
                if is_stale {
                    stale_tickers.push(ticker.clone());
                }
            }
        }

        // Refresh stale tickers
        for ticker in &stale_tickers {
            let symbol = ticker_to_binance_symbol(ticker);
            if let Ok(fresh_candles) = fetch_binance_1m_candles(symbol).await {
                let mut cache = kline_cache.lock().await;
                let entry = cache.entry(ticker.clone()).or_insert_with(VecDeque::new);
                entry.clear();
                for candle in fresh_candles {
                    entry.push_back(candle);
                }
                while entry.len() > 300 {
                    entry.pop_front();
                }
                info!("Refreshed stale kline cache for {} (had {} candles)", ticker, entry.len());
            }
        }

        // Now send backfill for all tickers
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
            if let Ok(json) = serde_json::to_string(&event) {
                let _ = ws_sender.send(Message::Text(json.into())).await;
            }
        }
    }

    // Spawn task to send market events to this client
    // Uses biased select! to prioritize trades over L2
    let mut send_task = tokio::spawn(async move {
        loop {
            // Biased select: trades always checked first (hot path priority)
            tokio::select! {
                biased;

                // PRIORITY 1: Trades, liquidations, OI, CVD, L1 (hot path)
                result = rx_trades.recv() => {
                    match result {
                        Ok(event) => {
                            let json = if use_envelope {
                                let wrapped = MarketEventEnvelope {
                                    schema_version: 1,
                                    source: envelope_source.clone(),
                                    time_sent: Utc::now(),
                                    payload: event,
                                };
                                serde_json::to_string(&wrapped)
                            } else {
                                serde_json::to_string(&event)
                            };
                            if let Ok(json) = json {
                                if ws_sender.send(Message::Text(json.into())).await.is_err() {
                                    break;
                                }
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
                        Ok(event) => {
                            let json = if use_envelope {
                                let wrapped = MarketEventEnvelope {
                                    schema_version: 1,
                                    source: envelope_source.clone(),
                                    time_sent: Utc::now(),
                                    payload: event,
                                };
                                serde_json::to_string(&wrapped)
                            } else {
                                serde_json::to_string(&event)
                            };
                            if let Ok(json) = json {
                                if ws_sender.send(Message::Text(json.into())).await.is_err() {
                                    break;
                                }
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

/// Initialize logging
fn init_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
}
