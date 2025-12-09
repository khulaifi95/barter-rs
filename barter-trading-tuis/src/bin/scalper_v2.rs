/// Scalper V2 - World-Class Trading TUI
///
/// Optimized for 1-2 second decision making:
/// - BIG SIGNAL at top (direction + confidence)
/// - Consistent bi-directional bars everywhere
/// - Flow + Book confirmation side by side
/// - Clean visual hierarchy
use std::{
    collections::HashMap,
    error::Error,
    io,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use barter_trading_tuis::{
    AggregatedSnapshot, Aggregator, Candle1m, ConnectionStatus, DivergenceSignal, FlowSignal,
    Side, VolTrend, WebSocketClient, WebSocketConfig, ticker_to_binance_symbol,
    // Trad markets (ES/NQ) correlation
    IbkrConnectionStatus, TradMarketState, render_trad_markets_panel, spawn_ibkr_feed,
};
use rustls::crypto::ring::default_provider;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Terminal,
};
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use reqwest::Client;
use serde_json::Value;
use futures::StreamExt;

// ============================================================================
// COLORS - Balanced palette for easy reading
// ============================================================================
const C_BUY: Color = Color::Rgb(100, 220, 100);       // Green
const C_SELL: Color = Color::Rgb(220, 100, 100);      // Red
const C_NEUTRAL: Color = Color::Rgb(180, 180, 100);   // Yellow
const C_DIM: Color = Color::Rgb(120, 120, 120);       // Gray
const C_BRIGHT: Color = Color::Rgb(220, 220, 220);    // White
const C_ACCENT: Color = Color::Rgb(100, 180, 220);    // Cyan
const C_HEADER: Color = Color::Rgb(180, 130, 220);    // Purple

// ============================================================================
// CONSTANTS
// ============================================================================
const TICKERS: [&str; 3] = ["BTC", "ETH", "SOL"];
const SIGNAL_DEBOUNCE_MS: u64 = 1000;
const L2_INTERVAL_MS: u128 = 1500;
const BANNER_INTERVAL_MS: u128 = 1200;

// ============================================================================
// STATE STRUCTS
// ============================================================================
#[derive(Default)]
struct SignalState {
    divergence_start: HashMap<String, (DivergenceSignal, Instant)>,
    flow_start: HashMap<String, (FlowSignal, Instant)>,
}

#[derive(Default, Clone)]
struct VenueThrottle {
    last_value: f64,
    last_update: Option<Instant>,
}

impl VenueThrottle {
    fn get_throttled(&mut self, raw: f64, interval_ms: u128) -> f64 {
        let now = Instant::now();
        let time_ok = self.last_update
            .map(|t| now.duration_since(t).as_millis() >= interval_ms)
            .unwrap_or(true);
        if time_ok && raw > 0.0 {
            self.last_value = raw;
            self.last_update = Some(now);
        }
        self.last_value
    }
}

#[derive(Default)]
struct BarState {
    last_pressure: f64,
    last_update: Option<Instant>,
    l2_bnc: VenueThrottle,
    l2_bbt: VenueThrottle,
    l2_okx: VenueThrottle,
}

impl BarState {
    fn should_update_pressure(&mut self, pressure: f64) -> bool {
        let now = Instant::now();
        let time_ok = self.last_update
            .map(|t| now.duration_since(t).as_millis() >= BANNER_INTERVAL_MS)
            .unwrap_or(true);
        if time_ok {
            self.last_pressure = pressure;
            self.last_update = Some(now);
            true
        } else {
            false
        }
    }

    fn get_l2_throttled(&mut self, bnc: f64, bbt: f64, okx: f64) -> (f64, f64, f64) {
        (
            self.l2_bnc.get_throttled(bnc, L2_INTERVAL_MS),
            self.l2_bbt.get_throttled(bbt, L2_INTERVAL_MS),
            self.l2_okx.get_throttled(okx, L2_INTERVAL_MS),
        )
    }
}

impl SignalState {
    fn update_divergence(&mut self, ticker: &str, signal: DivergenceSignal) -> Option<DivergenceSignal> {
        let dominated = matches!(signal, DivergenceSignal::Bullish | DivergenceSignal::Bearish);
        if !dominated {
            self.divergence_start.remove(ticker);
            return None;
        }
        let now = Instant::now();
        match self.divergence_start.get(ticker) {
            Some((prev, start)) if *prev == signal => {
                if now.duration_since(*start).as_millis() >= SIGNAL_DEBOUNCE_MS as u128 {
                    Some(signal)
                } else {
                    None
                }
            }
            _ => {
                self.divergence_start.insert(ticker.to_string(), (signal, now));
                None
            }
        }
    }

