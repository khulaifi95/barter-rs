use barter_data::{
    error::DataError,
    event::{DataKind, MarketEvent, MarketEventEnvelope},
    streams::{builder::dynamic::DynamicStreams, consumer::MarketStreamResult, reconnect::Event},
    subscription::funding::FundingRate,
    subscription::open_interest::OpenInterest,
};
use barter_instrument::{
    exchange::ExchangeId,
    instrument::market_data::{MarketDataInstrument, kind::MarketDataInstrumentKind},
};
use chrono::{DateTime, TimeZone, Utc};
use futures::{SinkExt, StreamExt, stream};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
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

/// Market event wrapper for JSON serialization
#[derive(Debug, Clone, Serialize)]
struct MarketEventMessage {
    time_exchange: DateTime<Utc>,
    time_received: DateTime<Utc>,
    exchange: String,
    instrument: InstrumentInfo,
    kind: String,
    data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
struct InstrumentInfo {
    base: String,
    quote: String,
    kind: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OptionsChainMessage {
    contracts: Vec<OptionContract>,
    timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OptionContract {
    instrument_name: String,
    strike: f64,
    expiry: i64,
    is_call: bool,
    open_interest: f64,
    mark_iv: f64,
    delta: f64,
    gamma: f64,
    vega: f64,
}

impl From<MarketEvent<MarketDataInstrument, DataKind>> for MarketEventMessage {
    fn from(event: MarketEvent<MarketDataInstrument, DataKind>) -> Self {
        let (kind_name, data) = match &event.kind {
            DataKind::Trade(trade) => ("trade", serde_json::to_value(trade).unwrap_or_default()),
            DataKind::Liquidation(liq) => {
                ("liquidation", serde_json::to_value(liq).unwrap_or_default())
            }
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

        Self {
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

    // Start WebSocket server
    // Configurable via WS_ADDR env var (default: 0.0.0.0:9001)
    let server_addr_str = std::env::var("WS_ADDR").unwrap_or_else(|_| "0.0.0.0:9001".to_string());
    let server_addr = server_addr_str
        .parse::<SocketAddr>()
        .unwrap_or_else(|_| "0.0.0.0:9001".parse().unwrap());
    let tx_trades_clone = tx_trades.clone();
    let tx_l2_clone = tx_l2.clone();
    tokio::spawn(async move {
        start_websocket_server(server_addr, tx_trades_clone, tx_l2_clone).await;
    });

    // IBKR bridge feed (ES/NQ) -> trad_tick events
    {
        let tx_trades = tx_trades.clone();
        tokio::spawn(async move {
            start_ibkr_bridge_feed(tx_trades).await;
        });
    }

    // Deribit options feed (options_chain events)
    {
        let tx_trades = tx_trades.clone();
        tokio::spawn(async move {
            start_deribit_options_feed(tx_trades).await;
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
                        let message = MarketEventMessage::from(market_event);
                        let _ = tx_l2.send(message); // Ignore errors if no receivers
                        continue; // Don't fall through to trade channel
                    }

                    let message = MarketEventMessage::from(market_event);

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
) {
    let listener = TcpListener::bind(&addr)
        .await
        .expect("Failed to bind WebSocket server");

    info!("WebSocket server bound to {}", addr);

    while let Ok((stream, peer_addr)) = listener.accept().await {
        info!("New WebSocket connection from {}", peer_addr);
        let tx_trades = tx_trades.clone();
        let tx_l2 = tx_l2.clone();
        tokio::spawn(handle_client(stream, peer_addr, tx_trades, tx_l2));
    }
}

async fn start_ibkr_bridge_feed(tx_trades: Arc<broadcast::Sender<MarketEventMessage>>) {
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

                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            match serde_json::from_str::<IbkrMessage>(&text) {
                                Ok(IbkrMessage::Tick(tick)) => {
                                    let _ = tx_trades.send(trad_tick_event(tick));
                                }
                                Ok(IbkrMessage::TickBackfill { ticks, .. }) => {
                                    for tick in ticks {
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
                        Ok(Message::Close(_)) => {
                            warn!("ibkr-bridge connection closed");
                            break;
                        }
                        Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
                        Err(e) => {
                            warn!("ibkr-bridge websocket error: {}", e);
                            break;
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                warn!("Failed to connect to ibkr-bridge at {}: {}", url, e);
            }
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn start_deribit_options_feed(tx_trades: Arc<broadcast::Sender<MarketEventMessage>>) {
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
                }
                Err(e) => {
                    warn!("Deribit options fetch failed for {}: {}", ticker, e);
                }
            }
        }
    }
}

async fn fetch_deribit_options_chain(
    client: &Client,
    base_url: &str,
    currency: &str,
    top_n: usize,
) -> Result<OptionsChainMessage, String> {
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

    Ok(OptionsChainMessage {
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
