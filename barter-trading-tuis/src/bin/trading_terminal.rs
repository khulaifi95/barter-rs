/// Unified Trading Terminal (Market State + Physics)
///
/// Primary UI for the Market State Engine, with tabbed views for
/// Global Radar, Execution, and Debug.
use std::{
    collections::HashMap,
    error::Error,
    io,
    sync::{
        atomic::{AtomicBool, AtomicI64, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};

use barter_trading_tuis::{
    Aggregator, ConnectionStatus, OrchestratorResult, WebSocketClient, WebSocketConfig,
    ticker_to_binance_symbol, Candle1m, TradMarketState, TradeData,
};
use barter_trading_tuis::shared::{
    audit::AuditLogger,
    config::Config,
    market_state::{ConfigProvider, Signal, TradMarketStatus},
    options_state::OptionsContextBuilder,
    orchestrator::StateOrchestrator,
    snapshot_bridge::build_market_data_input,
    state::TickerSnapshot,
    types::TradTickData,
};
use barter_trading_tuis::views::{ActiveView, ViewContext};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    Terminal,
};
use rustls::crypto::ring::default_provider;
use tokio::sync::{Mutex, watch};
use tokio_tungstenite::{connect_async, tungstenite::Message};

static TICKERS: OnceLock<Vec<String>> = OnceLock::new();

/// Get tickers from TICKERS env var (default: BTC,ETH,SOL)
fn get_tickers() -> Vec<String> {
    std::env::var("TICKERS")
        .unwrap_or_else(|_| "BTC,ETH,SOL".to_string())
        .split(',')
        .map(|s| s.trim().to_uppercase())
        .collect()
}

/// Get WebSocket URL from WS_URL env var (default: ws://127.0.0.1:9001)
fn get_ws_url() -> String {
    std::env::var("WS_URL").unwrap_or_else(|_| "ws://127.0.0.1:9001".to_string())
}

const BINANCE_PRICE_STALE_SECS: f64 = 2.0;

fn binance_perp_age(snapshot: &TickerSnapshot) -> Option<f64> {
    snapshot
        .exchange_health
        .iter()
        .filter_map(|(name, age)| {
            let name = name.to_lowercase();
            if name.contains("binancefutures")
                || name.contains("binancefuturesusd")
                || (name.contains("binance") && name.contains("futures"))
            {
                Some(*age)
            } else {
                None
            }
        })
        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .or_else(|| {
            snapshot
                .exchange_health
                .iter()
                .filter_map(|(name, age)| {
                    if name.to_lowercase().contains("binance") {
                        Some(*age)
                    } else {
                        None
                    }
                })
                .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        })
}

fn resolve_spot_price(snapshot: &TickerSnapshot) -> f64 {
    let binance_fresh = binance_perp_age(snapshot)
        .map(|age| age <= BINANCE_PRICE_STALE_SECS)
        .unwrap_or(true);
    if binance_fresh {
        snapshot
            .binance_perp_last
            .filter(|&p| p > 0.0)
            .or(snapshot.latest_price)
            .unwrap_or(0.0)
    } else {
        snapshot.latest_price.unwrap_or(0.0)
    }
}

fn tickers() -> &'static [String] {
    TICKERS.get_or_init(get_tickers)
}