    fn update_flow(&mut self, ticker: &str, signal: FlowSignal) -> FlowSignal {
        if matches!(signal, FlowSignal::Neutral) {
            self.flow_start.remove(ticker);
            return signal;
        }
        let now = Instant::now();
        match self.flow_start.get(ticker) {
            Some((prev, start)) if *prev == signal => {
                if now.duration_since(*start).as_millis() >= SIGNAL_DEBOUNCE_MS as u128 {
                    signal
                } else {
                    FlowSignal::Neutral
                }
            }
            _ => {
                self.flow_start.insert(ticker.to_string(), (signal, now));
                FlowSignal::Neutral
            }
        }
    }
}

// ============================================================================
// HELPERS
// ============================================================================
fn scale_number(v: f64) -> (f64, &'static str) {
    let abs = v.abs();
    if abs >= 1_000_000_000.0 { (v / 1_000_000_000.0, "B") }
    else if abs >= 1_000_000.0 { (v / 1_000_000.0, "M") }
    else if abs >= 1_000.0 { (v / 1_000.0, "K") }
    else { (v, "") }
}

fn get_ws_url() -> String {
    std::env::var("WS_URL").unwrap_or_else(|_| "ws://127.0.0.1:9001".to_string())
}

fn whale_threshold() -> f64 {
    std::env::var("WHALE_THRESHOLD").ok().and_then(|v| v.parse().ok()).unwrap_or(50_000.0)
}

async fn fetch_bvol24h(client: &Client) -> Result<f64, reqwest::Error> {
    let url = "https://www.bitmex.com/api/v1/instrument?symbol=.BVOL24H";
    let resp = client.get(url).send().await?.error_for_status()?;
    let v: Value = resp.json().await?;
    Ok(v.get(0).and_then(|o| o.get("lastPrice")).and_then(|p| p.as_f64()).unwrap_or(0.0))
}

/// Bi-directional bar: center = 50%, left = SELL, right = BUY
/// Returns (bar_string, color)
fn bidir_bar(value: f64, width: usize) -> (String, Color) {
    let half = width / 2;

    // Calculate fill from center
    let (left_fill, right_fill) = if value >= 50.0 {
        let fill = ((value - 50.0) / 50.0 * half as f64).ceil() as usize;
        (0, fill.min(half))
    } else {
        let fill = ((50.0 - value) / 50.0 * half as f64).ceil() as usize;
        (fill.min(half), 0)
    };

    // Build bar: [left_empty][left_fill]│[right_fill][right_empty]
    let left_empty = half.saturating_sub(left_fill);
    let right_empty = half.saturating_sub(right_fill);

    let bar = format!(
        "{}{}│{}{}",
        "░".repeat(left_empty),
        "█".repeat(left_fill),
        "█".repeat(right_fill),
        "░".repeat(right_empty)
    );

    let color = if value > 55.0 { C_BUY } else if value < 45.0 { C_SELL } else { C_NEUTRAL };
    (bar, color)
}

/// Compact bi-directional bar with label
fn bidir_bar_labeled(value: f64, width: usize, show_pct: bool) -> Vec<Span<'static>> {
    let (bar, color) = bidir_bar(value, width);
    let label = if value > 55.0 { "BUY" } else if value < 45.0 { "SELL" } else { "BAL" };

    if show_pct {
        vec![
            Span::styled(bar, Style::default().fg(color)),
            Span::styled(format!(" {:>2.0}% {}", value, label), Style::default().fg(color)),
        ]
    } else {
        vec![Span::styled(bar, Style::default().fg(color))]
    }
}

