//! Load Test Harness for barter-data-server
//!
//! Generates synthetic MarketEventMessage at configurable rates to validate
//! server performance under load without connecting to real exchanges.
//!
//! Usage:
//!   cargo run --bin load-test -- --rate 5000 --duration 60
//!
//! Environment variables:
//!   LOAD_TEST_RATE: Events per second (default: 5000)
//!   LOAD_TEST_DURATION_SECS: Test duration in seconds (default: 60)
//!   LOAD_TEST_TICKERS: Comma-separated tickers (default: BTC,ETH,SOL)

use bytes::Bytes;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};
use tracing::{info, warn};

// Re-use types from main server
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MarketEventMessage {
    time_exchange: chrono::DateTime<Utc>,
    time_received: chrono::DateTime<Utc>,
    exchange: String,
    instrument: InstrumentInfo,
    kind: String,
    data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstrumentInfo {
    base: String,
    quote: String,
    kind: String,
}

// Metrics
static EVENTS_GENERATED: AtomicU64 = AtomicU64::new(0);
static EVENTS_BROADCAST: AtomicU64 = AtomicU64::new(0);
static EVENTS_DROPPED: AtomicU64 = AtomicU64::new(0);
static LATENCY_SUM_US: AtomicU64 = AtomicU64::new(0);
static LATENCY_COUNT: AtomicU64 = AtomicU64::new(0);
static LATENCY_MAX_US: AtomicU64 = AtomicU64::new(0);

fn generate_synthetic_event(ticker: &str, exchange: &str) -> MarketEventMessage {
    let now = Utc::now();
    MarketEventMessage {
        time_exchange: now,
        time_received: now,
        exchange: exchange.to_string(),
        instrument: InstrumentInfo {
            base: ticker.to_string(),
            quote: "USD".to_string(),
            kind: "Perpetual".to_string(),
        },
        kind: "trade".to_string(),
        data: serde_json::json!({
            "price": 50000.0 + (rand_u64() % 1000) as f64,
            "amount": 0.1 + (rand_u64() % 100) as f64 / 100.0,
            "side": if rand_u64() % 2 == 0 { "buy" } else { "sell" }
        }),
    }
}

// Simple PRNG for test data variety
fn rand_u64() -> u64 {
    static SEED: AtomicU64 = AtomicU64::new(12345);
    let mut s = SEED.load(Ordering::Relaxed);
    s ^= s << 13;
    s ^= s >> 7;
    s ^= s << 17;
    SEED.store(s, Ordering::Relaxed);
    s
}

fn serialize_event(event: &MarketEventMessage) -> Option<Bytes> {
    serde_json::to_vec(event).ok().map(Bytes::from)
}

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("load_test=info".parse().unwrap()),
        )
        .init();

    // Parse configuration
    let rate: u64 = std::env::var("LOAD_TEST_RATE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000);

    let duration_secs: u64 = std::env::var("LOAD_TEST_DURATION_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);

    let tickers: Vec<String> = std::env::var("LOAD_TEST_TICKERS")
        .unwrap_or_else(|_| "BTC,ETH,SOL".to_string())
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();

    let exchanges = vec!["BinanceFuturesUsd", "Okx", "BybitPerpetualsUsd"];

    info!("=== LOAD TEST CONFIGURATION ===");
    info!("Rate: {} events/sec", rate);
    info!("Duration: {} seconds", duration_secs);
    info!("Tickers: {:?}", tickers);
    info!("Exchanges: {:?}", exchanges);
    info!("================================");

    // Create broadcast channel (simulates server's tx_trades)
    let (tx, _rx) = broadcast::channel::<Bytes>(100_000);

    // Create aggregator channel (simulates server's agg_event_tx)
    let (agg_tx, mut agg_rx) = mpsc::channel::<MarketEventMessage>(10_000);

    // Spawn aggregator consumer (simulates aggregator task)
    tokio::spawn(async move {
        while let Some(_event) = agg_rx.recv().await {
            // Simulate aggregator processing
            tokio::task::yield_now().await;
        }
    });

    // Spawn metrics reporter
    let metrics_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            let generated = EVENTS_GENERATED.swap(0, Ordering::Relaxed);
            let broadcast = EVENTS_BROADCAST.swap(0, Ordering::Relaxed);
            let dropped = EVENTS_DROPPED.swap(0, Ordering::Relaxed);
            let latency_sum = LATENCY_SUM_US.swap(0, Ordering::Relaxed);
            let latency_count = LATENCY_COUNT.swap(0, Ordering::Relaxed);
            let latency_max = LATENCY_MAX_US.swap(0, Ordering::Relaxed);

            let latency_avg = if latency_count > 0 {
                latency_sum / latency_count
            } else {
                0
            };

            info!(
                "METRICS: generated={}/10s ({:.1}/s), broadcast={}, dropped={}, latency_avg={}us, latency_max={}us",
                generated,
                generated as f64 / 10.0,
                broadcast,
                dropped,
                latency_avg,
                latency_max
            );

            // Report memory usage (RSS)
            #[cfg(target_os = "linux")]
            {
                if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
                    for line in status.lines() {
                        if line.starts_with("VmRSS:") {
                            info!("MEMORY: {}", line);
                            break;
                        }
                    }
                }
            }

            #[cfg(target_os = "macos")]
            {
                // On macOS, just report we're running
                info!("MEMORY: (use Activity Monitor or `ps` for RSS on macOS)");
            }
        }
    });

    // Calculate interval between events
    let interval_us = 1_000_000 / rate;
    let interval = Duration::from_micros(interval_us);

    info!("Starting load test at {} events/sec (interval: {:?})", rate, interval);

    let start = Instant::now();
    let duration = Duration::from_secs(duration_secs);
    let mut ticker_idx = 0;
    let mut exchange_idx = 0;

    // Main event generation loop
    while start.elapsed() < duration {
        let event_start = Instant::now();

        // Generate synthetic event
        let ticker = &tickers[ticker_idx % tickers.len()];
        let exchange = exchanges[exchange_idx % exchanges.len()];
        let event = generate_synthetic_event(ticker, exchange);

        EVENTS_GENERATED.fetch_add(1, Ordering::Relaxed);

        // Serialize and broadcast (simulates hot path)
        if let Some(bytes) = serialize_event(&event) {
            if tx.send(bytes).is_ok() {
                EVENTS_BROADCAST.fetch_add(1, Ordering::Relaxed);
            }
        }

        // Send to aggregator (simulates agg_tx.try_send)
        if agg_tx.try_send(event).is_err() {
            EVENTS_DROPPED.fetch_add(1, Ordering::Relaxed);
        }

        // Track latency
        let latency_us = event_start.elapsed().as_micros() as u64;
        LATENCY_SUM_US.fetch_add(latency_us, Ordering::Relaxed);
        LATENCY_COUNT.fetch_add(1, Ordering::Relaxed);

        // Update max latency (CAS loop)
        let mut current_max = LATENCY_MAX_US.load(Ordering::Relaxed);
        while latency_us > current_max {
            match LATENCY_MAX_US.compare_exchange_weak(
                current_max,
                latency_us,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => current_max = x,
            }
        }

        // Rotate through tickers and exchanges
        ticker_idx += 1;
        exchange_idx += 1;

        // Sleep to maintain target rate
        let elapsed = event_start.elapsed();
        if elapsed < interval {
            tokio::time::sleep(interval - elapsed).await;
        }
    }

    // Final metrics
    let total_time = start.elapsed();
    info!("=== LOAD TEST COMPLETE ===");
    info!("Total time: {:?}", total_time);
    info!(
        "Total events generated: {}",
        EVENTS_GENERATED.load(Ordering::Relaxed)
    );
    info!(
        "Total events broadcast: {}",
        EVENTS_BROADCAST.load(Ordering::Relaxed)
    );
    info!(
        "Total events dropped: {}",
        EVENTS_DROPPED.load(Ordering::Relaxed)
    );

    let effective_rate =
        EVENTS_BROADCAST.load(Ordering::Relaxed) as f64 / total_time.as_secs_f64();
    info!("Effective rate: {:.1} events/sec", effective_rate);

    if EVENTS_DROPPED.load(Ordering::Relaxed) > 0 {
        warn!(
            "WARNING: {} events were dropped due to backpressure",
            EVENTS_DROPPED.load(Ordering::Relaxed)
        );
        warn!("Consider increasing AGG_EVENT_BUFFER or reducing event rate");
    }

    info!("=============================");

    // Stop metrics reporter
    metrics_handle.abort();
}
