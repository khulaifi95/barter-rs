/// Scalper Mode Dashboard (Opus TUI #4)
///
/// High-frequency execution TUI with 50ms refresh rate.
/// Focus: Delta velocity, imbalance, tape speed for 5s-30s scalping windows.
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
    AggregatedSnapshot, Aggregator, ConnectionStatus, DivergenceSignal, FlowSignal, Side,
    VolTrend, WebSocketClient, WebSocketConfig,
};
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
use reqwest::Client;
use serde_json::Value;

/// Available tickers for focus mode
const TICKERS: [&str; 3] = ["BTC", "ETH", "SOL"];

/// Minimum time a signal must persist before displaying (prevents flickering)
const SIGNAL_DEBOUNCE_MS: u64 = 1000; // 1 second

/// Signal state tracker for debouncing
#[derive(Default)]
struct SignalState {
    /// When each ticker's divergence signal started (None = no signal or just changed)
    divergence_start: HashMap<String, (DivergenceSignal, Instant)>,
    /// When each ticker's flow signal started
    flow_start: HashMap<String, (FlowSignal, Instant)>,
}

/// Per-venue throttle state
#[derive(Default, Clone)]
struct VenueThrottle {
    last_value: f64,
    last_update: Option<Instant>,
}

impl VenueThrottle {
    /// Returns throttled value: updates ONLY when interval_ms elapsed AND raw has data
    /// If raw=0 (no data), keeps previous value (memory effect)
    fn get_throttled(&mut self, raw: f64, interval_ms: u128) -> f64 {
        let now = Instant::now();
        let time_ok = self.last_update
            .map(|t| now.duration_since(t).as_millis() >= interval_ms)
            .unwrap_or(true);

        // Only update if time elapsed AND raw has valid data (>0)
        // This preserves last known value when data briefly disappears
        if time_ok && raw > 0.0 {
            self.last_value = raw;
            self.last_update = Some(now);
        }
        self.last_value
    }
}

/// Bar state tracker for throttled visual updates (reduces bar flickering)
#[derive(Default)]
struct BarState {
    last_pressure: f64,
    last_flow_imb: f64,
    last_update: Option<Instant>,
    // tvVWAP throttling
    last_tv_dev: Option<f64>,
    last_tv_update: Option<Instant>,
    // L2 book throttling - INDEPENDENT per venue
    l2_bnc: VenueThrottle,
    l2_bbt: VenueThrottle,
    l2_okx: VenueThrottle,
    l2_agg: VenueThrottle,
}

// Throttle settings: pure time-based, no value escape hatch (data too volatile)
const L2_INTERVAL_MS: u128 = 1500;      // L2 book: ~0.67 updates/sec
const BANNER_INTERVAL_MS: u128 = 1200;  // Pressure banner: ~0.83 updates/sec (slightly snappier)
const TVWAP_INTERVAL_MS: u128 = 1000;   // tvVWAP display gate
const TVWAP_THRESHOLD: f64 = 0.05;      // Min change (in pct points) to refresh

impl BarState {
    /// Returns true if pressure banner should update (pure time gate)
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

    /// Returns throttled L2 values - EACH venue throttled independently (pure time gate)
    fn get_l2_throttled(&mut self, bnc: f64, bbt: f64, okx: f64, agg: f64) -> (f64, f64, f64, f64) {
        (
            self.l2_bnc.get_throttled(bnc, L2_INTERVAL_MS),
            self.l2_bbt.get_throttled(bbt, L2_INTERVAL_MS),
            self.l2_okx.get_throttled(okx, L2_INTERVAL_MS),
            self.l2_agg.get_throttled(agg, L2_INTERVAL_MS),
        )
    }

    /// Throttle tvVWAP deviation display to reduce flicker
    fn throttle_tvwav(&mut self, dev: Option<f64>) -> Option<f64> {
        let now = Instant::now();
        let time_ok = self
            .last_tv_update
            .map(|t| now.duration_since(t).as_millis() >= TVWAP_INTERVAL_MS)
            .unwrap_or(true);

        let change_ok = match (dev, self.last_tv_dev) {
            (Some(cur), Some(prev)) => (cur - prev).abs() >= TVWAP_THRESHOLD,
            (Some(_), None) => true,
            _ => false,
        };

        if time_ok && change_ok {
            self.last_tv_dev = dev;
            self.last_tv_update = Some(now);
        }

        self.last_tv_dev
    }
}

impl SignalState {
    /// Update divergence signal and return the debounced (stable) signal to display
    fn update_divergence(&mut self, ticker: &str, signal: DivergenceSignal) -> Option<DivergenceSignal> {
        let dominated = matches!(signal, DivergenceSignal::Bullish | DivergenceSignal::Bearish);

        if !dominated {
            // No actionable signal - clear tracking
            self.divergence_start.remove(ticker);
            return None;
        }

        let now = Instant::now();

        match self.divergence_start.get(ticker) {
            Some((prev_signal, start_time)) if *prev_signal == signal => {
                // Same signal continuing - check if stable long enough
                if now.duration_since(*start_time).as_millis() >= SIGNAL_DEBOUNCE_MS as u128 {
                    Some(signal)
                } else {
                    None // Still waiting for stability
                }
            }
            _ => {
                // New signal or changed - start tracking
                self.divergence_start.insert(ticker.to_string(), (signal, now));
                None // Don't show until stable
            }
        }
    }

    /// Update flow signal and return debounced signal
    fn update_flow(&mut self, ticker: &str, signal: FlowSignal) -> FlowSignal {
        let dominated = !matches!(signal, FlowSignal::Neutral);

        if !dominated {
            self.flow_start.remove(ticker);
            return signal; // Always show neutral
        }

        let now = Instant::now();

        match self.flow_start.get(ticker) {
            Some((prev_signal, start_time)) if *prev_signal == signal => {
                // Same signal continuing
                if now.duration_since(*start_time).as_millis() >= SIGNAL_DEBOUNCE_MS as u128 {
                    signal
                } else {
                    FlowSignal::Neutral // Show neutral while debouncing
                }
            }
            _ => {
                // New signal - start tracking
                self.flow_start.insert(ticker.to_string(), (signal, now));
                FlowSignal::Neutral // Show neutral while debouncing
            }
        }
    }
}

/// Get WebSocket URL from WS_URL env var (default: ws://127.0.0.1:9001)
fn get_ws_url() -> String {
    std::env::var("WS_URL").unwrap_or_else(|_| "ws://127.0.0.1:9001".to_string())
}

