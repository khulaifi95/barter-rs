use barter_data::{
    error::DataError,
    event::{DataKind, MarketEvent},
    streams::{
        builder::dynamic::DynamicStreams,
        consumer::MarketStreamResult,
        reconnect::Event,
    },
    subscription::open_interest::OpenInterest,
};
use barter_instrument::{
    exchange::ExchangeId,
    instrument::market_data::{MarketDataInstrument, kind::MarketDataInstrumentKind},
};
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt, stream};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::broadcast,
    time::interval,
};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

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

impl From<MarketEvent<MarketDataInstrument, DataKind>> for MarketEventMessage {
    fn from(event: MarketEvent<MarketDataInstrument, DataKind>) -> Self {
        let (kind_name, data) = match &event.kind {
            DataKind::Trade(trade) => ("trade", serde_json::to_value(trade).unwrap_or_default()),
            DataKind::Liquidation(liq) => ("liquidation", serde_json::to_value(liq).unwrap_or_default()),
            DataKind::OpenInterest(oi) => ("open_interest", serde_json::to_value(oi).unwrap_or_default()),
            DataKind::CumulativeVolumeDelta(cvd) => ("cumulative_volume_delta", serde_json::to_value(cvd).unwrap_or_default()),
            DataKind::OrderBookL1(ob) => ("order_book_l1", serde_json::to_value(ob).unwrap_or_default()),
            _ => ("other", serde_json::Value::Null),
        };

        Self {
            time_exchange: event.time_exchange,
            time_received: event.time_received,
            exchange: format!("{:?}", event.exchange),
            instrument: InstrumentInfo {
                base: event.instrument.base.to_string(),
                quote: event.instrument.quote.to_string(),
                kind: format!("{:?}", event.instrument.kind),
            },
            kind: kind_name.to_string(),
            data,
        }
    }
}

#[tokio::main]
async fn main() {
    // Initialize logging
    init_logging();

    info!("Starting barter-data WebSocket server");

    // Create broadcast channel for market events
    // Configurable buffer size via WS_BUFFER_SIZE env var (default: 10,000)
    let buffer_size = std::env::var("WS_BUFFER_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);

    info!("WebSocket broadcast buffer size: {}", buffer_size);
    let (tx, _rx) = broadcast::channel::<MarketEventMessage>(buffer_size);
    let tx = Arc::new(tx);

    // Start WebSocket server
    let server_addr = "0.0.0.0:9001".parse::<SocketAddr>().unwrap();
    let tx_clone = tx.clone();
    tokio::spawn(async move {
        start_websocket_server(server_addr, tx_clone).await;
    });

    info!("WebSocket server listening on ws://{}", server_addr);
    info!("Clients can connect to receive real-time market data");

    // Initialize market data streams
    let streams = init_market_streams().await;

    // Combine WebSocket and REST API streams
    let combined_stream = stream::select(
        streams.select_all::<MarketStreamResult<MarketDataInstrument, DataKind>>(),
        binance_open_interest_stream(),
    );

    futures::pin_mut!(combined_stream);

    // Process market events and broadcast to clients
    while let Some(event) = combined_stream.next().await {
        match event {
            Event::Reconnecting(exchange) => {
                warn!("Reconnecting to {:?}", exchange);
            }
            Event::Item(result) => match result {
                Ok(market_event) => {
                    let message = MarketEventMessage::from(market_event);

                    // Broadcast to all connected clients (ignore errors if no receivers)
                    let _ = tx.send(message);
                }
                Err(error) => {
                    // Filter out known non-errors
                    let error_str = format!("{:?}", error);
                    if !error_str.contains("payload: pong") && !error_str.contains("liquidation-orders|SWAP") {
                        debug!("Market stream error: {:?}", error);
                    }
                }
            },
        }
    }
}

/// Start WebSocket server that broadcasts market events to connected clients
async fn start_websocket_server(addr: SocketAddr, tx: Arc<broadcast::Sender<MarketEventMessage>>) {
    let listener = TcpListener::bind(&addr)
        .await
        .expect("Failed to bind WebSocket server");

    info!("WebSocket server bound to {}", addr);

    while let Ok((stream, peer_addr)) = listener.accept().await {
        info!("New WebSocket connection from {}", peer_addr);
        let tx = tx.clone();
        tokio::spawn(handle_client(stream, peer_addr, tx));
    }
}

/// Handle individual WebSocket client connection
async fn handle_client(
    stream: TcpStream,
    peer_addr: SocketAddr,
    tx: Arc<broadcast::Sender<MarketEventMessage>>,
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
    let mut rx = tx.subscribe();

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
    let mut send_task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Ok(json) = serde_json::to_string(&event) {
                        if ws_sender.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    // Client fell behind - this is NORMAL under high load
                    // Just log and continue, don't disconnect
                    warn!("Client {} lagged, skipped {} messages", peer_addr, skipped);
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    // Channel closed, exit gracefully
                    info!("Broadcast channel closed for {}", peer_addr);
                    break;
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
    use barter_data::subscription::SubKind;
    use ExchangeId::*;
    use MarketDataInstrumentKind::*;
    use SubKind::*;

    DynamicStreams::init([
        vec![
            (BybitPerpetualsUsd, "btc", "usdt", Perpetual, OpenInterest),
        ],
        vec![
            (BybitPerpetualsUsd, "btc", "usdt", Perpetual, Liquidations),
        ],
        vec![
            (BybitPerpetualsUsd, "btc", "usdt", Perpetual, CumulativeVolumeDelta),
        ],
        vec![
            (BinanceFuturesUsd, "btc", "usdt", Perpetual, Liquidations),
        ],
        vec![
            (BinanceFuturesUsd, "btc", "usdt", Perpetual, CumulativeVolumeDelta),
        ],
        vec![
            (Okx, "btc", "usdt", Perpetual, OpenInterest),
        ],
        vec![
            (Okx, "btc", "usdt", Perpetual, Liquidations),
        ],
        vec![
            (Okx, "btc", "usdt", Perpetual, CumulativeVolumeDelta),
        ],
        vec![
            (BinanceFuturesUsd, "btc", "usdt", Perpetual, OrderBooksL1),
        ],
        vec![
            (BybitPerpetualsUsd, "btc", "usdt", Perpetual, OrderBooksL1),
        ],
        vec![
            (BinanceFuturesUsd, "btc", "usdt", Perpetual, PublicTrades),
        ],
        vec![
            (BybitPerpetualsUsd, "btc", "usdt", Perpetual, PublicTrades),
        ],
        vec![
            (Okx, "btc", "usdt", Perpetual, PublicTrades),
        ],
    ])
    .await
    .expect("Failed to initialize market streams")
}

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
        (client, url, interval(std::time::Duration::from_secs(10)), instrument),
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
