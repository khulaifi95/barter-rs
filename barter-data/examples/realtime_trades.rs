use barter_data::{
    exchange::{binance::futures::BinanceFuturesUsd, bybit::futures::BybitPerpetualsUsd, okx::Okx},
    streams::{Streams, reconnect::stream::ReconnectingStream},
    subscription::trade::PublicTrades,
};
use barter_instrument::instrument::market_data::kind::MarketDataInstrumentKind;
use chrono::Local;
use futures::StreamExt;
use std::collections::HashMap;
use tracing::warn;

#[rustfmt::skip]
#[tokio::main]
async fn main() {
    // Initialise INFO Tracing log subscriber
    init_logging();

    println!("\n════════════════════════════════════════════════════════════");
    println!("🚀 REAL-TIME TRADE TRACKER");
    println!("════════════════════════════════════════════════════════════");
    println!("📡 Connecting to:");
    println!("   • Binance Futures");
    println!("   • Bybit Perpetuals");
    println!("   • OKX");
    println!("");
    println!("🎯 Tracking: BTC and ETH perpetual trades");
    println!("⏰ Current Time: {}", Local::now().format("%Y-%m-%d %H:%M:%S"));
    println!("════════════════════════════════════════════════════════════");
    println!("\n💡 Streaming real-time trades...\n");

    // Track statistics
    let mut stats = TradeStats::new();

    // Subscribe to trades from 3 exchanges for BTC and ETH
    let streams = match Streams::<PublicTrades>::builder()
        // Binance BTC and ETH
        .subscribe([
            (BinanceFuturesUsd::default(), "btc", "usdt", MarketDataInstrumentKind::Perpetual, PublicTrades),
            (BinanceFuturesUsd::default(), "eth", "usdt", MarketDataInstrumentKind::Perpetual, PublicTrades),
        ])
        // Bybit BTC and ETH
        .subscribe([
            (BybitPerpetualsUsd::default(), "btc", "usdt", MarketDataInstrumentKind::Perpetual, PublicTrades),
            (BybitPerpetualsUsd::default(), "eth", "usdt", MarketDataInstrumentKind::Perpetual, PublicTrades),
        ])
        // OKX BTC and ETH
        .subscribe([
            (Okx::default(), "btc", "usdt", MarketDataInstrumentKind::Perpetual, PublicTrades),
            (Okx::default(), "eth", "usdt", MarketDataInstrumentKind::Perpetual, PublicTrades),
        ])
        .init()
        .await
    {
        Ok(streams) => {
            println!("✅ Successfully connected to all exchanges!\n");
            streams
        },
        Err(e) => {
            eprintln!("Failed to initialize streams: {}", e);
            return;
        }
    };

    // Select and merge every exchange Stream
    let mut joined_stream = streams
        .select_all()
        .with_error_handler(|error| warn!(?error, "MarketStream generated error"));

    // Stream trades in real-time
    while let Some(event) = joined_stream.next().await {
        // Access the trade data properly from the Event wrapper
        if let barter_data::streams::reconnect::Event::Item(market_event) = event {
            stats.update(&market_event);

            // Format and display the trade
            let timestamp = Local::now().format("%H:%M:%S%.3f");
            let exchange = market_event.exchange;
            let symbol = format!("{}-{}",
                market_event.instrument.base.as_ref().to_uppercase(),
                market_event.instrument.quote.as_ref().to_uppercase()
            );

            // Determine trade direction
            let side_emoji = if market_event.kind.side.is_buy() {
                "🟢"  // Buy trade (taker bought)
            } else {
                "🔴"  // Sell trade (taker sold)
            };

            // Format volume
            let volume_usd = market_event.kind.quantity * market_event.kind.price;
            let volume_display = if volume_usd >= 100_000.0 {
                format!(" 💰 ${:.0}K", volume_usd / 1_000.0)
            } else {
                format!(" ${:.0}", volume_usd)
            };

            // Only show trades above a certain threshold to reduce noise
            if volume_usd >= 1000.0 {  // Only show trades >= $1,000
                println!("[{}] {} {} | {} | Price: ${:.2} | Size: {:.4} | Volume:{}",
                    timestamp,
                    side_emoji,
                    exchange,
                    symbol,
                    market_event.kind.price,
                    market_event.kind.quantity,
                    volume_display
                );
            }

            // Print statistics every 100 trades
            if stats.total_trades % 100 == 0 && stats.total_trades > 0 {
                stats.print_summary();
            }
        }
    }
}

