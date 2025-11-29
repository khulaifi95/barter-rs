/// Scalper V2 - World-Class Trading TUI
///
/// Optimized for 1-2 second decision making with:
/// - Clear SIGNAL hierarchy (big signal first, details second)
/// - Bi-directional pressure bars (see both sides at a glance)
/// - Balanced colors (less noise, more signal)
/// - OKX L2 integration
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
// CONSTANTS & CONFIG
// ============================================================================

const TICKERS: [&str; 3] = ["BTC", "ETH", "SOL"];
const SIGNAL_DEBOUNCE_MS: u64 = 1000;

// Throttling constants for visual stability
const L2_INTERVAL_MS: u128 = 1500;
const BANNER_INTERVAL_MS: u128 = 1200;

// Color palette - balanced and professional
const COLOR_BUY: Color = Color::Rgb(80, 200, 120);      // Soft green
const COLOR_SELL: Color = Color::Rgb(220, 80, 80);      // Soft red
const COLOR_NEUTRAL: Color = Color::Rgb(180, 180, 80);  // Soft yellow
const COLOR_DIM: Color = Color::Rgb(100, 100, 100);     // Dimmed text
const COLOR_ACCENT: Color = Color::Rgb(100, 180, 220);  // Accent cyan
const COLOR_HEADER: Color = Color::Rgb(180, 120, 220);  // Header purple

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
    last_flow_imb: f64,
    last_update: Option<Instant>,
    l2_bnc: VenueThrottle,
    l2_bbt: VenueThrottle,
    l2_okx: VenueThrottle,
    l2_agg: VenueThrottle,
}

impl BarState {
    fn should_update(&mut self, pressure: f64, flow_imb: f64) -> bool {
        let now = Instant::now();
        let time_ok = self.last_update
            .map(|t| now.duration_since(t).as_millis() >= BANNER_INTERVAL_MS)
            .unwrap_or(true);
        if time_ok {
            self.last_pressure = pressure;
            self.last_flow_imb = flow_imb;
            self.last_update = Some(now);
            true
        } else {
            false
        }
    }

