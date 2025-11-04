use barter_data::{
    error::DataError,
    event::{DataKind, MarketEvent},
    streams::{
        builder::dynamic::DynamicStreams,
        consumer::MarketStreamResult,
        reconnect::{Event, stream::ReconnectingStream},
    },
    subscription::{SubKind, open_interest::OpenInterest},
};
use barter_instrument::{
    exchange::ExchangeId,
    instrument::market_data::{MarketDataInstrument, kind::MarketDataInstrumentKind},
};
use chrono::{DateTime, Utc};
use futures::{StreamExt, stream};
use reqwest::Client;
use serde::Deserialize;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use tokio::time::interval;
use tracing::{debug, info, warn};

#[rustfmt::skip]
#[tokio::main]
async fn main() {
    // Initialise INFO Tracing log subscriber
    init_logging();

    use ExchangeId::*;
    use MarketDataInstrumentKind::*;
    use SubKind::*;

    // Note: Binance doesn't have open interest on WebSocket, only via REST API

    // Notes:
    // - DynamicStream::init requires an IntoIterator<Item = "subscription batch">.
    // - Each "subscription batch" is an IntoIterator<Item = Subscription>.
    // - Every "subscription batch" will initialise at-least-one WebSocket stream under the hood.
    // - If the "subscription batch" contains more-than-one ExchangeId and/or SubKind, the batch
    //   will be further split under the hood for compile-time reasons.

    // Initialise market reconnect::Event streams for various ExchangeIds and SubscriptionKinds
    let streams = DynamicStreams::init([
        // Batch notes:
        // Since batch contains 1 ExchangeId and 1 SubscriptionKind, so only 1 (1x1) WebSockets
        // will be spawned for this batch.
        // vec![
        //     (BinanceSpot, "btc", "usdt", Spot, PublicTrades),
        //     (BinanceSpot, "eth", "usdt", Spot, PublicTrades),
        // ],

        // Batch notes:
        // Since batch contains 1 ExchangeId and 3 SubscriptionKinds, 3 (1x3) WebSocket connections
        // will be spawned for this batch (back-end requires to further split).
        // vec![
        //     (BinanceFuturesUsd, "btc", "usdt", Perpetual, PublicTrades),
        //     (BinanceFuturesUsd, "btc", "usdt", Perpetual, OrderBooksL1),
        //     (BinanceFuturesUsd, "btc", "usdt", Perpetual, Liquidations),

        // ],

        // Batch notes:
        // Dedicated tickers subscription harvesting Bybit open interest for BTCUSDT.
        vec![
            (BybitPerpetualsUsd, "btc", "usdt", Perpetual, OpenInterest),
            // (BybitPerpetualsUsd, "eth", "usdt", Perpetual, OpenInterest),
            // (BybitPerpetualsUsd, "sol", "usdt", Perpetual, OpenInterest),
            // (BybitPerpetualsUsd, "xrp", "usdt", Perpetual, OpenInterest),
        ],

        // Batch notes:
        // Global liquidation updates for Bybit BTCUSDT perpetual.
        vec![
            (BybitPerpetualsUsd, "btc", "usdt", Perpetual, Liquidations),
            // (BybitPerpetualsUsd, "eth", "usdt", Perpetual, Liquidations),
            // (BybitPerpetualsUsd, "sol", "usdt", Perpetual, Liquidations),
            // (BybitPerpetualsUsd, "xrp", "usdt", Perpetual, Liquidations),
        ],

        // Batch notes:
        // Cumulative volume delta derived locally from Bybit trade stream for BTCUSDT.
        vec![
            (BybitPerpetualsUsd, "btc", "usdt", Perpetual, CumulativeVolumeDelta),
            // (BybitPerpetualsUsd, "eth", "usdt", Perpetual, CumulativeVolumeDelta),
            // (BybitPerpetualsUsd, "sol", "usdt", Perpetual, CumulativeVolumeDelta),
            // (BybitPerpetualsUsd, "xrp", "usdt", Perpetual, CumulativeVolumeDelta),
        ],

        // Batch notes:
        // Binance Futures liquidation updates for BTCUSDT.
        vec![
            (BinanceFuturesUsd, "btc", "usdt", Perpetual, Liquidations),
            // (BinanceFuturesUsd, "eth", "usdt", Perpetual, Liquidations),
            // (BinanceFuturesUsd, "sol", "usdt", Perpetual, Liquidations),
            // (BinanceFuturesUsd, "xrp", "usdt", Perpetual, Liquidations),
        ],

        // Batch notes:
        // Binance Futures CVD derived from trade stream for BTCUSDT.
        vec![
            (BinanceFuturesUsd, "btc", "usdt", Perpetual, CumulativeVolumeDelta),
            // (BinanceFuturesUsd, "eth", "usdt", Perpetual, CumulativeVolumeDelta),
            // (BinanceFuturesUsd, "sol", "usdt", Perpetual, CumulativeVolumeDelta),
            // (BinanceFuturesUsd, "xrp", "usdt", Perpetual, CumulativeVolumeDelta),
        ],

        // Batch notes:
        // Since batch contains 2 ExchangeIds and 1 SubscriptionKind, 2 (2x1) WebSocket connections
        // will be spawned for this batch (back-end requires to further split).
        // vec![
        //     (Okx, "btc", "usdt", Spot, PublicTrades),
        //     (Okx, "btc", "usdt", Perpetual, PublicTrades),
        //     (Bitmex, "btc", "usdt", Perpetual, PublicTrades),
        //     (Okx, "eth", "usdt", Spot, PublicTrades),
        //     (Okx, "eth", "usdt", Perpetual, PublicTrades),
        //     (Bitmex, "eth", "usdt", Perpetual, PublicTrades),
        // ],

        // Batch notes:
        // OKX open interest updates for BTCUSDT perpetual.
        vec![
            (Okx, "btc", "usdt", Perpetual, OpenInterest),
        ],

        // Batch notes:
        // OKX liquidation updates for BTCUSDT perpetual.
        vec![
            (Okx, "btc", "usdt", Perpetual, Liquidations),
        ],

        // Batch notes:
        // OKX CVD derived from trade stream for BTCUSDT perpetual.
        vec![
            (Okx, "btc", "usdt", Perpetual, CumulativeVolumeDelta),
        ],
    ]).await.unwrap();

    // Build a lookup map of which subscriptions we have per exchange
    // So we can provide better context in error messages
    let mut exchange_subscriptions: HashMap<ExchangeId, Vec<&str>> = HashMap::new();

    for exchange in streams.open_interests.keys() {
        exchange_subscriptions.entry(*exchange).or_default().push("open_interest");
    }
    for exchange in streams.liquidations.keys() {
        exchange_subscriptions.entry(*exchange).or_default().push("liquidations");
    }
    for exchange in streams.cvds.keys() {
        exchange_subscriptions.entry(*exchange).or_default().push("cumulative_volume_delta");
    }
    for exchange in streams.trades.keys() {
        exchange_subscriptions.entry(*exchange).or_default().push("trades");
    }
    for exchange in streams.l1s.keys() {
        exchange_subscriptions.entry(*exchange).or_default().push("order_books_l1");
    }
    for exchange in streams.l2s.keys() {
        exchange_subscriptions.entry(*exchange).or_default().push("order_books_l2");
    }

    // Select all WebSocket streams with an enhanced error handler that includes context
    exchange_subscriptions
        .entry(ExchangeId::BinanceFuturesUsd)
        .or_default()
        .push("open_interest_polling");

    let combined_results = stream::select(
        streams.select_all::<MarketStreamResult<MarketDataInstrument, DataKind>>(),
        binance_open_interest_stream(),
    );

    let market_stream = combined_results.with_error_handler(move |error| {
            // Try to extract exchange context from the error message if possible
            let error_str = format!("{:?}", error);

            // Ignore Bybit heartbeat 'pong' payloads that are known non-JSON responses
            if error_str.contains("payload: pong") {
                return;
            }

            // Ignore OKX liquidation subscription IDs (liquidation-orders|SWAP is correct)
            if error_str.contains("liquidation-orders|SWAP") {
                return;
            }

            // Find which exchange(s) might be related to this error
            for (exchange, kinds) in &exchange_subscriptions {
                let exchange_name = format!("{:?}", exchange);
                // Check if error message mentions this exchange or its subscriptions
                if error_str.contains(&exchange_name) || kinds.iter().any(|k| error_str.contains(k)) {
                    warn!(
                        exchange = %exchange,
                        subscriptions = ?kinds,
                        error = ?error,
                        "MarketStream error"
                    );
                    return;
                }
            }

            // If we can't determine the exchange, log with available subscriptions info
            warn!(
                exchanges = ?exchange_subscriptions.keys().collect::<Vec<_>>(),
                error = ?error,
                "MarketStream error (exchange unknown)"
            );
        });

    futures::pin_mut!(market_stream);

    // Track last emission time for CVD events per (exchange, instrument) pair (for throttling)
    let mut last_cvd_emission: HashMap<(ExchangeId, MarketDataInstrument), Instant> = HashMap::new();
    let cvd_throttle_duration = Duration::from_secs(5);

    while let Some(event) = market_stream.next().await {
        match event {
            Event::Reconnecting(exchange) => {
                warn!("Reconnecting to {exchange:?}");
            }
            Event::Item(market_event) => {
                // Color-code by event type, with exchange prefix
                let exchange_prefix = format!("[{}]", market_event.exchange);
                match &market_event.kind {
                    DataKind::Liquidation(_) => {
                        // Bright red for liquidations
                        println!("\x1b[91m{} {market_event:?}\x1b[0m", exchange_prefix);
                    }
                    DataKind::OpenInterest(_) => {
                        // Bright cyan for open interest
                        println!("\x1b[96m{} {market_event:?}\x1b[0m", exchange_prefix);
                    }
                    DataKind::CumulativeVolumeDelta(_) => {
                        // Bright yellow for CVD - with throttling per (exchange, instrument)
                        let now = Instant::now();
                        let key = (market_event.exchange, market_event.instrument.clone());
                        let should_emit = last_cvd_emission
                            .get(&key)
                            .map(|last| now.duration_since(*last) >= cvd_throttle_duration)
                            .unwrap_or(true); // Emit first event immediately

                        if should_emit {
                            println!("\x1b[93m{} {market_event:?}\x1b[0m", exchange_prefix);
                            last_cvd_emission.insert(key, now);
                        }
                    }
                    _ => {
                        // Default color for other events
                        info!("{} {market_event:?}", exchange_prefix);
                    }
                }
            }
        }
    }
}

// Binance REST API response for open interest
#[derive(Debug, Deserialize)]
struct BinanceOpenInterestResponse {
    #[serde(
        rename = "openInterest",
        deserialize_with = "barter_integration::de::de_str"
    )]
    open_interest: f64,
    time: i64,
}

/// Build a combined Stream of Binance open-interest polling events (REST fallback).
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

/// Poll Binance REST API for open interest every 10 seconds.
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
        (client, url, interval(Duration::from_secs(10)), instrument),
        move |(client, url, mut timer, instrument)| async move {
            // Wait for next tick (first one completes immediately)
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
                                    debug!("Polled Binance open interest: {:?}", data);

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

// Initialise an INFO `Subscriber` for `Tracing` logs and install it as the global default.
fn init_logging() {
    tracing_subscriber::fmt()
        // Filter messages based on the INFO
        .with_env_filter(
            tracing_subscriber::filter::EnvFilter::builder()
                .with_default_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        // Enable colours
        .with_ansi(true)
        // Use compact formatting for better readability with colored output
        .compact()
        // Install this Tracing subscriber as global default
        .init()
}