// Track trade statistics
struct TradeStats {
    total_trades: u64,
    exchange_counts: HashMap<String, u64>,
    symbol_counts: HashMap<String, u64>,
    total_volume_usd: f64,
    largest_trade_usd: f64,
    buy_count: u64,
    sell_count: u64,
    start_time: chrono::DateTime<Local>,
}

impl TradeStats {
    fn new() -> Self {
        Self {
            total_trades: 0,
            exchange_counts: HashMap::new(),
            symbol_counts: HashMap::new(),
            total_volume_usd: 0.0,
            largest_trade_usd: 0.0,
            buy_count: 0,
            sell_count: 0,
            start_time: Local::now(),
        }
    }

    fn update(
        &mut self,
        event: &barter_data::event::MarketEvent<
            barter_instrument::instrument::market_data::MarketDataInstrument,
            barter_data::subscription::trade::PublicTrade,
        >,
    ) {
        self.total_trades += 1;

        let exchange = event.exchange.to_string();
        let symbol = format!(
            "{}-{}",
            event.instrument.base.as_ref().to_uppercase(),
            event.instrument.quote.as_ref().to_uppercase()
        );
        let volume_usd = event.kind.quantity * event.kind.price;

        *self.exchange_counts.entry(exchange).or_insert(0) += 1;
        *self.symbol_counts.entry(symbol).or_insert(0) += 1;
        self.total_volume_usd += volume_usd;

        if event.kind.side.is_buy() {
            self.buy_count += 1;
        } else {
            self.sell_count += 1;
        }

        if volume_usd > self.largest_trade_usd {
            self.largest_trade_usd = volume_usd;
        }
    }

    fn print_summary(&self) {
        let runtime = Local::now().signed_duration_since(self.start_time);
        let runtime_secs = runtime.num_seconds() as f64;

        println!("\n╔══════════════════════════════════════════════════════════╗");
        println!("║                    TRADE STATISTICS                       ║");
        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║ Runtime: {:.0} seconds", runtime_secs);
        println!("║ Total Trades: {}", self.total_trades);
        println!(
            "║ Rate: {:.1} trades/second",
            self.total_trades as f64 / runtime_secs
        );
        println!("║ Total Volume: ${:.2}", self.total_volume_usd);
        println!("║ Largest Trade: ${:.2}", self.largest_trade_usd);
        println!("║");
        println!("║ Buy/Sell Ratio:");
        println!(
            "║   🟢 Buys: {} ({:.1}%)",
            self.buy_count,
            (self.buy_count as f64 / self.total_trades as f64) * 100.0
        );
        println!(
            "║   🔴 Sells: {} ({:.1}%)",
            self.sell_count,
            (self.sell_count as f64 / self.total_trades as f64) * 100.0
        );
        println!("║");
        println!("║ By Exchange:");
        for (exchange, count) in &self.exchange_counts {
            println!(
                "║   • {}: {} ({:.1}%)",
                exchange,
                count,
                (*count as f64 / self.total_trades as f64) * 100.0
            );
        }
        println!("║");
        println!("║ By Symbol:");
        for (symbol, count) in &self.symbol_counts {
            println!(
                "║   • {}: {} ({:.1}%)",
                symbol,
                count,
                (*count as f64 / self.total_trades as f64) * 100.0
            );
        }
        println!("╚══════════════════════════════════════════════════════════╝\n");
    }
}

// Initialise an INFO `Subscriber` for `Tracing` logs
fn init_logging() {
    tracing_subscriber::fmt()
        // Filter messages based on the INFO level
        .with_env_filter(
            tracing_subscriber::filter::EnvFilter::builder()
                .with_default_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        // Use colored output in debug mode
        .with_ansi(cfg!(debug_assertions))
        // Install this Tracing subscriber as global default
        .init()
}