    fn get_l2_throttled(&mut self, bnc: f64, bbt: f64, okx: f64, agg: f64) -> (f64, f64, f64, f64) {
        (
            self.l2_bnc.get_throttled(bnc, L2_INTERVAL_MS),
            self.l2_bbt.get_throttled(bbt, L2_INTERVAL_MS),
            self.l2_okx.get_throttled(okx, L2_INTERVAL_MS),
            self.l2_agg.get_throttled(agg, L2_INTERVAL_MS),
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
            Some((prev_signal, start_time)) if *prev_signal == signal => {
                if now.duration_since(*start_time).as_millis() >= SIGNAL_DEBOUNCE_MS as u128 {
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
        let dominated = !matches!(signal, FlowSignal::Neutral);
        if !dominated {
            self.flow_start.remove(ticker);
            return signal;
        }
        let now = Instant::now();
        match self.flow_start.get(ticker) {
            Some((prev_signal, start_time)) if *prev_signal == signal => {
                if now.duration_since(*start_time).as_millis() >= SIGNAL_DEBOUNCE_MS as u128 {
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
// HELPER FUNCTIONS
// ============================================================================

fn scale_number(v: f64) -> (f64, &'static str) {
    let abs = v.abs();
    if abs >= 1_000_000_000.0 {
        (v / 1_000_000_000.0, "B")
    } else if abs >= 1_000_000.0 {
        (v / 1_000_000.0, "M")
    } else if abs >= 1_000.0 {
        (v / 1_000.0, "K")
    } else {
        (v, "")
    }
}

fn get_ws_url() -> String {
    std::env::var("WS_URL").unwrap_or_else(|_| "ws://127.0.0.1:9001".to_string())
}

fn whale_threshold() -> f64 {
    std::env::var("WHALE_THRESHOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50_000.0) // Lower threshold for scalper
}

async fn fetch_bvol24h(client: &Client) -> Result<f64, reqwest::Error> {
    let url = "https://www.bitmex.com/api/v1/instrument?symbol=.BVOL24H";
    let resp = client.get(url).send().await?.error_for_status()?;
    let v: Value = resp.json().await?;
    Ok(v.get(0)
        .and_then(|o| o.get("lastPrice"))
        .and_then(|p| p.as_f64())
        .unwrap_or(0.0))
}

/// Render a bi-directional pressure bar
/// ░░░░░░░░░░░│██████████████████  (center = 50%, left = sell, right = buy)
fn render_bidirectional_bar(value: f64, width: usize) -> (String, Color) {
    let half = width / 2;
    let deviation = ((value - 50.0) / 50.0 * half as f64).round() as i32;

    let (left_fill, right_fill) = if deviation >= 0 {
        (0usize, deviation.min(half as i32) as usize)
    } else {
        ((-deviation).min(half as i32) as usize, 0usize)
    };

    let left_empty = half.saturating_sub(left_fill);
    let right_empty = half.saturating_sub(right_fill);

    let bar = format!(
        "{}{}│{}{}",
        "░".repeat(left_empty),
        "█".repeat(left_fill),
        "█".repeat(right_fill),
        "░".repeat(right_empty)
    );

    let color = if value > 55.0 { COLOR_BUY }
        else if value < 45.0 { COLOR_SELL }
        else { COLOR_NEUTRAL };

    (bar, color)
}

/// Spawn Binance 1m kline stream for tvVWAP/ATR/RV
async fn run_binance_kline_stream(ticker: &str, agg: Arc<Mutex<Aggregator>>) {
    let symbol = ticker_to_binance_symbol(ticker).to_lowercase();
    let url = format!("wss://fstream.binance.com/ws/{}@kline_1m", symbol);

    loop {
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
                            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                                if let Some(k) = v.get("k") {
                                    let is_final = k.get("x").and_then(|b| b.as_bool()).unwrap_or(false);
                                    if !is_final { continue; }
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
                                                    open: o, high: h, low: l, close: c, volume,
                                                    start_time, is_complete: true,
                                                };
                                                let mut guard = agg.lock().await;
                                                guard.push_1m_candle(ticker, candle);
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
            Err(e) => eprintln!("[kline-ws] {} connect error: {}", ticker, e),
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

// ============================================================================
// MAIN
// ============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    if let Err(e) = default_provider().install_default() {
        eprintln!("[crypto] provider install: {:?}", e);
    }

    // Panic hook for terminal cleanup
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic_info);
    }));

    // Terminal setup
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Shared state
    let aggregator = Arc::new(Mutex::new(Aggregator::new()));
    let connected = Arc::new(AtomicBool::new(false));
    let focus_index = Arc::new(AtomicUsize::new(0));

    // Backfill klines
    {
        let mut guard = aggregator.lock().await;
        let _ = guard.backfill_1m_klines(&TICKERS).await;
    }

    // BVOL24H background fetch
    let bvol24h = Arc::new(Mutex::new(None::<f64>));
    {
        let bvol = Arc::clone(&bvol24h);
        tokio::spawn(async move {
            let client = Client::new();
            loop {
                if let Ok(val) = fetch_bvol24h(&client).await {
                    *bvol.lock().await = Some(val);
                }
                tokio::time::sleep(Duration::from_secs(300)).await;
            }
        });
    }

    // Kline streams
    for &ticker in &TICKERS {
        let agg = Arc::clone(&aggregator);
        tokio::spawn(async move {
            run_binance_kline_stream(ticker, agg).await;
        });
    }

    // WebSocket client
    let ws_url = get_ws_url();
    let config = WebSocketConfig::new(ws_url)
        .with_ping_interval(Duration::from_secs(30))
        .with_reconnect_delay(Duration::from_secs(2))
        .with_channel_buffer_size(100_000);
    let client = WebSocketClient::with_config(config);
    let (mut event_rx, mut status_rx) = client.start();

    // Event processor
    {
        let agg = Arc::clone(&aggregator);
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                agg.lock().await.process_event(event);
            }
        });
    }

    // Connection status
    {
        let conn = Arc::clone(&connected);
        tokio::spawn(async move {
            while let Some(status) = status_rx.recv().await {
                match status {
                    ConnectionStatus::Connected => conn.store(true, Ordering::Relaxed),
                    _ => conn.store(false, Ordering::Relaxed),
                }
            }
        });
    }

    // UI loop - 50ms refresh (20Hz)
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
            let focus_idx = focus_index.load(Ordering::Relaxed);
            let focused_ticker = TICKERS[focus_idx];

            let debounced = snapshot.tickers.get(focused_ticker).map(|t| {
                let div = signal_state.update_divergence(focused_ticker, t.cvd_divergence_15s);
                let flow = signal_state.update_flow(focused_ticker, t.flow_signal);
                (div, flow)
            });

            let bvol_val = *bvol24h.lock().await;

            terminal.draw(|f| {
                render_ui(f, f.area(), &snapshot, connected_now, focused_ticker, debounced, &mut bar_state, bvol_val);
            })?;
            last_draw = Instant::now();
        }

        tokio::time::sleep(Duration::from_millis(5)).await;
    };

