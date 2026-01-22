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
    Aggregator, ConnectionStatus, OrchestratorResult, TickerSnapshot, WebSocketClient,
    WebSocketConfig, TradMarketState, TradeData,
};
use barter_trading_tuis::shared::types::TradTickData;
use barter_trading_tuis::views::{ActiveView, ViewContext};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    Terminal,
};
use serde::Deserialize;
use rustls::crypto::ring::default_provider;
use tokio::sync::{Mutex, watch};

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

#[derive(Debug, Deserialize)]
struct OrchestratorMessage {
    ticker: String,
    result: OrchestratorResult,
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

    let ws_url = get_ws_url();
    let client =
        WebSocketClient::with_config(WebSocketConfig::new(ws_url).with_channel_buffer_size(200_000));
    let (mut event_rx, mut status_rx) = client.start();

    let trad_state = Arc::new(Mutex::new(TradMarketState::new()));
    let trad_last_ms = Arc::new(AtomicI64::new(0));

    {
        let agg = Arc::clone(&aggregator);
        let trad_state = Arc::clone(&trad_state);
        let trad_last_ms = Arc::clone(&trad_last_ms);
        tokio::spawn(async move {
            let mut state_map: HashMap<String, OrchestratorResult> = HashMap::new();
            while let Some(event) = event_rx.recv().await {
                if event.kind == "orchestrator_result" {
                    if let Ok(msg) = serde_json::from_value::<OrchestratorMessage>(event.data.clone()) {
                        state_map.insert(msg.ticker, msg.result);
                        let _ = state_tx.send(state_map.clone());
                    }
                    continue;
                }
                if event.kind == "trad_tick" {
                    if let Ok(tick) = serde_json::from_value::<TradTickData>(event.data.clone()) {
                        if tick.ts > 0 {
                            trad_last_ms.store(tick.ts, Ordering::Relaxed);
                        }
                        let size = if tick.sz > 0.0 { tick.sz } else { 1.0 };
                        let mut trad_guard = trad_state.lock().await;
                            match tick.symbol.as_str() {
                                "ES" => trad_guard.update_es_tick(tick.px, size, tick.ts, tick.bid, tick.ask, tick.vwap),
                                "NQ" => trad_guard.update_nq_tick(tick.px, size, tick.ts, tick.bid, tick.ask, tick.vwap),
                                _ => {}
                            }
                    }
                    continue;
                }
                if event.kind == "options_context" {
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