async fn run_binance_kline_stream(ticker: &str, agg: Arc<Mutex<Aggregator>>) {
    let symbol = ticker_to_binance_symbol(ticker).to_lowercase();
    let url = format!("wss://fstream.binance.com/ws/{}@kline_1m", symbol);
    loop {
        { let mut g = agg.lock().await; let _ = g.backfill_1m_klines(&[ticker]).await; }
        match connect_async(&url).await {
            Ok((ws, _)) => {
                let (_, mut read) = ws.split();
                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                                if let Some(k) = v.get("k") {
                                    let is_final = k.get("x").and_then(|b| b.as_bool()).unwrap_or(false);
                                    if !is_final { continue; }
                                    if let (Some(t), Some(o), Some(h), Some(l), Some(c), Some(vol)) = (
                                        k.get("t").and_then(|v| v.as_i64()),
                                        k.get("o").and_then(|v| v.as_str()),
                                        k.get("h").and_then(|v| v.as_str()),
                                        k.get("l").and_then(|v| v.as_str()),
                                        k.get("c").and_then(|v| v.as_str()),
                                        k.get("v").and_then(|v| v.as_str()),
                                    ) {
                                        if let Some(st) = chrono::DateTime::from_timestamp_millis(t) {
                                            if let (Ok(o), Ok(h), Ok(l), Ok(c), Ok(v)) =
                                                (o.parse::<f64>(), h.parse::<f64>(), l.parse::<f64>(), c.parse::<f64>(), vol.parse::<f64>()) {
                                                let candle = Candle1m { open: o, high: h, low: l, close: c, volume: v, start_time: st, is_complete: true };
                                                agg.lock().await.push_1m_candle(ticker, candle);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Ok(Message::Close(_)) | Err(_) => break,
                        _ => {}
                    }
                }
            }
            Err(e) => eprintln!("[kline] {} error: {}", ticker, e),
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

// ============================================================================
// MAIN
// ============================================================================
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = default_provider().install_default();

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(info);
    }));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let aggregator = Arc::new(Mutex::new(Aggregator::new()));
    let connected = Arc::new(AtomicBool::new(false));
    let focus_index = Arc::new(AtomicUsize::new(0));

    { let mut g = aggregator.lock().await; let _ = g.backfill_1m_klines(&TICKERS).await; }

    let bvol24h = Arc::new(Mutex::new(None::<f64>));
    {
        let bvol = Arc::clone(&bvol24h);
        tokio::spawn(async move {
            let client = Client::new();
            loop {
                if let Ok(val) = fetch_bvol24h(&client).await { *bvol.lock().await = Some(val); }
                tokio::time::sleep(Duration::from_secs(300)).await;
            }
        });
    }

    // Trad markets (ES/NQ) correlation state
    let trad_state = Arc::new(Mutex::new(TradMarketState::new()));
    let (ibkr_status_tx, ibkr_status_rx) = tokio::sync::watch::channel(IbkrConnectionStatus::Disconnected);
    {
        let state = Arc::clone(&trad_state);
        spawn_ibkr_feed(state, ibkr_status_tx);
    }

    for &ticker in &TICKERS {
        let agg = Arc::clone(&aggregator);
        tokio::spawn(async move { run_binance_kline_stream(ticker, agg).await; });
    }

    let config = WebSocketConfig::new(get_ws_url())
        .with_ping_interval(Duration::from_secs(30))
        .with_reconnect_delay(Duration::from_secs(2))
        .with_channel_buffer_size(100_000);
    let client = WebSocketClient::with_config(config);
    let (mut event_rx, mut status_rx) = client.start();

    {
        let agg = Arc::clone(&aggregator);
        let trad = Arc::clone(&trad_state);
        tokio::spawn(async move {
            let mut last_latency_log = Instant::now();
            let mut latency_samples: Vec<i64> = Vec::new();

            while let Some(event) = event_rx.recv().await {
                // Measure latency: exchange timestamp vs now
                let now_ms = chrono::Utc::now().timestamp_millis();
                let exchange_ms = event.time_exchange.timestamp_millis();
                let latency_ms = now_ms - exchange_ms;

                // Sample latency for BTC trades from Binance
                if event.kind == "trade"
                    && event.instrument.base.to_lowercase() == "btc"
                    && event.exchange.to_lowercase().contains("binance")
                {
                    latency_samples.push(latency_ms);

                    // Log every 5 seconds
                    if last_latency_log.elapsed() >= Duration::from_secs(5) && !latency_samples.is_empty() {
                        let avg = latency_samples.iter().sum::<i64>() / latency_samples.len() as i64;
                        let max = *latency_samples.iter().max().unwrap_or(&0);
                        let min = *latency_samples.iter().min().unwrap_or(&0);
                        eprintln!("[LATENCY] BTC Binance trades: avg={}ms min={}ms max={}ms samples={}",
                            avg, min, max, latency_samples.len());
                        latency_samples.clear();
                        last_latency_log = Instant::now();
                    }
                }

                // Feed BTC trades to trad_state for ES/BTC correlation
                if event.kind == "trade" && event.instrument.base.to_lowercase() == "btc" {
                    if let Ok(trade) = serde_json::from_value::<barter_trading_tuis::TradeData>(event.data.clone()) {
                        let ts = event.time_exchange.timestamp_millis();
                        trad.lock().await.update_btc_trade(trade.price, trade.amount, ts);
                    }
                }
                agg.lock().await.process_event(event);
            }
        });
    }

    {
        let conn = Arc::clone(&connected);
        tokio::spawn(async move {
            while let Some(status) = status_rx.recv().await {
                conn.store(matches!(status, ConnectionStatus::Connected), Ordering::Relaxed);
            }
        });
    }

    let mut last_draw = Instant::now();
    let draw_interval = Duration::from_millis(50);
    let mut signal_state = SignalState::default();
    let mut bar_state = BarState::default();

    let result = loop {
        if event::poll(Duration::from_millis(5))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                    KeyCode::Char('b') | KeyCode::Char('B') => focus_index.store(0, Ordering::Relaxed),
                    KeyCode::Char('e') | KeyCode::Char('E') => focus_index.store(1, Ordering::Relaxed),
                    KeyCode::Char('s') | KeyCode::Char('S') => focus_index.store(2, Ordering::Relaxed),
                    KeyCode::Tab => {
                        let c = focus_index.load(Ordering::Relaxed);
                        focus_index.store((c + 1) % 3, Ordering::Relaxed);
                    }
                    _ => {}
                }
            }
        }

        if last_draw.elapsed() >= draw_interval {
            let snapshot = aggregator.lock().await.snapshot();
            let connected_now = connected.load(Ordering::Relaxed);
            let idx = focus_index.load(Ordering::Relaxed);
            let ticker = TICKERS[idx];

            let debounced = snapshot.tickers.get(ticker).map(|t| {
                (signal_state.update_divergence(ticker, t.cvd_divergence_15s),
                 signal_state.update_flow(ticker, t.flow_signal))
            });

            let bvol = *bvol24h.lock().await;

            // Get trad markets signals
            let trad_signals = trad_state.lock().await.get_signals();
            let ibkr_status = *ibkr_status_rx.borrow();

            terminal.draw(|f| {
                render_ui(f, f.area(), &snapshot, connected_now, ticker, debounced, &mut bar_state, bvol, &trad_signals, ibkr_status);
            })?;
            last_draw = Instant::now();
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;
    result
}