/// Get whale threshold from WHALE_THRESHOLD env var (default: $500,000)
fn whale_threshold() -> f64 {
    std::env::var("WHALE_THRESHOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(500_000.0)
}

/// Fetch BitMEX 24h historical volatility index (.BVOL24H)
async fn fetch_bvol24h(client: &Client) -> Result<f64, reqwest::Error> {
    let url = "https://www.bitmex.com/api/v1/instrument?symbol=.BVOL24H";
    let resp = client.get(url).send().await?.error_for_status()?;
    let v: Value = resp.json().await?;
    Ok(v.get(0)
        .and_then(|o| o.get("lastPrice"))
        .and_then(|p| p.as_f64())
        .unwrap_or(0.0))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Setup panic hook to restore terminal on crash
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic_info);
    }));

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Shared aggregation engine
    let aggregator = Arc::new(Mutex::new(Aggregator::new()));
    let connected = Arc::new(AtomicBool::new(false));
    let focus_index = Arc::new(AtomicUsize::new(0)); // 0=BTC, 1=ETH, 2=SOL

    // Backfill tvVWAP and ATR from historical data on startup (silently)
    {
        let mut guard = aggregator.lock().await;
        let _ = guard.backfill_all(&TICKERS).await;
    }

    // Background fetch for BitMEX BVOL24H (updates every 5 minutes)
    let bvol24h = Arc::new(Mutex::new(None::<f64>));
    {
        let bvol = Arc::clone(&bvol24h);
        tokio::spawn(async move {
            let client = Client::new();
            loop {
                if let Ok(val) = fetch_bvol24h(&client).await {
                    let mut lock = bvol.lock().await;
                    *lock = Some(val);
                }
                tokio::time::sleep(Duration::from_secs(300)).await;
            }
        });
    }

    // WebSocket client with larger buffer for high-frequency mode
    let ws_url = get_ws_url();
    let config = WebSocketConfig::new(ws_url)
        .with_ping_interval(Duration::from_secs(30))
        .with_reconnect_delay(Duration::from_secs(2))
        .with_channel_buffer_size(100_000); // Larger buffer for scalper
    let client = WebSocketClient::with_config(config);
    let (mut event_rx, mut status_rx) = client.start();

    // Event processor
    {
        let agg = Arc::clone(&aggregator);
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let mut guard = agg.lock().await;
                guard.process_event(event);
            }
        });
    }

    // Connection status tracker
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

    // UI loop - 50ms refresh for scalper (20Hz)
    let mut last_draw = Instant::now();
    let draw_interval = Duration::from_millis(50);

    // Signal debouncing state (prevents flickering)
    let mut signal_state = SignalState::default();
    // Bar throttling state (prevents bar flickering)
    let mut bar_state = BarState::default();

    let result = loop {
        if event::poll(Duration::from_millis(5))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                    // Focus switching: B=BTC, E=ETH, S=SOL
                    KeyCode::Char('b') | KeyCode::Char('B') => {
                        focus_index.store(0, Ordering::Relaxed);
                    }
                    KeyCode::Char('e') | KeyCode::Char('E') => {
                        focus_index.store(1, Ordering::Relaxed);
                    }
                    KeyCode::Char('s') | KeyCode::Char('S') => {
                        focus_index.store(2, Ordering::Relaxed);
                    }
                    // Tab to cycle through assets
                    KeyCode::Tab => {
                        let current = focus_index.load(Ordering::Relaxed);
                        focus_index.store((current + 1) % 3, Ordering::Relaxed);
                    }
                    _ => {}
                }
            }
        }

        if last_draw.elapsed() >= draw_interval {
            let snapshot = {
                let guard = aggregator.lock().await;
                guard.snapshot()
            };

            let connected_now = connected.load(Ordering::Relaxed);
            let focus_idx = focus_index.load(Ordering::Relaxed);
            let focused_ticker = TICKERS[focus_idx];

            // Update signal debouncing for focused ticker
            let debounced_signals = if let Some(t) = snapshot.tickers.get(focused_ticker) {
                let div = signal_state.update_divergence(focused_ticker, t.cvd_divergence_15s);
                let flow = signal_state.update_flow(focused_ticker, t.flow_signal);
                Some((div, flow))
            } else {
                None
            };

            let bvol_val = {
                let lock = bvol24h.lock().await;
                *lock
            };

            terminal.draw(|f| {
                render_scalper_ui(
                    f,
                    f.area(),
                    &snapshot,
                    connected_now,
                    focused_ticker,
                    debounced_signals,
                    &mut bar_state,
                    bvol_val,
                )
            })?;
            last_draw = Instant::now();
        }

        tokio::time::sleep(Duration::from_millis(5)).await;
    };

    // Cleanup
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn render_scalper_ui(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    connected: bool,
    focused_ticker: &str,
    debounced_signals: Option<(Option<DivergenceSignal>, FlowSignal)>,
    bar_state: &mut BarState,
    bvol24h: Option<f64>,
) {
    // TARGET LAYOUT (from mockup):
    // 1. Compact header: price + [LIVE] + t/s + Sprd + LEAD + Basis
    // 2. PRESSURE BANNER (full-width, colored bg)
    // 3. 3-column: DELTA VELOCITY | ORDERFLOW | L2 BOOK
    // 4. VOLATILITY | EXCHANGES (30s) side by side
    // 5. WHALE TAPE
    // 6. Footer (simplified)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Compact header
            Constraint::Length(3),  // PRESSURE banner
            Constraint::Length(6),  // 3-column metrics
            Constraint::Length(5),  // Volatility + Exchanges
            Constraint::Min(4),     // Whale tape
            Constraint::Length(1),  // Footer
        ])
        .split(area);

    render_header_compact_new(f, chunks[0], snapshot, connected, focused_ticker);
    render_pressure_banner_new(f, chunks[1], snapshot, focused_ticker, bar_state);

    // 3-column metrics: DELTA VELOCITY | ORDERFLOW | L2 BOOK
    let metrics_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(chunks[2]);

    render_delta_velocity_new(f, metrics_row[0], snapshot, focused_ticker);
    render_orderflow_new(f, metrics_row[1], snapshot, focused_ticker);
    render_l2_book_new(f, metrics_row[2], snapshot, focused_ticker, bar_state);

    // VOLATILITY | EXCHANGES side by side (Volatility same width as Delta Velocity)
    let vol_exch_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),  // Volatility (same as Delta Velocity)
            Constraint::Percentage(66),  // Exchanges
        ])
        .split(chunks[3]);

    render_volatility_new(f, vol_exch_row[0], snapshot, focused_ticker, bvol24h, bar_state);
    render_exchanges_table_new(f, vol_exch_row[1], snapshot, focused_ticker);

    render_whale_tape(f, chunks[4], snapshot, focused_ticker);
    render_footer_simple(f, chunks[5], focused_ticker);
}