    // Cleanup
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;
    result
}

// ============================================================================
// RENDER FUNCTIONS
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
) {
    // LAYOUT: Focus on signal hierarchy
    // 1. Header (compact info line)
    // 2. BIG SIGNAL BANNER (the decision)
    // 3. FLOW ROW: Delta | Orderflow | L2 Book (bi-directional bars)
    // 4. CONTEXT ROW: Volatility | Exchanges
    // 5. WHALE TAPE
    // 6. Footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Header
            Constraint::Length(5),  // BIG SIGNAL BANNER
            Constraint::Length(7),  // Flow row (3 columns)
            Constraint::Length(5),  // Context row
            Constraint::Min(4),     // Whale tape
            Constraint::Length(1),  // Footer
        ])
        .split(area);

    render_header(f, chunks[0], snapshot, connected, ticker);
    render_signal_banner(f, chunks[1], snapshot, ticker, bar_state, debounced);

    // Flow row: 3 columns
    let flow_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(chunks[2]);

    render_delta_velocity(f, flow_row[0], snapshot, ticker);
    render_orderflow(f, flow_row[1], snapshot, ticker);
    render_l2_book(f, flow_row[2], snapshot, ticker, bar_state);

    // Context row: 2 columns
    let ctx_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(chunks[3]);

    render_volatility(f, ctx_row[0], snapshot, ticker, bvol24h);
    render_exchanges(f, ctx_row[1], snapshot, ticker);

    render_whale_tape(f, chunks[4], snapshot, ticker);
    render_footer(f, chunks[5], ticker);
}

/// Compact header: Price | Status | Speed | Lead | OI | Basis
fn render_header(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    connected: bool,
    ticker: &str,
) {
    let block = Block::default()
        .title(format!(" SCALPER V2 - {} ", ticker))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_HEADER));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Some(t) = snapshot.tickers.get(ticker) {
        let price = t.latest_price.unwrap_or(0.0);
        let price_str = if price >= 1000.0 { format!("${:.2}", price) } else { format!("${:.4}", price) };

        let status = if connected { "LIVE" } else { "DISC" };
        let status_color = if connected { COLOR_BUY } else { COLOR_SELL };

        let lead = t.exchange_dominance.iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(k, _)| {
                let u = k.to_uppercase();
                if u.starts_with("BNC") { "BNC" }
                else if u.starts_with("BBT") { "BBT" }
                else if u.starts_with("OKX") { "OKX" }
                else { "OTH" }
            }).unwrap_or("--");

        let oi_vel = t.oi_velocity;
        let oi_arrow = if oi_vel > 0.3 { "↑" } else if oi_vel < -0.3 { "↓" } else { "→" };
        let oi_color = if oi_vel > 0.3 { COLOR_BUY } else if oi_vel < -0.3 { COLOR_SELL } else { COLOR_DIM };

        let basis = t.basis.as_ref().map(|b| b.basis_pct).unwrap_or(0.0);

        let line = Line::from(vec![
            Span::styled(price_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(format!("[{}]", status), Style::default().fg(status_color)),
            Span::raw("  "),
            Span::styled(format!("{:.0}t/s", t.trade_speed), Style::default().fg(COLOR_ACCENT)),
            Span::raw("  "),
            Span::styled(format!("LEAD:{}", lead), Style::default().fg(Color::Yellow)),
            Span::raw("  "),
            Span::styled(format!("OI:{}", oi_arrow), Style::default().fg(oi_color)),
            Span::raw("  "),
            Span::styled(format!("Basis:{:+.2}%", basis), Style::default().fg(
                if basis > 0.02 { COLOR_BUY } else if basis < -0.02 { COLOR_SELL } else { COLOR_NEUTRAL }
            )),
        ]);

        f.render_widget(Paragraph::new(line), inner);
    }
}