// ============================================================================
// RENDER - New optimized layout
// ============================================================================
fn render_ui(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    connected: bool,
    ticker: &str,
    debounced: Option<(Option<DivergenceSignal>, FlowSignal)>,
    bar_state: &mut BarState,
    bvol24h: Option<f64>,
    trad_signals: &barter_trading_tuis::CorrelationSignals,
    ibkr_status: IbkrConnectionStatus,
) {
    // Layout: Header | Signal | Flow+Book | Exchanges+Vol | Whales+TradMarkets | Footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // Header with full context
            Constraint::Length(4),   // BIG SIGNAL
            Constraint::Length(8),   // Flow + Book (side by side)
            Constraint::Length(5),   // Exchanges + Volatility (compressed)
            Constraint::Min(16),     // Whales + Trad Markets (side by side, needs height)
            Constraint::Length(1),   // Footer
        ])
        .split(area);

    render_header(f, chunks[0], snapshot, connected, ticker, bvol24h);
    render_signal(f, chunks[1], snapshot, ticker, bar_state, debounced);

    // Flow + Book side by side
    let flow_book = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[2]);
    render_flow(f, flow_book[0], snapshot, ticker);
    render_book(f, flow_book[1], snapshot, ticker, bar_state);

    // Exchanges + Volatility
    let exch_vol = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(chunks[3]);
    render_exchanges(f, exch_vol[0], snapshot, ticker);
    render_volatility(f, exch_vol[1], snapshot, ticker);

    // Whales (left 40%) + Trad Markets (right 60%)
    let whale_trad = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(chunks[4]);
    render_whales(f, whale_trad[0], snapshot, ticker);
    render_trad_markets_panel(f, whale_trad[1], trad_signals, ibkr_status);

    render_footer(f, chunks[5], ticker);
}

