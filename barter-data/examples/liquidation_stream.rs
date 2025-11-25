use barter_data::{
    exchange::{binance::futures::BinanceFuturesUsd, bybit::futures::BybitPerpetualsUsd, okx::Okx},
    streams::{Streams, reconnect::stream::ReconnectingStream},
    subscription::liquidation::Liquidations,
};
use barter_instrument::instrument::market_data::kind::MarketDataInstrumentKind;
use futures_util::StreamExt;
use tracing::{info, warn};

#[rustfmt::skip]
#[tokio::main]
async fn main() {
    // Initialise INFO Tracing log subscriber
    init_logging();

    println!("\n🔴 Starting Liquidation Data Stream...");
    println!("📡 Connecting to Binance, Bybit, and OKX...");
    println!("💥 Waiting for liquidation events (BTC only)...\n");

    // Subscribe to liquidations from 3 exchanges for BTC
    let streams = Streams::<Liquidations>::builder()
        .subscribe([
            (BinanceFuturesUsd::default(), "btc", "usdt", MarketDataInstrumentKind::Perpetual, Liquidations),
        ])
        .subscribe([
            (BybitPerpetualsUsd::default(), "btc", "usdt", MarketDataInstrumentKind::Perpetual, Liquidations),
        ])
        .subscribe([
            (Okx::default(), "btc", "usdt", MarketDataInstrumentKind::Perpetual, Liquidations),
        ])
        .init()
        .await
        .unwrap();

    // Select and merge every exchange Stream
    let mut joined_stream = streams
        .select_all()
        .with_error_handler(|error| warn!(?error, "MarketStream generated error"));

    // Stream liquidation events
    let mut event_count = 0;
    while let Some(event) = joined_stream.next().await {
        event_count += 1;
        info!("💥 LIQUIDATION #{} => {event:?}", event_count);
    }
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
        // Disable colours on release builds
        .with_ansi(cfg!(debug_assertions))
        // Install this Tracing subscriber as global default
        .init()
}