/// BIG SIGNAL BANNER - The main decision at a glance
fn render_signal_banner(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    ticker: &str,
    bar_state: &mut BarState,
    debounced: Option<(Option<DivergenceSignal>, FlowSignal)>,
) {
    if let Some(t) = snapshot.tickers.get(ticker) {
        // Calculate composite pressure
        let flow_imb = t.orderflow_1m.imbalance_pct;
        let cvd_dir = if t.cvd_1m_total > 0.0 { 1.0 } else { -1.0 };
        let book_imb = if t.aggregated_book_imbalance > 0.0 { t.aggregated_book_imbalance } else { 50.0 };
        let pressure_raw = (flow_imb * 0.4 + book_imb * 0.3 + (50.0 + cvd_dir * 20.0) * 0.3).clamp(0.0, 100.0);

        let _update = bar_state.should_update(pressure_raw, flow_imb);
        let pressure = bar_state.last_pressure;

        // Determine signal strength
        let (signal_text, bg_color, fg_color, confidence) = if pressure > 65.0 {
            ("BUY", Color::Rgb(0, 60, 0), COLOR_BUY, pressure)
        } else if pressure > 55.0 {
            ("LEAN BUY", Color::Rgb(0, 40, 0), COLOR_BUY, pressure)
        } else if pressure < 35.0 {
            ("SELL", Color::Rgb(60, 0, 0), COLOR_SELL, 100.0 - pressure)
        } else if pressure < 45.0 {
            ("LEAN SELL", Color::Rgb(40, 0, 0), COLOR_SELL, 100.0 - pressure)
        } else {
            ("NEUTRAL", Color::Rgb(30, 30, 0), COLOR_NEUTRAL, 50.0)
        };

        // Check for divergence signals
        let div_badge = debounced.and_then(|(div, _)| {
            match div {
                Some(DivergenceSignal::Bullish) => Some(("⚡BULL DIV", COLOR_BUY)),
                Some(DivergenceSignal::Bearish) => Some(("⚡BEAR DIV", COLOR_SELL)),
                _ => None,
            }
        });

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(fg_color))
            .style(Style::default().bg(bg_color));

        let inner = block.inner(area);
        f.render_widget(block, area);

        // Main signal line
        let signal_display = format!("{} ({:.0}%)", signal_text, confidence);

        // Bi-directional pressure bar
        let bar_width = (inner.width as usize).saturating_sub(4).min(40);
        let (bar, bar_color) = render_bidirectional_bar(pressure, bar_width);

        let mut lines = vec![
            Line::from(vec![
                Span::styled(
                    format!("{:^width$}", signal_display, width = inner.width as usize),
                    Style::default().fg(fg_color).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("SELL ", Style::default().fg(COLOR_SELL)),
                Span::styled(bar, Style::default().fg(bar_color)),
                Span::styled(" BUY", Style::default().fg(COLOR_BUY)),
            ]),
        ];

        // Add divergence badge if present
        if let Some((badge, badge_color)) = div_badge {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:^width$}", badge, width = inner.width as usize),
                    Style::default().fg(badge_color).add_modifier(Modifier::BOLD),
                ),
            ]));
        }

        f.render_widget(Paragraph::new(lines), inner);
    }
}