/// Header: Price | Status | Speed | Spread | Lead | OI | Basis
fn render_header(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    connected: bool,
    ticker: &str,
    bvol24h: Option<f64>,
) {
    let block = Block::default()
        .title(format!(" {} ", ticker))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(C_HEADER));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Some(t) = snapshot.tickers.get(ticker) {
        // Use Binance perp last price for consistent reference (falls back to latest_price)
        // Guard: show "--" if no valid price to prevent 0.000 display
        let price_opt = t.binance_perp_last.or(t.latest_price).filter(|&p| p > 0.0);
        let price_str = match price_opt {
            Some(p) if p >= 1000.0 => format!("${:.2}", p),
            Some(p) => format!("${:.4}", p),
            None => "--".to_string(),
        };

        let status = if connected { "[LIVE]" } else { "[DISC]" };
        let status_color = if connected { C_BUY } else { C_SELL };

        let spread = t.latest_spread_pct.unwrap_or(0.0);
        let spread_color = if spread > 0.03 { C_SELL } else if spread > 0.01 { C_NEUTRAL } else { C_DIM };

        let lead = t.exchange_dominance.iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(k, _)| {
                let u = k.to_uppercase();
                if u.starts_with("BNC") { "BNC" } else if u.starts_with("BBT") { "BBT" } else if u.starts_with("OKX") { "OKX" } else { "OTH" }
            }).unwrap_or("--");

        // Fair value deviation in bps (price vs VWM across venues)
        let fv_bps = t.fair_value_deviation_bps;
        let fv_span = fv_bps.map(|bps| {
            let color = if bps > 5.0 {
                C_SELL
            } else if bps < -5.0 {
                C_BUY
            } else {
                C_NEUTRAL
            };
            Span::styled(format!("FV:{:+.0}bps", bps), Style::default().fg(color))
        });

        // OI: show raw delta + freshness (e.g., "OI:↑142K 3s")
        let oi_delta = t.oi_delta_5m;
        // Use 100 contracts as threshold for BTC/ETH (more meaningful than 10)
        let oi_arrow = if oi_delta > 100.0 { "↑" } else if oi_delta < -100.0 { "↓" } else { "→" };
        let oi_color = if oi_delta > 100.0 { C_BUY } else if oi_delta < -100.0 { C_SELL } else { C_DIM };
        // Format delta with K/M suffix
        let oi_delta_str = if oi_delta.abs() >= 1_000_000.0 {
            format!("{:+.1}M", oi_delta / 1_000_000.0)
        } else if oi_delta.abs() >= 1_000.0 {
            format!("{:+.0}K", oi_delta / 1_000.0)
        } else {
            format!("{:+.0}", oi_delta)
        };
        // Freshness color: green < 5s, yellow 5-15s, red > 15s
        let oi_age = t.oi_freshness_secs;
        let oi_age_color = if oi_age < 5.0 { C_BUY } else if oi_age < 15.0 { C_NEUTRAL } else { C_SELL };
        let oi_age_str = if oi_age > 99.0 { "??s".to_string() } else { format!("{:.0}s", oi_age) };

        let basis = t.basis.as_ref().map(|b| b.basis_pct).unwrap_or(0.0);
        let basis_color = if basis > 0.02 { C_BUY } else if basis < -0.02 { C_SELL } else { C_DIM };

        let mut spans: Vec<Span> = Vec::new();
        spans.push(Span::styled(
            price_str,
            Style::default().fg(C_BRIGHT).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw("  "));
        if let Some(fv) = fv_span {
            spans.push(fv);
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(status, Style::default().fg(status_color)));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("{:.0}t/s", t.trade_speed),
            Style::default().fg(C_ACCENT),
        ));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("Sprd:{:.2}%", spread),
            Style::default().fg(spread_color),
        ));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("LEAD:{}", lead),
            Style::default().fg(Color::Yellow),
        ));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("OI:{}{} ", oi_arrow, oi_delta_str),
            Style::default().fg(oi_color),
        ));
        spans.push(Span::styled(
            oi_age_str,
            Style::default().fg(oi_age_color),
        ));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("Basis:{:+.2}%", basis),
            Style::default().fg(basis_color),
        ));

        if let Some(bvol) = bvol24h {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(format!("BVOL:{:.1}", bvol), Style::default().fg(C_DIM)));
        }

        f.render_widget(Paragraph::new(Line::from(spans)), inner);
    }
}

/// BIG SIGNAL - The main decision
fn render_signal(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    ticker: &str,
    bar_state: &mut BarState,
    debounced: Option<(Option<DivergenceSignal>, FlowSignal)>,
) {
    if let Some(t) = snapshot.tickers.get(ticker) {
        // Composite pressure
        let flow_imb = t.orderflow_1m.imbalance_pct;
        let cvd_dir = if t.cvd_1m_total > 0.0 { 1.0 } else { -1.0 };
        let book_imb = if t.aggregated_book_imbalance > 0.0 { t.aggregated_book_imbalance } else { 50.0 };
        let pressure_raw = (flow_imb * 0.4 + book_imb * 0.3 + (50.0 + cvd_dir * 20.0) * 0.3).clamp(0.0, 100.0);

        let _ = bar_state.should_update_pressure(pressure_raw);
        let pressure = bar_state.last_pressure;

        // Signal text
        let (signal_text, bg_color, fg_color) = if pressure > 65.0 {
            ("BUY", Color::Rgb(0, 50, 0), C_BUY)
        } else if pressure > 55.0 {
            ("LEAN BUY", Color::Rgb(0, 35, 0), C_BUY)
        } else if pressure < 35.0 {
            ("SELL", Color::Rgb(50, 0, 0), C_SELL)
        } else if pressure < 45.0 {
            ("LEAN SELL", Color::Rgb(35, 0, 0), C_SELL)
        } else {
            ("NEUTRAL", Color::Rgb(30, 30, 0), C_NEUTRAL)
        };

        // Divergence badge
        let div_badge = debounced.and_then(|(div, _)| match div {
            Some(DivergenceSignal::Bullish) => Some((" ⚡BULL", C_BUY)),
            Some(DivergenceSignal::Bearish) => Some((" ⚡BEAR", C_SELL)),
            _ => None,
        });

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(fg_color))
            .style(Style::default().bg(bg_color));
        let inner = block.inner(area);
        f.render_widget(block, area);

        // Big centered signal
        let signal_display = format!("{} ({:.0}%)", signal_text, pressure);
        let bar_width = (inner.width as usize).saturating_sub(16).min(50);
        let (bar, bar_color) = bidir_bar(pressure, bar_width);

        // Line 1: Signal text (with optional divergence badge)
        let mut line1_spans = vec![
            Span::styled(signal_display, Style::default().fg(fg_color).add_modifier(Modifier::BOLD)),
        ];
        if let Some((badge, badge_color)) = div_badge {
            line1_spans.push(Span::styled(badge, Style::default().fg(badge_color).add_modifier(Modifier::BOLD)));
        }

        // Line 2: Bar with SELL/BUY labels
        let line2_spans = vec![
            Span::styled("SELL ", Style::default().fg(C_SELL)),
            Span::styled(bar, Style::default().fg(bar_color)),
            Span::styled(" BUY", Style::default().fg(C_BUY)),
        ];

        let lines = vec![
            Line::from(line1_spans),
            Line::from(line2_spans),
        ];

        f.render_widget(
            Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center),
            inner
        );
    }
}