/// Spawn a Binance 1m kline stream for a ticker (perps)
async fn run_binance_kline_stream(ticker: &str, agg: Arc<Mutex<Aggregator>>) {
    let symbol = ticker_to_binance_symbol(ticker).to_lowercase();
    let url = format!("wss://fstream.binance.com/ws/{}@kline_1m", symbol);

    loop {
        // Resync from REST on reconnect to avoid drift
        {
            let mut guard = agg.lock().await;
            let _ = guard.backfill_1m_klines(&[ticker]).await;
        }

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
                                                let mut guard = agg.lock().await;
                                                guard.push_1m_candle(ticker, candle);
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
                eprintln!("[kline-ws] {} connect error: {}", ticker, e);
            }
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Install rustls crypto provider (required for TLS fetches / wss)
    if let Err(e) = default_provider().install_default() {
        eprintln!("[crypto] provider install: {:?}", e);
    }

    // Setup panic hook to restore terminal on crash
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic_info);
    }));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let aggregator = Arc::new(Mutex::new(Aggregator::new()));
    let connected = Arc::new(AtomicBool::new(false));

    // Watch channel for sharing MarketState from orchestrator to UI
    let (state_tx, state_rx) = watch::channel::<HashMap<String, OrchestratorResult>>(HashMap::new());

    // Backfill 1m klines for accurate RV/ATR on startup
    {
        let ticker_list: Vec<&str> = tickers().iter().map(|s| s.as_str()).collect();
        let mut guard = aggregator.lock().await;
        let _ = guard.backfill_1m_klines(&ticker_list).await;
    }

    // Start Binance 1m kline streams
    for ticker in tickers() {
        let agg = Arc::clone(&aggregator);
        let ticker = ticker.clone();
        tokio::spawn(async move {
            run_binance_kline_stream(&ticker, agg).await;
        });
    }

    let ws_url = get_ws_url();
    let client =
        WebSocketClient::with_config(WebSocketConfig::new(ws_url).with_channel_buffer_size(50_000));
    let (mut event_rx, mut status_rx) = client.start();

    let options_cache = Arc::new(Mutex::new(HashMap::new()));
    let trad_state = Arc::new(Mutex::new(TradMarketState::new()));
    let trad_last_ms = Arc::new(AtomicI64::new(0));

    {
        let agg = Arc::clone(&aggregator);
        let options_cache = Arc::clone(&options_cache);
        let trad_state = Arc::clone(&trad_state);
        let trad_last_ms = Arc::clone(&trad_last_ms);
        tokio::spawn(async move {
            let options_builder = OptionsContextBuilder::new();
            while let Some(event) = event_rx.recv().await {
                if event.kind == "trad_tick" {
                    if let Ok(tick) = serde_json::from_value::<TradTickData>(event.data.clone()) {
                        if tick.ts > 0 {
                            trad_last_ms.store(tick.ts, Ordering::Relaxed);
                        }
                        let size = if tick.sz > 0.0 { tick.sz } else { 1.0 };
                        let mut trad_guard = trad_state.lock().await;
                        match tick.symbol.as_str() {
                            "ES" => trad_guard.update_es_tick(tick.px, size, tick.ts),
                            "NQ" => trad_guard.update_nq_tick(tick.px, size, tick.ts),
                            _ => {}
                        }
                    }
                    continue;
                }
                if event.kind == "options_chain" {
                    if let Ok(chain) = serde_json::from_value::<barter_trading_tuis::shared::market_state::OptionsChain>(event.data.clone()) {
                        let ticker = event.instrument.base.to_uppercase();
                        let spot = {
                            let guard = agg.lock().await;
                            guard
                                .snapshot()
                                .tickers
                                .get(&ticker)
                                .map(resolve_spot_price)
                                .unwrap_or(0.0)
                        };
                        if spot > 0.0 {
                            let ctx = options_builder.build(&chain, spot);
                            let mut cache = options_cache.lock().await;
                            cache.insert(ticker, ctx);
                        }
                    }
                    continue;
                }

                if event.kind == "trade" && event.instrument.base.to_lowercase() == "btc" {
                    if let Ok(trade) = serde_json::from_value::<TradeData>(event.data.clone()) {
                        let ts = event.time_exchange.timestamp_millis();
                        trad_state
                            .lock()
                            .await
                            .update_btc_trade(trade.price, trade.amount, ts);
                    }
                }

                let mut guard = agg.lock().await;
                guard.process_event(event);
            }
        });
    }

    {
        let connected_flag = Arc::clone(&connected);
        tokio::spawn(async move {
            while let Some(status) = status_rx.recv().await {
                match status {
                    ConnectionStatus::Connected => connected_flag.store(true, Ordering::Relaxed),
                    ConnectionStatus::Disconnected | ConnectionStatus::Reconnecting => {
                        connected_flag.store(false, Ordering::Relaxed)
                    }
                }
            }
        });
    }

    // Market State Engine - calculates MarketState every 200ms
    {
        let agg = Arc::clone(&aggregator);
        let options_cache = Arc::clone(&options_cache);
        let trad_last_ms = Arc::clone(&trad_last_ms);
        let state_tx = state_tx.clone();
        tokio::spawn(async move {
            let config = Config::load().unwrap_or_else(|e| {
                tracing::warn!("Failed to load config: {}, using defaults", e);
                Config::default()
            });
            let logger = AuditLogger::new(&config.thresholds().audit);
            let trad_fresh_ms = config.freshness(Signal::TradMarkets).as_millis() as u64;
            let mut orchestrator = StateOrchestrator::new(config, logger);
            let calc_interval = Duration::from_millis(200);
            let mut last_calc = Instant::now();

            loop {
                if last_calc.elapsed() >= calc_interval {
                    let trad_ts = trad_last_ms.load(Ordering::Relaxed);
                    let trad_status = if trad_ts <= 0 {
                        TradMarketStatus::Unavailable
                    } else {
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        let age_ms = (now_ms - trad_ts).max(0) as u64;
                        let max_age = trad_fresh_ms;
                        if age_ms > max_age {
                            TradMarketStatus::Stale
                        } else {
                            TradMarketStatus::Live
                        }
                    };

                    let snapshot = {
                        let guard = agg.lock().await;
                        guard.snapshot()
                    };
                    let options_cache = options_cache.lock().await;
                    let mut state_results = HashMap::new();

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
                        if let Some(ctx) = options_cache.get(ticker) {
                            input.options_context = Some(ctx.clone());
                        }
                        let result = orchestrator.calculate(&input);
                        state_results.insert(ticker.clone(), result);
                    }
                    let _ = state_tx.send(state_results);
                    last_calc = Instant::now();
                }

                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
    }

    let mut active_view = ActiveView::GlobalRadar;
    let mut focused_index = 0usize;
    let tick_rate = Duration::from_millis(200);
    let mut last_tick = Instant::now();

    loop {
        if last_tick.elapsed() >= tick_rate {
            let snapshot = {
                let guard = aggregator.lock().await;
                guard.snapshot()
            };
            let focused_ticker = tickers()
                .get(focused_index)
                .map(|s| s.as_str())
                .unwrap_or("BTC");
            let state_map = state_rx.borrow();
            let state = state_map.get(focused_ticker);

            let trad_signals = {
                let trad_guard = trad_state.lock().await;
                trad_guard.get_signals()
            };
            let ctx = ViewContext {
                snapshot: &snapshot,
                state,
                focused_ticker,
                connected: connected.load(Ordering::Relaxed),
                trad_signals: Some(trad_signals),
            };

            terminal.draw(|f| {
                barter_trading_tuis::views::render(f, f.area(), active_view, &ctx);
            })?;

            last_tick = Instant::now();
        }

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('1') => active_view = ActiveView::GlobalRadar,
                    KeyCode::Char('2') => active_view = ActiveView::Execution,
                    KeyCode::Char('3') => active_view = ActiveView::Debug,
                    KeyCode::Left => {
                        if focused_index > 0 {
                            focused_index -= 1;
                        }
                    }
                    KeyCode::Right => {
                        if focused_index + 1 < tickers().len() {
                            focused_index += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}