/// Delta Velocity with multi-timeframe confirmation
fn render_delta_velocity(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    ticker: &str,
) {
    let block = Block::default()
        .title(" DELTA ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_ACCENT));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(ticker) {
        let velocity = t.cvd_5s / 5.0;
        let (vel_scaled, vel_suffix) = scale_number(velocity);
        let vel_color = if velocity > 0.0 { COLOR_BUY } else { COLOR_SELL };

        let accel = if velocity.abs() > (t.cvd_30s / 30.0).abs() * 1.5 { "ACCEL" }
            else if velocity.abs() < (t.cvd_30s / 30.0).abs() * 0.5 { "DECEL" }
            else { "STEADY" };
        let accel_color = match accel {
            "ACCEL" => COLOR_BUY,
            "DECEL" => COLOR_SELL,
            _ => COLOR_DIM,
        };

        lines.push(Line::from(vec![
            Span::styled("VEL: ", Style::default().fg(COLOR_DIM)),
            Span::styled(format!("{:+.1}{}/s", vel_scaled, vel_suffix), Style::default().fg(vel_color).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(accel, Style::default().fg(accel_color)),
        ]));

        // Multi-timeframe with visual indicators
        let fmt_cvd = |val: f64| -> (String, Color) {
            let (s, suffix) = scale_number(val);
            let color = if val > 0.0 { COLOR_BUY } else if val < 0.0 { COLOR_SELL } else { COLOR_DIM };
            (format!("{:+.0}{}", s, suffix), color)
        };

        let (d5, c5) = fmt_cvd(t.cvd_5s);
        let (d15, c15) = fmt_cvd(t.cvd_15s);
        let (d30, c30) = fmt_cvd(t.cvd_30s);
        let (d1m, c1m) = fmt_cvd(t.cvd_1m_total);

        lines.push(Line::from(vec![
            Span::styled("5s:", Style::default().fg(COLOR_DIM)),
            Span::styled(format!("{:>7}", d5), Style::default().fg(c5)),
            Span::raw(" "),
            Span::styled("15s:", Style::default().fg(COLOR_DIM)),
            Span::styled(format!("{:>7}", d15), Style::default().fg(c15)),
        ]));

        lines.push(Line::from(vec![
            Span::styled("30s:", Style::default().fg(COLOR_DIM)),
            Span::styled(format!("{:>6}", d30), Style::default().fg(c30)),
            Span::raw(" "),
            Span::styled("1m:", Style::default().fg(COLOR_DIM)),
            Span::styled(format!("{:>7}", d1m), Style::default().fg(c1m)),
        ]));

        // Alignment indicator
        let pos_count = [t.cvd_5s, t.cvd_15s, t.cvd_30s, t.cvd_1m_total].iter().filter(|&&x| x > 0.0).count();
        let neg_count = 4 - pos_count;
        let (align_text, align_color) = if pos_count >= 3 {
            (format!("✓{}/4 BUY", pos_count), COLOR_BUY)
        } else if neg_count >= 3 {
            (format!("✓{}/4 SELL", neg_count), COLOR_SELL)
        } else {
            ("MIXED".to_string(), COLOR_NEUTRAL)
        };

        lines.push(Line::from(Span::styled(align_text, Style::default().fg(align_color))));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

/// Orderflow with BI-DIRECTIONAL BAR
fn render_orderflow(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    ticker: &str,
) {
    let block = Block::default()
        .title(" ORDERFLOW ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(ticker) {
        let imb = t.orderflow_1m.imbalance_pct;
        let label = if imb > 55.0 { "BUY" } else if imb < 45.0 { "SELL" } else { "BAL" };
        let color = if imb > 55.0 { COLOR_BUY } else if imb < 45.0 { COLOR_SELL } else { COLOR_NEUTRAL };

        // Bi-directional bar
        let bar_width = (inner.width as usize).saturating_sub(12).min(20);
        let (bar, _) = render_bidirectional_bar(imb, bar_width);

        lines.push(Line::from(vec![
            Span::styled(format!("{:.0}%", imb), Style::default().fg(color).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(label, Style::default().fg(color)),
        ]));

        lines.push(Line::from(vec![
            Span::styled("S ", Style::default().fg(COLOR_SELL)),
            Span::styled(bar, Style::default().fg(color)),
            Span::styled(" B", Style::default().fg(COLOR_BUY)),
        ]));

        // Trend
        let imb_30s = t.orderflow_30s.imbalance_pct;
        let trend = if imb_30s > imb + 3.0 { "↑str" }
            else if imb_30s < imb - 3.0 { "↓wkn" }
            else { "→std" };
        lines.push(Line::from(vec![
            Span::styled("1m: ", Style::default().fg(COLOR_DIM)),
            Span::styled(format!("{:.0}% {}", imb, trend), Style::default().fg(Color::White)),
        ]));

        // Empty for spacing
        lines.push(Line::from(""));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

/// L2 Book with BI-DIRECTIONAL BARS per venue
fn render_l2_book(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    ticker: &str,
    bar_state: &mut BarState,
) {
    let block = Block::default()
        .title(" L2 BOOK ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(ticker) {
        let raw_bnc = t.per_exchange_book_imbalance.get("BNC").copied().unwrap_or(0.0);
        let raw_bbt = t.per_exchange_book_imbalance.get("BBT").copied().unwrap_or(0.0);
        let raw_okx = t.per_exchange_book_imbalance.get("OKX").copied().unwrap_or(0.0);
        let raw_agg = t.aggregated_book_imbalance;

        let (bnc, bbt, okx, agg) = bar_state.get_l2_throttled(raw_bnc, raw_bbt, raw_okx, raw_agg);

        let bar_width = 12;
        let venues = [("BNC", bnc), ("BBT", bbt), ("OKX", okx)];

        for (label, imb) in venues {
            if imb > 1.0 {
                let dir = if imb > 55.0 { "BID" } else if imb < 45.0 { "ASK" } else { "BAL" };
                let color = if imb > 55.0 { COLOR_BUY } else if imb < 45.0 { COLOR_SELL } else { COLOR_NEUTRAL };
                let (bar, _) = render_bidirectional_bar(imb, bar_width);

                lines.push(Line::from(vec![
                    Span::styled(format!("{}: ", label), Style::default().fg(COLOR_DIM)),
                    Span::styled(format!("{:>3.0}%", imb), Style::default().fg(color)),
                    Span::raw(" "),
                    Span::styled(bar, Style::default().fg(color)),
                    Span::raw(" "),
                    Span::styled(dir, Style::default().fg(color)),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(format!("{}: ", label), Style::default().fg(COLOR_DIM)),
                    Span::styled("--", Style::default().fg(COLOR_DIM)),
                ]));
            }
        }

        // Aggregate
        if agg > 1.0 {
            let dir = if agg > 55.0 { "BID" } else if agg < 45.0 { "ASK" } else { "BAL" };
            let color = if agg > 55.0 { COLOR_BUY } else if agg < 45.0 { COLOR_SELL } else { COLOR_NEUTRAL };
            lines.push(Line::from(vec![
                Span::styled("AGG: ", Style::default().fg(Color::White)),
                Span::styled(format!("{:.0}% {}", agg, dir), Style::default().fg(color).add_modifier(Modifier::BOLD)),
            ]));
        }
    }

    f.render_widget(Paragraph::new(lines), inner);
}

/// Volatility context
fn render_volatility(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    ticker: &str,
    bvol24h: Option<f64>,
) {
    let block = Block::default()
        .title(" VOLATILITY ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_ACCENT));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(ticker) {
        let atr = t.atr_14.unwrap_or(0.0);

        let mut atr_spans = vec![
            Span::styled("ATR: ", Style::default().fg(COLOR_DIM)),
            Span::styled(format!("${:.0}", atr), Style::default().fg(Color::White)),
        ];
        if let Some(bvol) = bvol24h {
            atr_spans.push(Span::raw("  "));
            atr_spans.push(Span::styled(format!("BVOL:{:.1}", bvol), Style::default().fg(COLOR_DIM)));
        }
        lines.push(Line::from(atr_spans));

        // tvVWAP
        let vwap_str = t.tv_vwap_deviation
            .map(|d| {
                let color = if d > 0.0 { COLOR_BUY } else if d < 0.0 { COLOR_SELL } else { COLOR_DIM };
                (format!("{:+.2}%", d), color)
            })
            .unwrap_or(("--".to_string(), COLOR_DIM));

        lines.push(Line::from(vec![
            Span::styled("tvVWAP: ", Style::default().fg(COLOR_DIM)),
            Span::styled(vwap_str.0, Style::default().fg(vwap_str.1)),
        ]));

        // RV trend
        let trend = match t.realized_vol_trend {
            VolTrend::Expanding => ("EXP↑", COLOR_SELL),
            VolTrend::Contracting => ("CTR↓", COLOR_BUY),
            VolTrend::Stable => ("STB→", COLOR_DIM),
        };
        lines.push(Line::from(vec![
            Span::styled("RV: ", Style::default().fg(COLOR_DIM)),
            Span::styled(trend.0, Style::default().fg(trend.1)),
        ]));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

/// Exchange comparison table
fn render_exchanges(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    ticker: &str,
) {
    let block = Block::default()
        .title(" EXCHANGES (30s) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_ACCENT));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(ticker) {
        // Header
        lines.push(Line::from(vec![
            Span::styled("         ", Style::default()),
            Span::styled(format!("{:^10}", "OKX"), Style::default().fg(Color::Yellow)),
            Span::styled(format!("{:^10}", "BNC"), Style::default().fg(COLOR_ACCENT)),
            Span::styled(format!("{:^10}", "BBT"), Style::default().fg(Color::Magenta)),
        ]));

        let get_cvd = |ex: &str| -> (String, f64) {
            let norm = |n: &str| -> &'static str {
                if n.to_lowercase().contains("binance") { "binance" }
                else if n.to_lowercase().contains("bybit") { "bybit" }
                else if n.to_lowercase().contains("okx") { "okx" }
                else { "other" }
            };
            t.per_exchange_30s.iter()
                .find(|(k, _)| norm(k) == norm(ex))
                .map(|(_, v)| {
                    let (s, suffix) = scale_number(v.cvd_30s);
                    (format!("{:+.1}{}", s, suffix), v.cvd_30s)
                })
                .unwrap_or(("--".to_string(), 0.0))
        };

        let (cvd_okx, cvd_okx_raw) = get_cvd("Okx");
        let (cvd_bnc, cvd_bnc_raw) = get_cvd("BinanceFuturesUsd");
        let (cvd_bbt, cvd_bbt_raw) = get_cvd("BybitPerpetualsUsd");

        let cvd_color = |v: f64| if v > 0.0 { COLOR_BUY } else if v < 0.0 { COLOR_SELL } else { COLOR_DIM };

        lines.push(Line::from(vec![
            Span::styled("CVD:     ", Style::default().fg(COLOR_DIM)),
            Span::styled(format!("{:^10}", cvd_okx), Style::default().fg(cvd_color(cvd_okx_raw))),
            Span::styled(format!("{:^10}", cvd_bnc), Style::default().fg(cvd_color(cvd_bnc_raw))),
            Span::styled(format!("{:^10}", cvd_bbt), Style::default().fg(cvd_color(cvd_bbt_raw))),
        ]));

        // Flow imbalance row
        let get_imb = |ex: &str| -> (String, f64) {
            let norm = |n: &str| -> &'static str {
                if n.to_lowercase().contains("binance") { "binance" }
                else if n.to_lowercase().contains("bybit") { "bybit" }
                else if n.to_lowercase().contains("okx") { "okx" }
                else { "other" }
            };
            t.per_exchange_30s.iter()
                .find(|(k, _)| norm(k) == norm(ex))
                .map(|(_, v)| {
                    let buy = (v.total_30s + v.cvd_30s) / 2.0;
                    let imb = if v.total_30s > 0.0 { (buy / v.total_30s * 100.0).round() } else { 50.0 };
                    let label = if imb < 50.0 { "S" } else { "B" };
                    (format!("{:.0}%{}", imb, label), imb)
                })
                .unwrap_or(("--".to_string(), 50.0))
        };

        let (imb_okx, imb_okx_raw) = get_imb("Okx");
        let (imb_bnc, imb_bnc_raw) = get_imb("BinanceFuturesUsd");
        let (imb_bbt, imb_bbt_raw) = get_imb("BybitPerpetualsUsd");

        let imb_color = |v: f64| if v > 55.0 { COLOR_BUY } else if v < 45.0 { COLOR_SELL } else { COLOR_NEUTRAL };

        lines.push(Line::from(vec![
            Span::styled("FLOW:    ", Style::default().fg(COLOR_DIM)),
            Span::styled(format!("{:^10}", imb_okx), Style::default().fg(imb_color(imb_okx_raw))),
            Span::styled(format!("{:^10}", imb_bnc), Style::default().fg(imb_color(imb_bnc_raw))),
            Span::styled(format!("{:^10}", imb_bbt), Style::default().fg(imb_color(imb_bbt_raw))),
        ]));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

/// Whale tape
fn render_whale_tape(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    ticker: &str,
) {
    let threshold_k = whale_threshold() / 1000.0;
    let block = Block::default()
        .title(format!(" WHALE TAPE (>${:.0}K, 30s) ", threshold_k))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_ACCENT));

    let available_rows = area.height.saturating_sub(2) as usize;
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(ticker) {
        let now = chrono::Utc::now();
        let cutoff = now - chrono::Duration::seconds(30);

        let recent_whales: Vec<_> = t.whales.iter()
            .filter(|w| w.time >= cutoff)
            .take(available_rows)
            .collect();

        if recent_whales.is_empty() {
            lines.push(Line::from(Span::styled("No whale trades in last 30s", Style::default().fg(COLOR_DIM))));
        } else {
            for whale in recent_whales {
                let age = (now - whale.time).num_milliseconds() as f64 / 1000.0;
                let side_color = if whale.side == Side::Buy { COLOR_BUY } else { COLOR_SELL };

                let exchange_abbrev = match whale.exchange.as_str() {
                    "BinanceFuturesUsd" | "BinanceSpot" => "BNC",
                    "BybitPerpetualsUsd" | "BybitSpot" => "BBT",
                    "Okx" => "OKX",
                    _ => "OTH",
                };

                let vol_str = if whale.volume_usd >= 1_000_000.0 {
                    format!("${:.1}M", whale.volume_usd / 1_000_000.0)
                } else {
                    format!("${:.0}K", whale.volume_usd / 1_000.0)
                };

                let price_str = if whale.price >= 1000.0 {
                    format!("@{:.0}", whale.price)
                } else {
                    format!("@{:.2}", whale.price)
                };

                lines.push(Line::from(vec![
                    Span::raw("→ "),
                    Span::styled(format!("{:>7} ", vol_str), Style::default().fg(side_color).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{:4} ", whale.side.as_str().to_uppercase()), Style::default().fg(side_color)),
                    Span::styled(format!("{} ", price_str), Style::default().fg(Color::White)),
                    Span::styled(format!("[{}] ", exchange_abbrev), Style::default().fg(COLOR_ACCENT)),
                    Span::styled(format!("{:.1}s", age), Style::default().fg(COLOR_DIM)),
                ]));
            }
        }
    }

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

/// Footer with hotkeys
fn render_footer(f: &mut ratatui::Frame, area: Rect, ticker: &str) {
    let hotkeys = vec![
        Span::raw(" ["),
        Span::styled("B", if ticker == "BTC" { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(COLOR_DIM) }),
        Span::raw("]TC  ["),
        Span::styled("E", if ticker == "ETH" { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(COLOR_DIM) }),
        Span::raw("]TH  ["),
        Span::styled("S", if ticker == "SOL" { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(COLOR_DIM) }),
        Span::raw("]OL  |  "),
        Span::styled("SCALPER V2", Style::default().fg(COLOR_HEADER)),
        Span::raw("  |  [q] Quit"),
    ];

    f.render_widget(Paragraph::new(Line::from(hotkeys)), area);
}