/// FLOW section - CVD with bi-directional bars
fn render_flow(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    ticker: &str,
) {
    let block = Block::default()
        .title(" NET FLOW ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(C_ACCENT));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(ticker) {
        let bar_width = 16;

        // Convert CVD to imbalance % (map to 0-100 range based on typical values)
        let cvd_to_imb = |cvd: f64| -> f64 {
            // Rough scaling: ±500K maps to 0-100%
            (50.0 + (cvd / 500_000.0) * 50.0).clamp(0.0, 100.0)
        };

        // Format delta value - show actual small values instead of -0
        let format_delta = |v: f64| -> String {
            let abs = v.abs();
            if abs >= 1_000_000_000.0 {
                format!("{:+.1}B", v / 1_000_000_000.0)
            } else if abs >= 1_000_000.0 {
                format!("{:+.1}M", v / 1_000_000.0)
            } else if abs >= 1_000.0 {
                format!("{:+.0}K", v / 1_000.0)
            } else if abs >= 1.0 {
                format!("{:+.0}", v)  // Show actual small values
            } else {
                "~0".to_string()  // Only show ~0 for truly near-zero values
            }
        };

        let timeframes = [
            ("5s ", t.cvd_5s),
            ("15s", t.cvd_15s),
            ("30s", t.cvd_30s),
            ("1m ", t.cvd_1m_total),
        ];

        for (label, cvd) in timeframes {
            let formatted = format_delta(cvd);
            let imb = cvd_to_imb(cvd);
            let (bar, color) = bidir_bar(imb, bar_width);
            let cvd_color = if cvd > 0.0 { C_BUY } else if cvd < 0.0 { C_SELL } else { C_DIM };

            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", label), Style::default().fg(C_DIM)),
                Span::styled(format!("{:>8} ", formatted), Style::default().fg(cvd_color)),
                Span::styled(bar, Style::default().fg(color)),
            ]));
        }

        // Alignment summary - more specific than "MIXED SIGNALS"
        let pos = timeframes.iter().filter(|(_, v)| *v > 0.0).count();
        let neg = timeframes.iter().filter(|(_, v)| *v < 0.0).count();
        let (align_text, align_color) = if pos >= 3 {
            (format!("↑ BUY PRESSURE ({}/4)", pos), C_BUY)
        } else if neg >= 3 {
            (format!("↓ SELL PRESSURE ({}/4)", neg), C_SELL)
        } else if pos == 2 && neg == 2 {
            ("⟷ DIVERGENT".to_string(), C_NEUTRAL)
        } else {
            ("→ TRANSITIONING".to_string(), C_NEUTRAL)
        };

        lines.push(Line::from(vec![])); // spacer
        lines.push(Line::from(Span::styled(align_text, Style::default().fg(align_color).add_modifier(Modifier::BOLD))));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