fn render_header(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    connected: bool,
    focused_ticker: &str,
) {
    let (price_str, delta_str, freshness_str) = if let Some(t) = snapshot.tickers.get(focused_ticker) {
        let price = t.latest_price.unwrap_or(0.0);
        let price_fmt = if price >= 1000.0 {
            format!("${:.2}", price)
        } else {
            format!("${:.4}", price)
        };

        // Calculate 30s price change
        let delta = t.orderflow_30s.net_flow_per_min * 0.5; // 30s of flow
        let delta_pct = if t.vol_5m > 0.0 {
            (delta / t.vol_5m) * 100.0
        } else {
            0.0
        };
        let delta_fmt = format!("Δ: {:+.2}%", delta_pct);

        // Build exchange freshness string (show if any exchange is stale >2s)
        let mut freshness_parts = Vec::new();
        for (ex, age) in &t.exchange_health {
            let abbrev = match ex.as_str() {
                "BinanceFuturesUsd" | "BinanceSpot" => "BNC",
                "BybitPerpetualsUsd" | "BybitSpot" => "BBT",
                "Okx" => "OKX",
                _ => continue,
            };
            // Only show if stale (>2s) - otherwise assume fresh
            if *age > 2.0 {
                freshness_parts.push(format!("{}:{:.1}s", abbrev, age));
            }
        }
        let freshness = if freshness_parts.is_empty() {
            String::new() // All fresh, don't clutter
        } else {
            format!(" ⚠ {}", freshness_parts.join(" "))
        };

        (price_fmt, delta_fmt, freshness)
    } else {
        ("---".to_string(), "---".to_string(), String::new())
    };

    let status = if connected { "LIVE" } else { "DISCONNECTED" };
    let status_color = if connected { Color::Green } else { Color::Red };

    let block = Block::default()
        .title(format!(" SCALPER MODE - {} ", focused_ticker))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    let mut spans = vec![
        Span::styled(
            format!("Last: {} ", price_str),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(delta_str, Style::default().fg(Color::Yellow)),
        Span::raw("  "),
        Span::styled(format!("[{}]", status), Style::default().fg(status_color)),
    ];

    // Add stale exchange warning if any
    if !freshness_str.is_empty() {
        spans.push(Span::styled(freshness_str, Style::default().fg(Color::Red)));
    }

    let content = Line::from(spans);

    let paragraph = Paragraph::new(content).block(block);
    f.render_widget(paragraph, area);
}

/// Extended header with full execution context for 1-2s decision making
/// Line 1: Price + Status + Spread
/// Line 2: VWAP deviation + ATR + Vol regime
/// Line 3: OI velocity + Basis
fn render_header_extended(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    connected: bool,
    focused_ticker: &str,
) {
    let block = Block::default()
        .title(format!(" SCALPER - {} ", focused_ticker))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(focused_ticker) {
        let price = t.latest_price.unwrap_or(0.0);
        let price_str = if price >= 1000.0 {
            format!("${:.2}", price)
        } else {
            format!("${:.4}", price)
        };

        // Status
        let status = if connected { "LIVE" } else { "DISC" };
        let status_color = if connected { Color::Green } else { Color::Red };

        // Spread calculation from best bid/ask
        let spread_str = match (t.best_bid, t.best_ask) {
            (Some((bid, _)), Some((ask, _))) if bid > 0.0 => {
                let spread_pct = ((ask - bid) / bid) * 100.0;
                format!("Sprd:{:.3}%", spread_pct)
            }
            _ => "Sprd:---".to_string(),
        };
        let spread_color = match (t.best_bid, t.best_ask) {
            (Some((bid, _)), Some((ask, _))) if bid > 0.0 => {
                let spread_pct = ((ask - bid) / bid) * 100.0;
                if spread_pct > 0.05 { Color::Red } else if spread_pct > 0.02 { Color::Yellow } else { Color::Green }
            }
            _ => Color::Gray,
        };

        // Line 1: Price + Status + Spread + Tape speed
        lines.push(Line::from(vec![
            Span::styled(price_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(format!("[{}]", status), Style::default().fg(status_color)),
            Span::raw("  "),
            Span::styled(spread_str, Style::default().fg(spread_color)),
            Span::raw("  "),
            Span::styled(format!("{:.0}t/s", t.trade_speed), Style::default().fg(Color::Cyan)),
        ]));

        // VWAP deviation (where am I vs fair value?) - tvVWAP is daily reset
        let vwap_dev_str = t.tv_vwap_deviation
            .map(|d| format!("vVWAP(d):{:+.2}%", d))
            .unwrap_or_else(|| "vVWAP:---".to_string());
        let vwap_color = t.tv_vwap_deviation.map(|d| {
            if d > 0.5 { Color::Red }      // Overextended high
            else if d < -0.5 { Color::Green } // Overextended low (buy zone)
            else { Color::Yellow }
        }).unwrap_or(Color::Gray);

        // ATR-14 (5m candles) for risk sizing - show absolute + percentage
        let atr_str = match (t.atr_14, t.atr_14_pct) {
            (Some(atr_abs), Some(atr_pct)) => {
                if atr_abs >= 100.0 {
                    format!("ATR14:${:.0}({:.2}%)", atr_abs, atr_pct)
                } else {
                    format!("ATR14:${:.2}({:.2}%)", atr_abs, atr_pct)
                }
            }
            _ => "ATR14:---".to_string(),
        };

        // Vol regime (30m vs 1h realized volatility comparison)
        let vol_regime = t.realized_vol_trend.label();
        let vol_color = match t.realized_vol_trend {
            VolTrend::Expanding => Color::Red,
            VolTrend::Contracting => Color::Green,
            VolTrend::Stable => Color::Yellow,
        };
        // Show actual RV values with trend label
        let rv_label = match t.realized_vol_trend {
            VolTrend::Expanding => "EXP",
            VolTrend::Contracting => "CTR",
            VolTrend::Stable => "STB",
        };
        let rv_str = match (t.realized_vol_30m, t.realized_vol_1h) {
            (Some(rv30), Some(rv1h)) => format!("RV:{:.2}%/{:.2}%{}{}", rv30, rv1h, t.realized_vol_trend.arrow(), rv_label),
            _ => format!("RV:{}", vol_regime),
        };

        // Line 2: VWAP + ATR + Vol regime (with timeframes explicit)
        lines.push(Line::from(vec![
            Span::styled(vwap_dev_str, Style::default().fg(vwap_color)),
            Span::raw(" "),
            Span::styled(atr_str, Style::default().fg(Color::Cyan)),
        ]));

        // OI velocity (5m window) - institutional flow
        let oi_str = if t.oi_delta_5m.abs() > 100_000.0 {
            let (oi_val, oi_suffix) = scale_number(t.oi_delta_5m.abs());
            let oi_dir = if t.oi_delta_5m > 0.0 { "↑" } else { "↓" };
            let oi_label = if t.oi_delta_5m > 0.0 { "LONG" } else { "SHRT" };
            format!("OI(5m):{:+.1}{}{}{}", if t.oi_delta_5m > 0.0 { oi_val } else { -oi_val }, oi_suffix, oi_dir, oi_label)
        } else {
            "OI(5m):FLAT".to_string()
        };
        let oi_color = if t.oi_delta_5m > 500_000.0 { Color::Green }
            else if t.oi_delta_5m < -500_000.0 { Color::Red }
            else { Color::Gray };

        // Basis (spot vs perp)
        let basis_str = t.basis.as_ref().map(|b| {
            let state = if b.basis_pct > 0.01 { "CTG" }
                else if b.basis_pct < -0.01 { "BWD" }
                else { "FLT" };
            format!("Basis:{:+.3}%{}", b.basis_pct, state)
        }).unwrap_or_else(|| "Basis:---".to_string());
        let basis_color = t.basis.as_ref().map(|b| {
            if b.basis_pct > 0.03 { Color::Yellow }  // High funding, longs paying
            else if b.basis_pct < -0.03 { Color::Cyan } // Shorts paying
            else { Color::Gray }
        }).unwrap_or(Color::Gray);

        // Venue freshness warning (show if ANY exchange > 2s stale)
        let mut stale_venues = Vec::new();
        for (ex, age) in &t.exchange_health {
            if *age > 2.0 {
                let abbrev = match ex.as_str() {
                    "BinanceFuturesUsd" | "BinanceSpot" => "BNC",
                    "BybitPerpetualsUsd" | "BybitSpot" => "BBT",
                    "Okx" => "OKX",
                    _ => continue,
                };
                stale_venues.push(format!("{}:{:.0}s", abbrev, age));
            }
        }
        let freshness_warning = if stale_venues.is_empty() {
            None
        } else {
            Some(format!("⚠{}", stale_venues.join(" ")))
        };

        // Line 3: OI + Basis + RV + Freshness warning
        let mut line3_spans = vec![
            Span::styled(oi_str, Style::default().fg(oi_color)),
            Span::raw(" "),
            Span::styled(basis_str, Style::default().fg(basis_color)),
            Span::raw(" "),
            Span::styled(rv_str, Style::default().fg(vol_color)),
        ];
        if let Some(warning) = freshness_warning {
            line3_spans.push(Span::raw(" "));
            line3_spans.push(Span::styled(warning, Style::default().fg(Color::Red)));
        }
        lines.push(Line::from(line3_spans));

        // Line 4: Per-exchange CVD intelligence (who's buying/selling, divergence)
        let mut venue_spans = vec![Span::styled("EXC:", Style::default().fg(Color::DarkGray))];

        // Get per-exchange CVD (5m window for stability)
        let cvd_by_ex = &t.cvd_per_exchange_5m;
        let mut venue_data: Vec<(&str, &str, f64, Color)> = Vec::new();

        for (ex, cvd) in cvd_by_ex {
            let abbrev = match ex.as_str() {
                "BinanceFuturesUsd" => "BNC",
                "BybitPerpetualsUsd" => "BBT",
                "Okx" => "OKX",
                _ => continue,
            };
            let (arrow, color) = if *cvd > 50_000.0 {
                ("↑", Color::Green)
            } else if *cvd < -50_000.0 {
                ("↓", Color::Red)
            } else {
                ("→", Color::Gray)
            };
            venue_data.push((abbrev, arrow, *cvd, color));
        }

        // Sort by absolute CVD to find leader
        venue_data.sort_by(|a, b| b.2.abs().partial_cmp(&a.2.abs()).unwrap_or(std::cmp::Ordering::Equal));

        // Display each venue
        for (abbrev, arrow, cvd, color) in &venue_data {
            let (scaled, suffix) = scale_number(cvd.abs());
            let display = format!(" {}:{:.0}{}{}", abbrev, scaled, suffix, arrow);
            venue_spans.push(Span::styled(display, Style::default().fg(*color)));
        }

        // Determine leader and check for divergence
        if venue_data.len() >= 2 {
            // Leader is first (highest abs CVD)
            let leader = venue_data[0].0;
            venue_spans.push(Span::raw(" "));
            venue_spans.push(Span::styled(format!("LEAD:{}", leader), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));

            // Check divergence: if venues disagree on direction
            let directions: Vec<bool> = venue_data.iter()
                .filter(|(_, _, cvd, _)| cvd.abs() > 50_000.0) // Only count significant
                .map(|(_, _, cvd, _)| *cvd > 0.0)
                .collect();

            if directions.len() >= 2 {
                let has_buyers = directions.iter().any(|&d| d);
                let has_sellers = directions.iter().any(|&d| !d);
                if has_buyers && has_sellers {
                    venue_spans.push(Span::raw(" "));
                    venue_spans.push(Span::styled("⚠DIV", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
                }
            }
        }

        lines.push(Line::from(venue_spans));
    } else {
        lines.push(Line::from(Span::styled("Waiting for data...", Style::default().fg(Color::DarkGray))));
    }

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

fn render_delta_velocity(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    focused_ticker: &str,
    debounced_signals: Option<(Option<DivergenceSignal>, FlowSignal)>,
) {
    let block = Block::default()
        .title(" DELTA VELOCITY ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(focused_ticker) {
        // Real multi-timeframe CVD from actual tick data
        let cvd_5s = t.cvd_5s;
        let cvd_15s = t.cvd_15s;
        let cvd_30s = t.cvd_30s;
        let cvd_1m = t.cvd_1m_total;

        // Calculate velocity ($/second) from real data
        let velocity_5s = cvd_5s / 5.0;
        let velocity_30s = cvd_30s / 30.0;

        // Determine acceleration (compare 5s velocity vs 30s baseline)
        let acceleration = if velocity_5s > velocity_30s * 1.5 {
            ("ACCEL", Color::Green)
        } else if velocity_5s < velocity_30s * 0.5 {
            ("DECEL", Color::Red)
        } else {
            ("STEADY", Color::Yellow)
        };

        // Compact delta display
        let format_delta_compact = |val: f64| -> (String, Color) {
            let (scaled, suffix) = scale_number(val.abs());
            let arrow = if val > 0.0 { "↑" } else if val < 0.0 { "↓" } else { "→" };
            let color = if val > 0.0 { Color::Green } else if val < 0.0 { Color::Red } else { Color::Gray };
            (format!("{:+.1}{}{}", if val > 0.0 { scaled } else { -scaled }, suffix, arrow), color)
        };

        let (d5_str, d5_color) = format_delta_compact(cvd_5s);
        let (d15_str, d15_color) = format_delta_compact(cvd_15s);
        let (d30_str, d30_color) = format_delta_compact(cvd_30s);
        let (d1m_str, d1m_color) = format_delta_compact(cvd_1m);

        // Line 1: Velocity + acceleration
        lines.push(Line::from(vec![
            Span::styled("VEL: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:+.1}K/s", velocity_5s / 1000.0),
                Style::default().fg(d5_color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(acceleration.0, Style::default().fg(acceleration.1)),
        ]));

        // Line 2: Scalper timeframes (5s/15s/30s) - compact single line
        lines.push(Line::from(vec![
            Span::styled("5s:", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{} ", d5_str), Style::default().fg(d5_color)),
            Span::styled("15s:", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{} ", d15_str), Style::default().fg(d15_color)),
            Span::styled("30s:", Style::default().fg(Color::DarkGray)),
            Span::styled(d30_str, Style::default().fg(d30_color)),
        ]));

        // Line 3: 1m confirmation (context for scalp direction)
        let confirms_1m = (cvd_5s > 0.0 && cvd_1m > 0.0) || (cvd_5s < 0.0 && cvd_1m < 0.0);
        let confirm_badge = if confirms_1m { ("✓CONF", Color::Green) } else { ("✗DIV", Color::Yellow) };
        lines.push(Line::from(vec![
            Span::styled("1m:", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{} ", d1m_str), Style::default().fg(d1m_color)),
            Span::styled(confirm_badge.0, Style::default().fg(confirm_badge.1).add_modifier(Modifier::BOLD)),
        ]));

        // Use debounced signals (only show after stable for 1+ second)
        let (debounced_div, debounced_flow) = debounced_signals.unwrap_or((None, FlowSignal::Neutral));

        // 15s Divergence badge (only shows after stable for 1s - no flickering!)
        let div_15s = match debounced_div {
            Some(DivergenceSignal::Bullish) => Some(("⚡BULL", Color::Green)),
            Some(DivergenceSignal::Bearish) => Some(("⚡BEAR", Color::Red)),
            _ => None,
        };

        // Flow signal (debounced) - compact labels
        let signal = match debounced_flow {
            FlowSignal::Accumulation => ("ACCUM", Color::Green),
            FlowSignal::Distribution => ("DISTR", Color::Red),
            FlowSignal::Exhaustion => ("EXHST", Color::Yellow),
            FlowSignal::Confirmation => ("CONFM", Color::Blue),
            FlowSignal::Neutral => ("NETRL", Color::Gray),
        };

        // Line 4: Signal + divergence badge
        let mut signal_spans = vec![
            Span::styled("SIG: ", Style::default().fg(Color::DarkGray)),
            Span::styled(signal.0, Style::default().fg(signal.1).add_modifier(Modifier::BOLD)),
        ];

        if let Some((div_text, div_color)) = div_15s {
            signal_spans.push(Span::raw(" "));
            signal_spans.push(Span::styled(div_text, Style::default().fg(div_color).add_modifier(Modifier::BOLD)));
        }

        lines.push(Line::from(signal_spans));
    } else {
        lines.push(Line::from(Span::styled(
            "Waiting for data...",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

fn render_imbalance(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    focused_ticker: &str,
) {
    let block = Block::default()
        .title(" IMBALANCE ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(focused_ticker) {
        let imb_30s = t.orderflow_30s.imbalance_pct;
        let imb_1m = t.orderflow_1m.imbalance_pct;

        // Visual bar for imbalance
        let bar_width = 20;
        let filled = ((imb_30s / 100.0) * bar_width as f64).round() as usize;
        let bar = format!(
            "[{}{}]",
            "█".repeat(filled.min(bar_width)),
            "░".repeat(bar_width.saturating_sub(filled))
        );

        let (side_label, side_color) = if imb_30s >= 55.0 {
            ("BUYERS", Color::Green)
        } else if imb_30s <= 45.0 {
            ("SELLERS", Color::Red)
        } else {
            ("BALANCED", Color::Yellow)
        };

        lines.push(Line::from(vec![
            Span::styled(
                format!("{:.0}%", imb_30s),
                Style::default().fg(side_color).add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(Span::styled(bar, Style::default().fg(side_color))));
        lines.push(Line::from(Span::styled(side_label, Style::default().fg(side_color))));

        // 1m for comparison
        let trend = if imb_30s > imb_1m + 3.0 {
            "↑ strengthening"
        } else if imb_30s < imb_1m - 3.0 {
            "↓ weakening"
        } else {
            "→ steady"
        };
        lines.push(Line::from(Span::styled(
            format!("1m: {:.0}% {}", imb_1m, trend),
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "---",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

fn render_tape_speed(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    focused_ticker: &str,
) {
    let block = Block::default()
        .title(" TAPE SPEED ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(focused_ticker) {
        let speed = t.trade_speed;
        let (speed_label, speed_color) = if speed >= 200.0 {
            ("VERY HIGH", Color::Red)
        } else if speed >= 100.0 {
            ("HIGH", Color::Yellow)
        } else if speed >= 50.0 {
            ("MEDIUM", Color::Green)
        } else {
            ("LOW", Color::Gray)
        };

        lines.push(Line::from(vec![
            Span::styled(
                format!("{:.0}/s", speed),
                Style::default().fg(speed_color).add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(Span::styled(speed_label, Style::default().fg(speed_color))));

        // Avg trade size
        let avg = t.avg_trade_usd;
        let avg_str = if avg >= 1000.0 {
            format!("${:.1}K avg", avg / 1000.0)
        } else {
            format!("${:.0} avg", avg)
        };
        lines.push(Line::from(Span::styled(
            avg_str,
            Style::default().fg(Color::DarkGray),
        )));

        // Large trade ratio (whale trades / total)
        let whale_count = t.whales.len();
        let total_trades = t.trades_5m;
        let large_ratio = if total_trades > 0 {
            (whale_count as f64 / total_trades as f64) * 100.0
        } else {
            0.0
        };
        lines.push(Line::from(Span::styled(
            format!("Large: {:.1}%", large_ratio),
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "---",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

/// Compact imbalance panel (2 lines max)
fn render_imbalance_compact(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    focused_ticker: &str,
) {
    let block = Block::default()
        .title(" IMBALANCE ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(focused_ticker) {
        let imb_30s = t.orderflow_30s.imbalance_pct;
        let imb_1m = t.orderflow_1m.imbalance_pct;

        // Compact bar (12 chars)
        let bar_width = 12;
        let filled = ((imb_30s / 100.0) * bar_width as f64).round() as usize;
        let bar = format!(
            "[{}{}]",
            "█".repeat(filled.min(bar_width)),
            "░".repeat(bar_width.saturating_sub(filled))
        );

        let (side_label, side_color) = if imb_30s >= 55.0 {
            ("BUY", Color::Green)
        } else if imb_30s <= 45.0 {
            ("SELL", Color::Red)
        } else {
            ("BAL", Color::Yellow)
        };

        // Line 1: Percentage + bar + label
        lines.push(Line::from(vec![
            Span::styled(format!("{:.0}% ", imb_30s), Style::default().fg(side_color).add_modifier(Modifier::BOLD)),
            Span::styled(bar, Style::default().fg(side_color)),
            Span::styled(format!(" {}", side_label), Style::default().fg(side_color)),
        ]));

        // Line 2: 1m comparison
        let trend = if imb_30s > imb_1m + 3.0 { "↑str" }
            else if imb_30s < imb_1m - 3.0 { "↓wkn" }
            else { "→std" };
        lines.push(Line::from(Span::styled(
            format!("1m:{:.0}% {}", imb_1m, trend),
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(Span::styled("---", Style::default().fg(Color::DarkGray))));
    }

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

/// Compact tape speed panel (2 lines max)
fn render_tape_speed_compact(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    focused_ticker: &str,
) {
    let block = Block::default()
        .title(" TAPE ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(focused_ticker) {
        let speed = t.trade_speed;
        let (speed_label, speed_color) = if speed >= 200.0 {
            ("VHIGH", Color::Red)
        } else if speed >= 100.0 {
            ("HIGH", Color::Yellow)
        } else if speed >= 50.0 {
            ("MED", Color::Green)
        } else {
            ("LOW", Color::Gray)
        };

        // Line 1: Speed + label
        lines.push(Line::from(vec![
            Span::styled(format!("{:.0}/s ", speed), Style::default().fg(speed_color).add_modifier(Modifier::BOLD)),
            Span::styled(speed_label, Style::default().fg(speed_color)),
        ]));

        // Line 2: Avg size + large %
        let avg = t.avg_trade_usd;
        let avg_str = if avg >= 1000.0 { format!("${:.1}K", avg / 1000.0) } else { format!("${:.0}", avg) };
        let whale_count = t.whales.len();
        let large_pct = if t.trades_5m > 0 { (whale_count as f64 / t.trades_5m as f64) * 100.0 } else { 0.0 };
        lines.push(Line::from(Span::styled(
            format!("{} avg | Lg:{:.1}%", avg_str, large_pct),
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(Span::styled("---", Style::default().fg(Color::DarkGray))));
    }

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

fn render_whale_tape(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    focused_ticker: &str,
) {
    let threshold_k = whale_threshold() / 1000.0;
    let block = Block::default()
        .title(format!(" WHALE TAPE (>${:.0}K, 30s) ", threshold_k))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let available_rows = area.height.saturating_sub(2) as usize;
    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(focused_ticker) {
        let now = chrono::Utc::now();
        let cutoff = now - chrono::Duration::seconds(30);

        // Filter whales to last 30s
        let recent_whales: Vec<_> = t
            .whales
            .iter()
            .filter(|w| w.time >= cutoff)
            .take(available_rows)
            .collect();

        if recent_whales.is_empty() {
            lines.push(Line::from(Span::styled(
                "No whale trades in last 30s",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            for whale in recent_whales {
                let age = (now - whale.time).num_milliseconds() as f64 / 1000.0;
                let side_color = if whale.side == Side::Buy {
                    Color::Green
                } else {
                    Color::Red
                };

                // Exchange abbreviation + market kind (consistent with TUI1)
                let exchange_abbrev = match whale.exchange.as_str() {
                    "BinanceFuturesUsd" | "BinanceSpot" => "BNC",
                    "BybitPerpetualsUsd" | "BybitSpot" => "BBT",
                    "Okx" => "OKX",
                    _ => "OTH",
                };
                let exch_label = format!("[{}-{}]", exchange_abbrev, whale.market_kind);

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
                    Span::styled(
                        format!("{} ", vol_str),
                        Style::default().fg(side_color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{:4} ", whale.side.as_str().to_uppercase()),
                        Style::default().fg(side_color),
                    ),
                    Span::raw(format!("{} ", price_str)),
                    Span::styled(format!("{} ", exch_label), Style::default().fg(Color::Cyan)),
                    Span::styled(
                        format!("← {:.1}s", age),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }
        }
    } else {
        lines.push(Line::from(Span::styled(
            "Waiting for data...",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
}

fn render_per_exchange_strip(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    focused_ticker: &str,
) {
    let block = Block::default()
        .title(" EXCHANGES (30s) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(focused_ticker) {
        // Header
        lines.push(Line::from(vec![
            Span::styled("     ", Style::default().fg(Color::DarkGray)),
            Span::styled("OKX", Style::default().fg(Color::Yellow)),
            Span::raw("     "),
            Span::styled("BNC", Style::default().fg(Color::Cyan)),
            Span::raw("     "),
            Span::styled("BBT", Style::default().fg(Color::Magenta)),
        ]));

        // Helper to format stats per venue
        let fmt_stats = |ex: &str| -> (String, String, String) {
            let stats = t
                .per_exchange_30s
                .iter()
                .find(|(k, _)| normalize_ex(k) == normalize_ex(ex))
                .map(|(_, v)| v);
            if let Some(v) = stats {
                let buy = (v.total_30s + v.cvd_30s) / 2.0;
                let imb = if v.total_30s > 0.0 {
                    (buy / v.total_30s * 100.0).round()
                } else {
                    50.0
                };
                let tps = v.trades_30s as f64 / 30.0;
                let avg = if v.trades_30s > 0 {
                    v.total_30s / v.trades_30s as f64
                } else {
                    0.0
                };
                let (scaled, suffix) = scale_number(v.cvd_30s);
                let cvd_str = format!("{:+.1}{}", scaled, suffix);
                let imb_str = format!("{:>3.0}% {}", imb, if imb >= 50.0 { "BUY" } else { "SELL" });
                let tape_str = format!("{:.0}t/s ${:.0}", tps, avg / 1000.0);
                (cvd_str, imb_str, tape_str)
            } else {
                ("--".to_string(), "--".to_string(), "--".to_string())
            }
        };

        // Line: CVD
        let (cvd_okx, cvd_bnc, cvd_bbt) = (fmt_stats("Okx").0, fmt_stats("BinanceFuturesUsd").0, fmt_stats("BybitPerpetualsUsd").0);
        lines.push(Line::from(vec![
            Span::styled(" CVD: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{:>10}   {:>10}   {:>10}", cvd_okx, cvd_bnc, cvd_bbt)),
        ]));
        // Line: Imbalance
        let (imb_okx, imb_bnc, imb_bbt) = (fmt_stats("Okx").1, fmt_stats("BinanceFuturesUsd").1, fmt_stats("BybitPerpetualsUsd").1);
        lines.push(Line::from(vec![
            Span::styled(" IMB: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{:>10}   {:>10}   {:>10}", imb_okx, imb_bnc, imb_bbt)),
        ]));
        // Line: Tape
        let (tp_okx, tp_bnc, tp_bbt) = (fmt_stats("Okx").2, fmt_stats("BinanceFuturesUsd").2, fmt_stats("BybitPerpetualsUsd").2);
        lines.push(Line::from(vec![
            Span::styled(" TAPE:", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{:>10}   {:>10}   {:>10}", tp_okx, tp_bnc, tp_bbt)),
        ]));

        // Line: L2 Book Imbalance (from orderbook data)
        // Show "--" for exchanges without L2 data (e.g., OKX)
        let book_okx = t.per_exchange_book_imbalance.get("OKX").copied();
        let book_bnc = t.per_exchange_book_imbalance.get("BNC").copied();
        let book_bbt = t.per_exchange_book_imbalance.get("BBT").copied();

        let fmt_book = |imb: Option<f64>| -> Span {
            match imb.filter(|&v| v > 0.0) {
                Some(val) => {
                    let (color, label) = if val > 55.0 {
                        (Color::Green, "BID")
                    } else if val < 45.0 {
                        (Color::Red, "ASK")
                    } else {
                        (Color::Yellow, "BAL")
                    };
                    Span::styled(format!("{:>5.0}%{} ", val, label), Style::default().fg(color))
                }
                None => Span::styled("   --    ", Style::default().fg(Color::DarkGray)),
            }
        };

        if !t.per_exchange_book_imbalance.is_empty() {
            lines.push(Line::from(vec![
                Span::styled(" BOOK:", Style::default().fg(Color::DarkGray)),
                Span::raw("   "),
                fmt_book(book_okx),
                Span::raw("       "),
                fmt_book(book_bnc),
                Span::raw("       "),
                fmt_book(book_bbt),
            ]));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "Waiting for data...",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
}

fn normalize_ex(name: &str) -> &str {
    let lower = name.to_lowercase();
    if lower.contains("binance") {
        "binance"
    } else if lower.contains("bybit") {
        "bybit"
    } else if lower.contains("okx") {
        "okx"
    } else {
        name
    }
}

fn render_footer(f: &mut ratatui::Frame, area: Rect, focused_ticker: &str) {
    let hotkeys = vec![
        Span::raw(" ["),
        Span::styled(
            "B",
            if focused_ticker == "BTC" {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
        Span::raw("]TC  ["),
        Span::styled(
            "E",
            if focused_ticker == "ETH" {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
        Span::raw("]TH  ["),
        Span::styled(
            "S",
            if focused_ticker == "SOL" {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
        Span::raw("]OL  |  Refresh: 50ms  |  [q] Quit"),
    ];

    let footer = Paragraph::new(Line::from(hotkeys));
    f.render_widget(footer, area);
}

/// Scale large numbers into a compact value + suffix (K/M/B)
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

// ============================================================================
// NEW RENDER FUNCTIONS FOR TARGET LAYOUT
// ============================================================================

/// Compact header: price + [LIVE] + spread + t/s + vVWAP + ATR + Basis
fn render_header_compact_new(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    connected: bool,
    focused_ticker: &str,
) {
    let block = Block::default()
        .title(format!(" SCALPER - {} ", focused_ticker))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Some(t) = snapshot.tickers.get(focused_ticker) {
        let price = t.latest_price.unwrap_or(0.0);
        let price_str = if price >= 1000.0 {
            format!("${:.2}", price)
        } else {
            format!("${:.4}", price)
        };

        let status = if connected { "LIVE" } else { "DISC" };
        let status_color = if connected { Color::Green } else { Color::Red };

        let spread_pct = t.latest_spread_pct.unwrap_or(0.0);
        let basis = t.basis.as_ref().map(|b| b.basis_pct).unwrap_or(0.0);
        let tps = t.trade_speed;

        // Get LEAD exchange (keys are like "BNC-PERP", "BBT-SPOT", "OKX-PERP")
        let lead = t.exchange_dominance.iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(k, _)| k.clone());
        let lead_short = lead.as_ref().map(|ex| {
            let ex_upper = ex.to_uppercase();
            if ex_upper.starts_with("BNC") { "BNC" }
            else if ex_upper.starts_with("BBT") { "BBT" }
            else if ex_upper.starts_with("OKX") { "OKX" }
            else { "OTH" }
        }).unwrap_or("--");

        // OI direction and percentage (5m context)
        let oi_vel = t.oi_velocity;
        let oi_arrow = if oi_vel > 0.3 { "↑" } else if oi_vel < -0.3 { "↓" } else { "→" };
        let oi_color = if oi_vel > 0.3 { Color::Green } else if oi_vel < -0.3 { Color::Red } else { Color::Gray };
        // Calculate OI change % from delta and total
        let oi_pct = if t.oi_total > 0.0 { (t.oi_delta_5m / t.oi_total) * 100.0 } else { 0.0 };

        let line = Line::from(vec![
            Span::styled(price_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(format!("[{}]", status), Style::default().fg(status_color)),
            Span::raw("  "),
            Span::styled(format!("{:.0}t/s", tps), Style::default().fg(Color::Cyan)),
            Span::raw("  "),
            Span::styled(format!("Sprd:{:.2}%", spread_pct), Style::default().fg(Color::Gray)),
            Span::raw("  "),
            Span::styled(format!("LEAD:{}", lead_short), Style::default().fg(Color::Yellow)),
            Span::raw("  "),
            Span::styled(format!("OI:{}{:+.1}%", oi_arrow, oi_pct), Style::default().fg(oi_color)),
            Span::raw("  "),
            Span::styled(
                format!("Basis:{:+.2}%", basis),
                Style::default().fg(if basis > 0.02 { Color::Green } else if basis < -0.02 { Color::Red } else { Color::Yellow }),
            ),
        ]);

        let para = Paragraph::new(line);
        f.render_widget(para, inner);
    }
}

/// Full-width PRESSURE banner with colored background
fn render_pressure_banner_new(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    focused_ticker: &str,
    bar_state: &mut BarState,
) {
    if let Some(t) = snapshot.tickers.get(focused_ticker) {
        let flow_imb = t.orderflow_1m.imbalance_pct;
        let cvd_dir = if t.cvd_1m_total > 0.0 { 1.0 } else { -1.0 };
        let book_imb = if t.aggregated_book_imbalance > 0.0 { t.aggregated_book_imbalance } else { 50.0 };

        let pressure_raw = (flow_imb * 0.4 + book_imb * 0.3 + (50.0 + cvd_dir * 20.0) * 0.3).clamp(0.0, 100.0);

        // Use throttled value for bar stability
        let _update = bar_state.should_update(pressure_raw, flow_imb);
        let pressure = bar_state.last_pressure;

        let (label, bg_color, fg_color) = if pressure > 60.0 {
            ("BUY PRESSURE", Color::Rgb(0, 80, 0), Color::Green)
        } else if pressure < 40.0 {
            ("SELL PRESSURE", Color::Rgb(80, 0, 0), Color::Red)
        } else {
            ("NEUTRAL", Color::Rgb(40, 40, 40), Color::Yellow)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(fg_color))
            .style(Style::default().bg(bg_color));

        let inner = block.inner(area);
        f.render_widget(block, area);

        // Centered label with pressure value
        let label_text = format!("{} ({:.0}%)", label, pressure);
        let padding = (inner.width as usize).saturating_sub(label_text.len()) / 2;

        let line = Line::from(vec![
            Span::raw(" ".repeat(padding)),
            Span::styled(label_text, Style::default().fg(fg_color).add_modifier(Modifier::BOLD)),
        ]);

        let para = Paragraph::new(line);
        f.render_widget(para, inner);
    }
}

/// DELTA VELOCITY panel
fn render_delta_velocity_new(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    focused_ticker: &str,
) {
    let block = Block::default()
        .title(" DELTA VELOCITY ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(focused_ticker) {
        let velocity = t.cvd_5s / 5.0;
        let (vel_scaled, vel_suffix) = scale_number(velocity);
        let vel_color = if velocity > 0.0 { Color::Green } else { Color::Red };

        let accel = if velocity.abs() > (t.cvd_30s / 30.0).abs() * 1.5 { "ACCEL" }
            else if velocity.abs() < (t.cvd_30s / 30.0).abs() * 0.5 { "DECEL" }
            else { "STEADY" };

        lines.push(Line::from(vec![
            Span::styled("VEL: ", Style::default().fg(Color::Gray)),
            Span::styled(format!("{:>+8.1}{}/s", vel_scaled, vel_suffix), Style::default().fg(vel_color).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(accel, Style::default().fg(Color::Gray)),
        ]));

        // Multi-TF CVD
        let (d5, s5) = scale_number(t.cvd_5s);
        let (d15, s15) = scale_number(t.cvd_15s);
        let (d30, s30) = scale_number(t.cvd_30s);
        let (d1m, s1m) = scale_number(t.cvd_1m_total);

        lines.push(Line::from(vec![
            Span::styled("5s: ", Style::default().fg(Color::Gray)),
            Span::styled(format!("{:>+6.0}{}", d5, s5), Style::default().fg(if t.cvd_5s > 0.0 { Color::Green } else { Color::Red })),
            Span::raw("   "),
            Span::styled("15s: ", Style::default().fg(Color::Gray)),
            Span::styled(format!("{:>+6.0}{}", d15, s15), Style::default().fg(if t.cvd_15s > 0.0 { Color::Green } else { Color::Red })),
        ]));

        lines.push(Line::from(vec![
            Span::styled("30s:", Style::default().fg(Color::Gray)),
            Span::styled(format!("{:>+6.0}{}", d30, s30), Style::default().fg(if t.cvd_30s > 0.0 { Color::Green } else { Color::Red })),
            Span::raw("   "),
            Span::styled("1m: ", Style::default().fg(Color::Gray)),
            Span::styled(format!("{:>+6.0}{}", d1m, s1m), Style::default().fg(if t.cvd_1m_total > 0.0 { Color::Green } else { Color::Red })),
        ]));

        // Confirmation count
        let mut confirms = 0;
        if t.cvd_5s > 0.0 { confirms += 1; }
        if t.cvd_15s > 0.0 { confirms += 1; }
        if t.cvd_30s > 0.0 { confirms += 1; }
        if t.cvd_1m_total > 0.0 { confirms += 1; }
        let neg = 4 - confirms;
        let (conf_label, conf_color) = if confirms >= 3 {
            (format!("✓CONF ({}/4 aligned)", confirms), Color::Green)
        } else if neg >= 3 {
            (format!("✓CONF ({}/4 aligned)", neg), Color::Red)
        } else {
            ("MIXED".to_string(), Color::Yellow)
        };

        lines.push(Line::from(Span::styled(conf_label, Style::default().fg(conf_color))));
    }

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

/// ORDERFLOW panel with imbalance bar
fn render_orderflow_new(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    focused_ticker: &str,
) {
    let block = Block::default()
        .title(" ORDERFLOW ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(focused_ticker) {
        let imb = t.orderflow_1m.imbalance_pct;
        let label = if imb > 55.0 { "BUY" } else if imb < 45.0 { "SELL" } else { "BAL" };
        let color = if imb > 55.0 { Color::Green } else if imb < 45.0 { Color::Red } else { Color::Yellow };

        // Visual bar
        let bar_width: usize = 10;
        let fill = ((imb / 100.0) * bar_width as f64) as usize;
        let bar = format!("{}{}", "█".repeat(fill), "░".repeat(bar_width.saturating_sub(fill)));

        lines.push(Line::from(vec![
            Span::styled(format!("{:.0}%", imb), Style::default().fg(color).add_modifier(Modifier::BOLD)),
            Span::raw(" ["),
            Span::styled(bar, Style::default().fg(color)),
            Span::raw("] "),
            Span::styled(label, Style::default().fg(color).add_modifier(Modifier::BOLD)),
        ]));

        // Empty line for spacing
        lines.push(Line::from(""));

        // 1m trend
        let imb_30s = t.orderflow_30s.imbalance_pct;
        let trend = if imb_30s > imb + 3.0 { "↑strengthening" }
            else if imb_30s < imb - 3.0 { "↓weakening" }
            else { "→steady" };
        lines.push(Line::from(vec![
            Span::styled("1m: ", Style::default().fg(Color::Gray)),
            Span::styled(format!("{:.0}% {}", imb, trend), Style::default().fg(Color::White)),
        ]));
    }

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

/// L2 BOOK panel with per-venue imbalance (throttled: 3% change OR 300ms)
fn render_l2_book_new(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    focused_ticker: &str,
    bar_state: &mut BarState,
) {
    let block = Block::default()
        .title(" L2 BOOK ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(focused_ticker) {
        // Get raw values (default 0 = no data flag)
        let raw_bnc = t.per_exchange_book_imbalance.get("BNC").copied().unwrap_or(0.0);
        let raw_bbt = t.per_exchange_book_imbalance.get("BBT").copied().unwrap_or(0.0);
        let raw_okx = t.per_exchange_book_imbalance.get("OKX").copied().unwrap_or(0.0);
        let raw_agg = t.aggregated_book_imbalance;

        // Get THROTTLED values - these are stable and "remember" last valid state
        let (bnc, bbt, okx, agg) = bar_state.get_l2_throttled(raw_bnc, raw_bbt, raw_okx, raw_agg);

        // Presence based on THROTTLED value (stable!), not raw
        // Throttle starts at 0, so if data never arrived, shows "no L2"
        // Once data arrives (>0), throttle holds that value even if raw briefly drops
        let venues = [("BNC", bnc, bnc > 1.0), ("BBT", bbt, bbt > 1.0), ("OKX", okx, okx > 1.0)];

        for (label, imb, has_data) in venues {
            if has_data {  // has_data = throttled > 1.0, already implies valid
                let dir = if imb > 55.0 { "BID" } else if imb < 45.0 { "ASK" } else { "BAL" };
                let color = if imb > 55.0 { Color::Green } else if imb < 45.0 { Color::Red } else { Color::Yellow };
                let bar_w: usize = 8;
                let fill = ((imb / 100.0) * bar_w as f64) as usize;
                let bar = format!("{}{}", "█".repeat(fill), "░".repeat(bar_w.saturating_sub(fill)));

                lines.push(Line::from(vec![
                    Span::styled(format!("{}: ", label), Style::default().fg(Color::Gray)),
                    Span::styled(format!("{:>3.0}% {}", imb, dir), Style::default().fg(color)),
                    Span::raw(" "),
                    Span::styled(bar, Style::default().fg(color)),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(format!("{}: ", label), Style::default().fg(Color::Gray)),
                    Span::styled("-- (no L2)", Style::default().fg(Color::DarkGray)),
                ]));
            }
        }

        // Aggregate (also throttled - use same threshold for consistency)
        if agg > 1.0 {
            let dir = if agg > 55.0 { "BID" } else if agg < 45.0 { "ASK" } else { "BAL" };
            let color = if agg > 55.0 { Color::Green } else if agg < 45.0 { Color::Red } else { Color::Yellow };
            lines.push(Line::from(vec![
                Span::styled("AGG: ", Style::default().fg(Color::White)),
                Span::styled(format!("{:.0}% {}", agg, dir), Style::default().fg(color).add_modifier(Modifier::BOLD)),
            ]));
        }
    }

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

/// EXCHANGES (30s) table with CVD/FLOW IMB/BOOK IMB rows
fn render_exchanges_table_new(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    focused_ticker: &str,
) {
    let block = Block::default()
        .title(" EXCHANGES (30s) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(focused_ticker) {
        // Header row - align with data columns below
        lines.push(Line::from(vec![
            Span::styled("            ", Style::default()),
            Span::styled(format!("{:^12}", "OKX"), Style::default().fg(Color::Yellow)),
            Span::styled(format!("{:^12}", "BNC"), Style::default().fg(Color::Cyan)),
            Span::styled(format!("{:^12}", "BBT"), Style::default().fg(Color::Magenta)),
        ]));

        // Helper to get stats with raw values for coloring
        let get_stats = |ex: &str| -> (String, f64, String, f64) {
            let stats = t.per_exchange_30s.iter()
                .find(|(k, _)| normalize_ex(k) == normalize_ex(ex))
                .map(|(_, v)| v);
            if let Some(v) = stats {
                let (cvd_scaled, cvd_suffix) = scale_number(v.cvd_30s);
                let buy = (v.total_30s + v.cvd_30s) / 2.0;
                let imb = if v.total_30s > 0.0 { (buy / v.total_30s * 100.0).round() } else { 50.0 };
                let imb_label = if imb < 50.0 { "SELL" } else { "BUY" };
                (format!("{:+.1}{}", cvd_scaled, cvd_suffix), v.cvd_30s, format!("{:.0}% {}", imb, imb_label), imb)
            } else {
                ("--".to_string(), 0.0, "--".to_string(), 50.0)
            }
        };

        let (cvd_okx, cvd_okx_raw, imb_okx, imb_okx_raw) = get_stats("Okx");
        let (cvd_bnc, cvd_bnc_raw, imb_bnc, imb_bnc_raw) = get_stats("BinanceFuturesUsd");
        let (cvd_bbt, cvd_bbt_raw, imb_bbt, imb_bbt_raw) = get_stats("BybitPerpetualsUsd");

        // Find dominant CVD (largest absolute value)
        let cvd_vals = [(cvd_okx_raw.abs(), 0), (cvd_bnc_raw.abs(), 1), (cvd_bbt_raw.abs(), 2)];
        let dominant_cvd = cvd_vals.iter().max_by(|a, b| a.0.partial_cmp(&b.0).unwrap()).map(|x| x.1).unwrap_or(0);

        // Color function: green for positive, red for negative, bold for dominant
        let cvd_color = |val: f64, is_dominant: bool| -> Style {
            let color = if val > 0.0 { Color::Green } else if val < 0.0 { Color::Red } else { Color::Gray };
            let style = Style::default().fg(color);
            if is_dominant { style.add_modifier(Modifier::BOLD) } else { style }
        };

        let imb_color = |val: f64| -> Style {
            let color = if val > 55.0 { Color::Green } else if val < 45.0 { Color::Red } else { Color::Yellow };
            Style::default().fg(color)
        };

        // CVD row - color coded
        lines.push(Line::from(vec![
            Span::styled("CVD:        ", Style::default().fg(Color::Gray)),
            Span::styled(format!("{:^12}", cvd_okx), cvd_color(cvd_okx_raw, dominant_cvd == 0)),
            Span::styled(format!("{:^12}", cvd_bnc), cvd_color(cvd_bnc_raw, dominant_cvd == 1)),
            Span::styled(format!("{:^12}", cvd_bbt), cvd_color(cvd_bbt_raw, dominant_cvd == 2)),
        ]));

        // FLOW IMB row - color coded
        lines.push(Line::from(vec![
            Span::styled("FLOW IMB:   ", Style::default().fg(Color::Gray)),
            Span::styled(format!("{:^12}", imb_okx), imb_color(imb_okx_raw)),
            Span::styled(format!("{:^12}", imb_bnc), imb_color(imb_bnc_raw)),
            Span::styled(format!("{:^12}", imb_bbt), imb_color(imb_bbt_raw)),
        ]));

        // BOOK IMB row (0.0 = no data)
        let book_okx = t.per_exchange_book_imbalance.get("OKX").filter(|&&v| v > 0.0).map(|v| format!("{:.0}%", v)).unwrap_or("--".to_string());
        let book_bnc = t.per_exchange_book_imbalance.get("BNC").filter(|&&v| v > 0.0).map(|v| format!("{:.0}%", v)).unwrap_or("--".to_string());
        let book_bbt = t.per_exchange_book_imbalance.get("BBT").filter(|&&v| v > 0.0).map(|v| format!("{:.0}%", v)).unwrap_or("--".to_string());

        lines.push(Line::from(vec![
            Span::styled("BOOK IMB:   ", Style::default().fg(Color::Gray)),
            Span::styled(format!("{:^12}", book_okx), Style::default().fg(Color::White)),
            Span::styled(format!("{:^12}", book_bnc), Style::default().fg(Color::White)),
            Span::styled(format!("{:^12}", book_bbt), Style::default().fg(Color::White)),
        ]));
    }

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

/// VOLATILITY section with ATR, vVWAP, RV
fn render_volatility_new(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    focused_ticker: &str,
    bvol24h: Option<f64>,
    bar_state: &mut BarState,
) {
    let block = Block::default()
        .title(" VOLATILITY ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(focused_ticker) {
        let atr = t.atr_14.unwrap_or(0.0);
        let price = t.latest_price.unwrap_or(1.0);
        let rv = if atr > 0.0 && price > 0.0 { (atr / price) * 100.0 } else { 0.0 };
        let trend = match t.realized_vol_trend {
            VolTrend::Expanding => "+EXP",
            VolTrend::Contracting => "+CTR",
            VolTrend::Stable => "+STB",
        };

        // ATR line with BVOL24H appended if available
        let mut atr_spans = vec![
            Span::styled("ATR:   ", Style::default().fg(Color::Gray)),
            Span::styled(format!("${:.0}", atr), Style::default().fg(Color::White)),
        ];
        if let Some(bvol) = bvol24h {
            atr_spans.push(Span::raw("   "));
            atr_spans.push(Span::styled("BVOL24H:", Style::default().fg(Color::Gray)));
            atr_spans.push(Span::raw(" "));
            atr_spans.push(Span::styled(format!("{:.2}", bvol), Style::default().fg(Color::Yellow)));
        }
        lines.push(Line::from(atr_spans));

        // tvVWAP shown only if we have >=12 candles; throttle to reduce flicker
        if t.candles_5m_len >= 12 {
            let throttled = bar_state.throttle_tvwav(t.tv_vwap_deviation);
            if let Some(vwap_dev) = throttled {
                let vwap_color = if vwap_dev > 0.0 { Color::Green } else if vwap_dev < 0.0 { Color::Red } else { Color::Gray };
                lines.push(Line::from(vec![
                    Span::styled("tvVWAP:", Style::default().fg(Color::Gray)),
                    Span::raw(" "),
                    Span::styled(format!("{:+.2}%", vwap_dev), Style::default().fg(vwap_color)),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled("tvVWAP:", Style::default().fg(Color::Gray)),
                    Span::raw(" "),
                    Span::styled("--", Style::default().fg(Color::DarkGray)),
                ]));
            }
        } else {
            lines.push(Line::from(vec![
                Span::styled("tvVWAP:", Style::default().fg(Color::Gray)),
                Span::raw(" "),
                Span::styled("warming", Style::default().fg(Color::DarkGray)),
            ]));
        }

        // Realized volatility 30m/1h (match micro TUI)
        let rv30 = t.realized_vol_30m.unwrap_or(0.0);
        let rv1h = t.realized_vol_1h.unwrap_or(0.0);
        lines.push(Line::from(vec![
            Span::styled("RV30/1h:", Style::default().fg(Color::Gray)),
            Span::raw(" "),
            Span::styled(format!("{:.2}%/{:.2}%", rv30, rv1h), Style::default().fg(Color::White)),
            Span::raw(" "),
            Span::styled(trend, Style::default().fg(Color::DarkGray)),
        ]));
    }

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

/// Simplified footer - just ticker hotkeys + quit
fn render_footer_simple(
    f: &mut ratatui::Frame,
    area: Rect,
    focused_ticker: &str,
) {
    let spans = vec![
        Span::raw(" ["),
        Span::styled("B", if focused_ticker == "BTC" { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::DarkGray) }),
        Span::raw("]TC  ["),
        Span::styled("E", if focused_ticker == "ETH" { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::DarkGray) }),
        Span::raw("]TH  ["),
        Span::styled("S", if focused_ticker == "SOL" { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::DarkGray) }),
        Span::raw("]OL"),
        Span::raw("                                                      "),
        Span::raw("["),
        Span::styled("q", Style::default().fg(Color::Red)),
        Span::raw("]uit"),
    ];

    let footer = Paragraph::new(Line::from(spans));
    f.render_widget(footer, area);
}

/// Footer with [B]TC [E]TH [S]OL | OI | RV | LEAD | [q]uit (OLD - unused)
fn render_footer_new(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    focused_ticker: &str,
) {
    let mut spans = vec![
        Span::raw(" ["),
        Span::styled("B", if focused_ticker == "BTC" { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::DarkGray) }),
        Span::raw("]TC ["),
        Span::styled("E", if focused_ticker == "ETH" { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::DarkGray) }),
        Span::raw("]TH ["),
        Span::styled("S", if focused_ticker == "SOL" { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::DarkGray) }),
        Span::raw("]OL"),
    ];

    if let Some(t) = snapshot.tickers.get(focused_ticker) {
        // OI
        let oi_label = if t.oi_velocity > 0.5 { "OI:↑" } else if t.oi_velocity < -0.5 { "OI:↓" } else { "OI:FLAT" };
        spans.push(Span::raw(" | "));
        spans.push(Span::styled(oi_label, Style::default().fg(Color::Cyan)));

        // RV
        if let Some(atr) = t.atr_14 {
            let price = t.latest_price.unwrap_or(1.0);
            let rv = (atr / price) * 100.0;
            let trend = match t.realized_vol_trend {
                VolTrend::Expanding => "+EXP",
                VolTrend::Contracting => "+CTR",
                VolTrend::Stable => "+STB",
            };
            spans.push(Span::raw(" | "));
            spans.push(Span::styled(format!("RV:{:.2}%{}", rv, trend), Style::default().fg(Color::Gray)));
        }

        // LEAD (keys are like "BNC-PERP", "BBT-SPOT", "OKX-PERP")
        let lead = t.exchange_dominance.iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(k, _)| k.clone());
        if let Some(lead_ex) = lead {
            let ex_upper = lead_ex.to_uppercase();
            let short = if ex_upper.starts_with("BNC") { "BNC" }
                else if ex_upper.starts_with("BBT") { "BBT" }
                else if ex_upper.starts_with("OKX") { "OKX" }
                else { "OTH" };
            spans.push(Span::raw(" | "));
            spans.push(Span::styled(format!("LEAD:{}", short), Style::default().fg(Color::Yellow)));
        }
    }

    spans.push(Span::raw(" | ["));
    spans.push(Span::styled("q", Style::default().fg(Color::Red)));
    spans.push(Span::raw("]uit"));

    let footer = Paragraph::new(Line::from(spans));
    f.render_widget(footer, area);
}