/// BOOK section - L2 with bi-directional bars
fn render_book(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    ticker: &str,
    bar_state: &mut BarState,
) {
    let block = Block::default()
        .title(" BOOK (L2) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(ticker) {
        let raw_bnc = t.per_exchange_book_imbalance.get("BNC").copied().unwrap_or(0.0);
        let raw_bbt = t.per_exchange_book_imbalance.get("BBT").copied().unwrap_or(0.0);
        let raw_okx = t.per_exchange_book_imbalance.get("OKX").copied().unwrap_or(0.0);

        let (bnc, bbt, okx) = bar_state.get_l2_throttled(raw_bnc, raw_bbt, raw_okx);

        let bar_width = 16;
        let venues = [("BNC", bnc), ("BBT", bbt), ("OKX", okx)];

        for (label, imb) in venues {
            if imb > 1.0 {
                let (bar, color) = bidir_bar(imb, bar_width);
                let dir = if imb > 55.0 { "BID" } else if imb < 45.0 { "ASK" } else { "BAL" };

                lines.push(Line::from(vec![
                    Span::styled(format!("{}: ", label), Style::default().fg(C_DIM)),
                    Span::styled(bar, Style::default().fg(color)),
                    Span::styled(format!(" {:>2.0}% {}", imb, dir), Style::default().fg(color)),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(format!("{}: ", label), Style::default().fg(C_DIM)),
                    Span::styled("-- no L2 --", Style::default().fg(C_DIM)),
                ]));
            }
        }

        // Aggregate
        let agg = t.aggregated_book_imbalance;
        if agg > 1.0 {
            let (bar, color) = bidir_bar(agg, bar_width);
            let dir = if agg > 55.0 { "BID" } else if agg < 45.0 { "ASK" } else { "BAL" };
            lines.push(Line::from(vec![])); // spacer
            lines.push(Line::from(vec![
                Span::styled("AGG: ", Style::default().fg(C_BRIGHT)),
                Span::styled(bar, Style::default().fg(color)),
                Span::styled(format!(" {:>2.0}% {}", agg, dir), Style::default().fg(color).add_modifier(Modifier::BOLD)),
            ]));
        }
    }

    f.render_widget(Paragraph::new(lines), inner);
}

/// Exchanges table
fn render_exchanges(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    ticker: &str,
) {
    let block = Block::default()
        .title(" EXCHANGES (30s) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(C_ACCENT));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(ticker) {
        // Header
        lines.push(Line::from(vec![
            Span::styled("         ", Style::default()),
            Span::styled(format!("{:^10}", "OKX"), Style::default().fg(Color::Yellow)),
            Span::styled(format!("{:^10}", "BNC"), Style::default().fg(C_ACCENT)),
            Span::styled(format!("{:^10}", "BBT"), Style::default().fg(Color::Magenta)),
        ]));

        let norm = |n: &str| -> &'static str {
            if n.to_lowercase().contains("binance") { "binance" }
            else if n.to_lowercase().contains("bybit") { "bybit" }
            else if n.to_lowercase().contains("okx") { "okx" }
            else { "other" }
        };

        // Format CVD with proper small value handling
        let format_cvd = |v: f64| -> String {
            let abs = v.abs();
            if abs >= 1_000_000.0 {
                format!("{:+.1}M", v / 1_000_000.0)
            } else if abs >= 1_000.0 {
                format!("{:+.0}K", v / 1_000.0)
            } else if abs >= 1.0 {
                format!("{:+.0}", v)
            } else {
                "~0".to_string()
            }
        };

        let get_stats = |ex: &str| -> (String, f64, String, f64) {
            t.per_exchange_30s.iter()
                .find(|(k, _)| norm(k) == norm(ex))
                .map(|(_, v)| {
                    let cvd_str = format_cvd(v.cvd_30s);
                    let buy = (v.total_30s + v.cvd_30s) / 2.0;
                    let imb = if v.total_30s > 0.0 { (buy / v.total_30s * 100.0).round() } else { 50.0 };
                    let label = if imb >= 55.0 { "BUY" } else if imb <= 45.0 { "SELL" } else { "BAL" };
                    (cvd_str, v.cvd_30s, format!("{:.0}% {}", imb, label), imb)
                })
                .unwrap_or(("--".to_string(), 0.0, "--".to_string(), 50.0))
        };

        let (cvd_okx, cvd_okx_raw, imb_okx, imb_okx_raw) = get_stats("Okx");
        let (cvd_bnc, cvd_bnc_raw, imb_bnc, imb_bnc_raw) = get_stats("BinanceFuturesUsd");
        let (cvd_bbt, cvd_bbt_raw, imb_bbt, imb_bbt_raw) = get_stats("BybitPerpetualsUsd");

        let cvd_color = |v: f64| if v > 0.0 { C_BUY } else if v < 0.0 { C_SELL } else { C_DIM };
        let imb_color = |v: f64| if v >= 55.0 { C_BUY } else if v <= 45.0 { C_SELL } else { C_NEUTRAL };

        lines.push(Line::from(vec![
            Span::styled("CVD:     ", Style::default().fg(C_DIM)),
            Span::styled(format!("{:^10}", cvd_okx), Style::default().fg(cvd_color(cvd_okx_raw))),
            Span::styled(format!("{:^10}", cvd_bnc), Style::default().fg(cvd_color(cvd_bnc_raw))),
            Span::styled(format!("{:^10}", cvd_bbt), Style::default().fg(cvd_color(cvd_bbt_raw))),
        ]));

        lines.push(Line::from(vec![
            Span::styled("FLOW:    ", Style::default().fg(C_DIM)),
            Span::styled(format!("{:^10}", imb_okx), Style::default().fg(imb_color(imb_okx_raw))),
            Span::styled(format!("{:^10}", imb_bnc), Style::default().fg(imb_color(imb_bnc_raw))),
            Span::styled(format!("{:^10}", imb_bbt), Style::default().fg(imb_color(imb_bbt_raw))),
        ]));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

/// Volatility section
fn render_volatility(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    ticker: &str,
) {
    let block = Block::default()
        .title(" VOL ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(C_ACCENT));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(ticker) {
        let atr = t.atr_14.unwrap_or(0.0);
        lines.push(Line::from(vec![
            Span::styled("ATR: ", Style::default().fg(C_DIM)),
            Span::styled(format!("${:.0}", atr), Style::default().fg(C_BRIGHT)),
        ]));

        let vwap_str = t.tv_vwap_deviation
            .map(|d| (format!("{:+.2}%", d), if d > 0.0 { C_BUY } else { C_SELL }))
            .unwrap_or(("--".to_string(), C_DIM));
        lines.push(Line::from(vec![
            Span::styled("tvVWAP: ", Style::default().fg(C_DIM)),
            Span::styled(vwap_str.0, Style::default().fg(vwap_str.1)),
        ]));

        // Realized volatility 30m/1h (match scalper v1 format)
        let rv30 = t.realized_vol_30m.unwrap_or(0.0);
        let rv1h = t.realized_vol_1h.unwrap_or(0.0);
        let rv_trend = match t.realized_vol_trend {
            VolTrend::Expanding => "+EXP",
            VolTrend::Contracting => "-CTR",
            VolTrend::Stable => "+STB",
        };
        let rv_color = if rv1h > rv30 { C_SELL } else if rv1h < rv30 { C_BUY } else { C_DIM };
        lines.push(Line::from(vec![
            Span::styled("RV30/1h: ", Style::default().fg(C_DIM)),
            Span::styled(format!("{:.2}%/{:.2}% {}", rv30, rv1h, rv_trend), Style::default().fg(rv_color)),
        ]));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

/// Whale tape
fn render_whales(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    ticker: &str,
) {
    let threshold_k = whale_threshold() / 1000.0;
    let block = Block::default()
        .title(format!(" WHALES (>${:.0}K, 90s) ", threshold_k))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(C_ACCENT));
    let rows = area.height.saturating_sub(2) as usize;
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(ticker) {
        let now = chrono::Utc::now();
        let cutoff = now - chrono::Duration::seconds(90);

        let recent: Vec<_> = t.whales.iter().filter(|w| w.time >= cutoff).take(rows).collect();

        if recent.is_empty() {
            lines.push(Line::from(Span::styled("No whale trades in last 90s", Style::default().fg(C_DIM))));
        } else {
            for w in recent {
                let age = (now - w.time).num_milliseconds() as f64 / 1000.0;
                let side_color = if w.side == Side::Buy { C_BUY } else { C_SELL };
                let ex = match w.exchange.as_str() {
                    "BinanceFuturesUsd" | "BinanceSpot" => "BNC",
                    "BybitPerpetualsUsd" | "BybitSpot" => "BBT",
                    "Okx" => "OKX",
                    _ => "OTH",
                };
                let vol = if w.volume_usd >= 1_000_000.0 { format!("${:.1}M", w.volume_usd / 1_000_000.0) }
                    else { format!("${:.0}K", w.volume_usd / 1_000.0) };
                let price = if w.price >= 1000.0 { format!("@{:.0}", w.price) } else { format!("@{:.2}", w.price) };

                lines.push(Line::from(vec![
                    Span::raw("→ "),
                    Span::styled(format!("{:>7} ", vol), Style::default().fg(side_color).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{:4} ", w.side.as_str().to_uppercase()), Style::default().fg(side_color)),
                    Span::styled(format!("{} ", price), Style::default().fg(C_BRIGHT)),
                    Span::styled(format!("[{}] ", ex), Style::default().fg(C_ACCENT)),
                    Span::styled(format!("{:.0}s", age), Style::default().fg(C_DIM)),
                ]));
            }
        }
    }

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

/// Footer
fn render_footer(f: &mut ratatui::Frame, area: Rect, ticker: &str) {
    let hl = |t: &str, active: bool| -> Style {
        if active { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) }
        else { Style::default().fg(C_DIM) }
    };

    let line = Line::from(vec![
        Span::raw(" ["),
        Span::styled("B", hl("BTC", ticker == "BTC")),
        Span::raw("]TC  ["),
        Span::styled("E", hl("ETH", ticker == "ETH")),
        Span::raw("]TH  ["),
        Span::styled("S", hl("SOL", ticker == "SOL")),
        Span::raw("]OL  │  "),
        Span::styled("SCALPER V2", Style::default().fg(C_HEADER)),
        Span::raw("  │  [q] Quit"),
    ]);

    f.render_widget(Paragraph::new(line), area);
}
