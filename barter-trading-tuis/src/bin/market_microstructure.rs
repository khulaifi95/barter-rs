/// Market Microstructure Dashboard (Opus TUI #1)
///
/// Renders orderflow, basis, liquidation clusters, whales, and CVD signals
/// using the shared aggregation engine so all TUIs share one source of truth.
use std::{
    collections::HashSet,
    error::Error,
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};

use barter_trading_tuis::{
    AggregatedSnapshot, Aggregator, ConnectionStatus, DivergenceSignal,
    FlowSignal, Side, WebSocketClient, WebSocketConfig,
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

/// Get mega whale threshold from MEGA_WHALE_THRESHOLD env var (default: $5,000,000)
fn mega_whale_threshold() -> f64 {
    std::env::var("MEGA_WHALE_THRESHOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5_000_000.0)
}

/// Get liq display danger threshold from LIQ_DISPLAY_DANGER_THRESHOLD env var (default: $1,000,000)
fn liq_display_danger_threshold() -> f64 {
    std::env::var("LIQ_DISPLAY_DANGER_THRESHOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000_000.0)
}

/// Whale threshold for display (shared with aggregator; default $500,000)
fn whale_threshold_display() -> f64 {
    std::env::var("WHALE_THRESHOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(500_000.0)
}

fn whale_floor_display() -> f64 {
    std::env::var("WHALE_FLOOR")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100_000.0)
}

fn whale_multiplier_display() -> f64 {
    std::env::var("WHALE_MULTIPLIER")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10.0)
}

fn mega_whale_threshold_display() -> f64 {
    std::env::var("MEGA_WHALE_THRESHOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(500_000.0)
}

fn tickers() -> &'static [String] {
    TICKERS.get_or_init(get_tickers)
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

    // Backfill tvVWAP and ATR from historical data on startup (silently - no logs to avoid TUI bleed)
    {
        let ticker_list: Vec<&str> = tickers().iter().map(|s| s.as_str()).collect();
        let mut guard = aggregator.lock().await;
        let _ = guard.backfill_all(&ticker_list).await;
    }

    // WebSocket client
    let ws_url = get_ws_url();
    let config = WebSocketConfig::new(ws_url)
        .with_ping_interval(Duration::from_secs(30))
        .with_reconnect_delay(Duration::from_secs(2))
        .with_channel_buffer_size(50_000);
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

    // UI loop
    let mut last_draw = Instant::now();
    let draw_interval = Duration::from_millis(250);

    let result = loop {
        if event::poll(Duration::from_millis(10))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
                    break Ok(());
                }
            }
        }

        if last_draw.elapsed() >= draw_interval {
            let snapshot = {
                let guard = aggregator.lock().await;
                guard.snapshot()
            };

            let connected_now = connected.load(Ordering::Relaxed);
            terminal.draw(|f| render_ui(f, f.area(), &snapshot, connected_now))?;
            last_draw = Instant::now();
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
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

fn render_ui(f: &mut ratatui::Frame, area: Rect, snapshot: &AggregatedSnapshot, connected: bool) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(24), // Top row (reduced from 28%)
            Constraint::Percentage(44), // Middle row - MARKET PULSE needs more space (increased from 38%)
            Constraint::Percentage(32), // Bottom row (reduced from 34%)
        ])
        .split(area);

    // 55/45 split - left panels wider for whale/liq detail, right for compact stats
    let row1 = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(rows[0]);
    let row2 = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(rows[1]);
    let row3 = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(rows[2]);

    render_orderflow_panel(f, row1[0], snapshot);
    render_exchange_intelligence(f, row1[1], snapshot);
    render_liquidation_panel(f, row2[0], snapshot);
    render_market_stats_panel(f, row2[1], snapshot);
    render_whale_panel(f, row3[0], snapshot);
    render_cvd_panel(f, row3[1], snapshot, connected);
}

fn render_orderflow_panel(f: &mut ratatui::Frame, area: Rect, snapshot: &AggregatedSnapshot) {
    let block = Block::default()
        .title(" ORDERFLOW IMBALANCE ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = Vec::new();

    for ticker in tickers() {
        if let Some(t) = snapshot.tickers.get(ticker) {
            // 1m view
            let imb_1m = t.orderflow_1m.imbalance_pct;
            let flow_1m = t.orderflow_1m.net_flow_per_min;
            let bar_1m = format!(
                "[{}{}]",
                "█".repeat(((imb_1m / 10.0).round() as usize).min(10)),
                "░".repeat(10 - ((imb_1m / 10.0).round() as usize).min(10))
            );
            let color_1m = if imb_1m >= 60.0 {
                Color::Green
            } else if imb_1m <= 40.0 {
                Color::Red
            } else {
                Color::Yellow
            };

            // 5m view
            let imb_5m = t.orderflow_5m.imbalance_pct;
            let flow_5m = t.orderflow_5m.net_flow_per_min * 5.0 / 5.0; // per min already
            let bar_5m = format!(
                "[{}{}]",
                "█".repeat(((imb_5m / 10.0).round() as usize).min(10)),
                "░".repeat(10 - ((imb_5m / 10.0).round() as usize).min(10))
            );
            let color_5m = if imb_5m >= 60.0 {
                Color::Green
            } else if imb_5m <= 40.0 {
                Color::Red
            } else {
                Color::Yellow
            };

            // Trend hint
            let trend = if imb_1m > imb_5m + 5.0 {
                "ACCEL"
            } else if imb_1m + 5.0 < imb_5m {
                "FADING"
            } else {
                "STABLE"
            };

            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:3}:", ticker),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::raw("   1m "),
                Span::styled(bar_1m, Style::default().fg(color_1m)),
                Span::styled(
                    format!(" {} ", if imb_1m >= 50.0 { "BUY " } else { "SELL" }),
                    Style::default().fg(if imb_1m >= 50.0 { Color::Green } else { Color::Red }),
                ),
                Span::styled(
                    format!(" {:>3.0}% Δ {:>+.1}M/min  ", imb_1m, flow_1m / 1_000_000.0),
                    Style::default().fg(color_1m),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::raw("   5m "),
                Span::styled(bar_5m, Style::default().fg(color_5m)),
                Span::styled(
                    format!(" {} ", if imb_5m >= 50.0 { "BUY " } else { "SELL" }),
                    Style::default().fg(if imb_5m >= 50.0 { Color::Green } else { Color::Red }),
                ),
                Span::styled(
                    format!(" {:>3.0}% Δ {:>+.1}M/min  {}", imb_5m, flow_5m / 1_000_000.0, trend),
                    Style::default().fg(color_5m),
                ),
            ]));
            // Spacer between tickers
            lines.push(Line::from(Span::raw("")));
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Waiting for trade data...",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
}

fn render_exchange_intelligence(f: &mut ratatui::Frame, area: Rect, snapshot: &AggregatedSnapshot) {
    let block = Block::default()
        .title(" EXCHANGE FLOW (BTC) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = Vec::new();

    // Get BTC ticker for exchange-level data (representative)
    if let Some(t) = snapshot.tickers.get("BTC") {
        // Aggregate OI by abbreviated exchange name
        let mut oi_by_exch: std::collections::HashMap<&str, f64> = std::collections::HashMap::new();

        for (name, val) in &t.oi_per_exchange {
            let abbrev = abbreviate_exchange_for_display(name);
            *oi_by_exch.entry(abbrev).or_insert(0.0) += val;
        }

        let total_oi: f64 = oi_by_exch.values().copied().sum();

        // Get BTC price to convert OI delta from BTC to USD
        let btc_price = t.latest_price.unwrap_or(87000.0);

        // Use the correct total delta (same as MARKET PULSE) and distribute by share
        // This avoids the oi_delta_per_exchange bug where HashMap overwrites on duplicate abbreviated keys
        let total_delta_5m_usd = t.oi_delta_5m * btc_price;
        let total_delta_15m_usd = t.oi_delta_15m * btc_price;

        // OI Section Header with better spacing
        lines.push(Line::from(vec![
            Span::styled("OI Δ ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("(5m/15m)", Style::default().fg(Color::DarkGray)),
        ]));

        // Per-exchange OI with health - cleaner format
        let exchanges = ["BNC", "OKX", "BBT"];
        for exch in exchanges {
            let oi = oi_by_exch.get(exch).copied().unwrap_or(0.0);
            let share = if total_oi > 0.0 { oi / total_oi } else { 0.0 };
            let share_pct = share * 100.0;

            // Calculate per-exchange delta by distributing total delta by OI share
            let delta_5m_usd = total_delta_5m_usd * share;
            let delta_15m_usd = total_delta_15m_usd * share;

            // Color each value independently based on its own sign
            let color_5m = if delta_5m_usd > 0.0 { Color::Green } else if delta_5m_usd < 0.0 { Color::Red } else { Color::Gray };
            let color_15m = if delta_15m_usd > 0.0 { Color::Green } else if delta_15m_usd < 0.0 { Color::Red } else { Color::Gray };
            let arrow = if delta_5m_usd > 0.0 { "↑" } else if delta_5m_usd < 0.0 { "↓" } else { "→" };

            let health = t.exchange_health.get(exch).copied().unwrap_or(0.5);
            let health_color = if health < 1.0 { Color::Green } else if health < 5.0 { Color::Yellow } else { Color::Red };

            lines.push(Line::from(vec![
                Span::styled(format!("  {:3}", exch), Style::default().fg(Color::Cyan)),
                Span::styled(format!(" {:>2.0}%  ", share_pct), Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{:>7}", format_delta(delta_5m_usd)), Style::default().fg(color_5m)),
                Span::styled("/", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{:<7} ", format_delta(delta_15m_usd)), Style::default().fg(color_15m)),
                Span::styled(arrow, Style::default().fg(color_5m)),
                Span::styled(" ●", Style::default().fg(health_color)),
            ]));
        }

        lines.push(Line::from(Span::raw("")));

        // CVD Section Header
        lines.push(Line::from(vec![
            Span::styled("CVD ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("(5m/15m)", Style::default().fg(Color::DarkGray)),
        ]));

        // Per-exchange CVD - aggregate by abbreviated exchange name
        let mut cvd_by_exch_5m: std::collections::HashMap<&str, f64> = std::collections::HashMap::new();
        let mut cvd_by_exch_15m: std::collections::HashMap<&str, f64> = std::collections::HashMap::new();

        for (name, val) in &t.cvd_per_exchange_5m {
            let abbrev = abbreviate_exchange_for_display(name);
            *cvd_by_exch_5m.entry(abbrev).or_insert(0.0) += val;
        }
        for (name, val) in &t.cvd_per_exchange_15m {
            let abbrev = abbreviate_exchange_for_display(name);
            *cvd_by_exch_15m.entry(abbrev).or_insert(0.0) += val;
        }

        // Find leader
        let mut leader_exch = "";
        let mut leader_cvd = 0.0f64;
        for exch in exchanges {
            let cvd_15m = cvd_by_exch_15m.get(exch).copied().unwrap_or(0.0);
            if cvd_15m.abs() > leader_cvd.abs() {
                leader_cvd = cvd_15m;
                leader_exch = exch;
            }
        }

        for exch in exchanges {
            let cvd_5m = cvd_by_exch_5m.get(exch).copied().unwrap_or(0.0);
            let cvd_15m = cvd_by_exch_15m.get(exch).copied().unwrap_or(0.0);

            // Color each value independently based on its own sign
            let color_5m = if cvd_5m > 0.0 { Color::Green } else if cvd_5m < 0.0 { Color::Red } else { Color::Gray };
            let color_15m = if cvd_15m > 0.0 { Color::Green } else if cvd_15m < 0.0 { Color::Red } else { Color::Gray };

            // Arrow based on 5m (short-term direction)
            let arrow = if cvd_5m > 0.0 { "↑" } else if cvd_5m < 0.0 { "↓" } else { "→" };

            let is_leader = exch == leader_exch;

            lines.push(Line::from(vec![
                Span::styled(format!("  {:3}", exch), Style::default().fg(Color::Cyan)),
                Span::styled(format!("       {:>7}", format_delta(cvd_5m)), Style::default().fg(color_5m)),
                Span::styled("/", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{:<7} ", format_delta(cvd_15m)), Style::default().fg(color_15m)),
                Span::styled(arrow, Style::default().fg(color_5m)),
                if is_leader {
                    Span::styled(" ★", Style::default().fg(Color::Yellow))
                } else {
                    Span::raw("  ")
                },
            ]));
        }

        lines.push(Line::from(Span::raw("")));

        // Consensus/Divergence indicator based on 5m CVD (faster signal)
        let bnc_5m = cvd_by_exch_5m.get("BNC").copied().unwrap_or(0.0);
        let okx_5m = cvd_by_exch_5m.get("OKX").copied().unwrap_or(0.0);
        let bbt_5m = cvd_by_exch_5m.get("BBT").copied().unwrap_or(0.0);
        let net_flow = bnc_5m + okx_5m + bbt_5m;

        // Count directions
        let buying_count = [bnc_5m, okx_5m, bbt_5m].iter().filter(|&&x| x > 0.0).count();
        let selling_count = [bnc_5m, okx_5m, bbt_5m].iter().filter(|&&x| x < 0.0).count();

        if buying_count == 3 {
            // All buying - strong bullish consensus
            lines.push(Line::from(vec![
                Span::styled("  ✓ ", Style::default().fg(Color::Green)),
                Span::styled("CONSENSUS ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::styled("All BUYING ", Style::default().fg(Color::White)),
                Span::styled(format_delta(net_flow), Style::default().fg(Color::Green)),
                Span::styled(" (3/3)", Style::default().fg(Color::DarkGray)),
            ]));
        } else if selling_count == 3 {
            // All selling - strong bearish consensus
            lines.push(Line::from(vec![
                Span::styled("  ✓ ", Style::default().fg(Color::Red)),
                Span::styled("CONSENSUS ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                Span::styled("All SELLING ", Style::default().fg(Color::White)),
                Span::styled(format_delta(net_flow), Style::default().fg(Color::Red)),
                Span::styled(" (3/3)", Style::default().fg(Color::DarkGray)),
            ]));
        } else {
            // Mixed signals - divergence
            let mut buyers: Vec<&str> = vec![];
            let mut sellers: Vec<&str> = vec![];
            if bnc_5m > 0.0 { buyers.push("BNC"); } else if bnc_5m < 0.0 { sellers.push("BNC"); }
            if okx_5m > 0.0 { buyers.push("OKX"); } else if okx_5m < 0.0 { sellers.push("OKX"); }
            if bbt_5m > 0.0 { buyers.push("BBT"); } else if bbt_5m < 0.0 { sellers.push("BBT"); }

            let buyer_str = if buyers.is_empty() { "none".to_string() } else { buyers.join("/") };
            let seller_str = if sellers.is_empty() { "none".to_string() } else { sellers.join("/") };

            lines.push(Line::from(vec![
                Span::styled("  ⚠ ", Style::default().fg(Color::Yellow)),
                Span::styled("MIXED ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{} ", buyer_str), Style::default().fg(Color::Green)),
                Span::styled("vs ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{} ", seller_str), Style::default().fg(Color::Red)),
                Span::styled("Net:", Style::default().fg(Color::DarkGray)),
                Span::styled(format_delta(net_flow), Style::default().fg(if net_flow > 0.0 { Color::Green } else { Color::Red })),
            ]));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "Waiting for exchange data...",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

/// Abbreviate exchange name for display
fn abbreviate_exchange_for_display(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.contains("binance") {
        "BNC"
    } else if lower.contains("bybit") {
        "BBT"
    } else if lower.contains("okx") {
        "OKX"
    } else {
        "OTH"
    }
}

/// Format delta value with K/M suffix
fn format_delta(value: f64) -> String {
    if value.abs() >= 1_000_000.0 {
        format!("{:+.1}M", value / 1_000_000.0)
    } else if value.abs() >= 1_000.0 {
        format!("{:+.1}K", value / 1_000.0)
    } else {
        format!("{:+.0}", value)
    }
}

fn render_liquidation_panel(f: &mut ratatui::Frame, area: Rect, snapshot: &AggregatedSnapshot) {
    let block = Block::default()
        .title(" LIQUIDATION CLUSTERS ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = Vec::new();
    let max_rows = area.height.saturating_sub(3) as usize;

    for ticker in tickers() {
        if let Some(t) = snapshot.tickers.get(ticker) {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}: ", ticker),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        "rate {:.1}/min  bucket ${}  window 10m",
                        t.liq_rate_per_min, t.liq_bucket as i64
                    ),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));

            if t.liquidations.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  No liquidations detected",
                    Style::default().fg(Color::DarkGray),
                )));
                continue;
            }

            let max_volume = t
                .liquidations
                .iter()
                .map(|c| c.total_usd)
                .fold(0.0_f64, f64::max)
                .max(1.0);

            for cluster in t.liquidations.iter().take(max_rows.saturating_sub(1)) {
                let normalized = (cluster.total_usd / max_volume).clamp(0.0, 1.0);
                let bar_space = area.width.saturating_sub(36).max(10) as usize;
                let bar_len = ((normalized * bar_space as f64).ceil() as usize).max(1);
                let bar = "█".repeat(bar_len);
                let danger = cluster.total_usd > liq_display_danger_threshold();
                let bar_color = if danger { Color::Red } else { Color::Yellow };

                let usd_display = if cluster.total_usd >= 1_000_000.0 {
                    format!("{:.1}M", cluster.total_usd / 1_000_000.0)
                } else if cluster.total_usd >= 1_000.0 {
                    format!("{:.0}K", cluster.total_usd / 1_000.0)
                } else {
                    "<1K".to_string()
                };

                lines.push(Line::from(vec![
                    Span::raw(format!("${:<6.0} ", cluster.price_level)),
                    Span::styled(bar, Style::default().fg(bar_color)),
                    Span::raw(format!(" {:>5} ", usd_display)),
                    Span::styled(format!("{}L", cluster.long_count), Style::default().fg(Color::Green)),
                    Span::raw("/"),
                    Span::styled(format!("{}S", cluster.short_count), Style::default().fg(Color::Red)),
                    if danger {
                        Span::styled(
                            " DANGER",
                            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                        )
                    } else {
                        Span::raw("")
                    },
                ]));
            }
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No liquidations detected",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
}

fn render_market_stats_panel(f: &mut ratatui::Frame, area: Rect, snapshot: &AggregatedSnapshot) {
    let block = Block::default()
        .title(" MARKET PULSE ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = Vec::new();

    for ticker in tickers() {
        if let Some(t) = snapshot.tickers.get(ticker) {
            // Ticker header with price and health indicator
            let price_str = t
                .latest_price
                .map(|p| {
                    if p >= 1000.0 {
                        format!("${:.1}K", p / 1000.0)
                    } else {
                        format!("${:.2}", p)
                    }
                })
                .unwrap_or_else(|| "---".to_string());

            // Health indicator based on exchange health
            let all_healthy = t.exchange_health.values().all(|&h| h < 2.0);
            let health_indicator = if all_healthy || t.exchange_health.is_empty() {
                Span::styled("● ", Style::default().fg(Color::Green))
            } else {
                Span::styled("● ", Style::default().fg(Color::Yellow))
            };

            lines.push(Line::from(vec![
                Span::styled(
                    format!("{} ", ticker),
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("{:<8} ", price_str)),
                // Speed indicator
                Span::styled(
                    format!("{:.0}t/s ", t.trade_speed),
                    if t.trade_speed >= 100.0 {
                        Style::default().fg(Color::Red)
                    } else if t.trade_speed >= 50.0 {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
                health_indicator,
            ]));

            // Multi-timeframe volume: 30s | 1m | 5m
            let vol_30s = t.orderflow_30s.buy_usd + t.orderflow_30s.sell_usd;
            let vol_1m = t.orderflow_1m.buy_usd + t.orderflow_1m.sell_usd;
            let (v30, s30) = scale_number(vol_30s, true);
            let (v1m, s1m) = scale_number(vol_1m, true);
            let (v5m, s5m) = scale_number(t.vol_5m, true);

            lines.push(Line::from(vec![
                Span::styled(" Vol: ", Style::default().fg(Color::DarkGray)),
                Span::raw(format!("{:.0}{}(30s) ", v30, s30)),
                Span::raw(format!("{:.0}{}(1m) ", v1m, s1m)),
                Span::raw(format!("{:.0}{}(5m)", v5m, s5m)),
            ]));

            // OI with per-exchange breakdown - convert from contracts to USD
            let ticker_price = t.latest_price.unwrap_or(1.0);
            let oi_delta_usd = t.oi_delta_5m * ticker_price;
            let (oi_arrow, oi_color) = if oi_delta_usd > 100_000.0 {
                ("↑", Color::Green)
            } else if oi_delta_usd < -100_000.0 {
                ("↓", Color::Red)
            } else {
                ("→", Color::Gray)
            };

            // Build per-exchange OI breakdown string - convert to USD
            let bnc_delta_usd = t.oi_delta_per_exchange_5m.get("BNC").copied().unwrap_or(0.0) * ticker_price;
            let okx_delta_usd = t.oi_delta_per_exchange_5m.get("OKX").copied().unwrap_or(0.0) * ticker_price;
            let bbt_delta_usd = t.oi_delta_per_exchange_5m.get("BBT").copied().unwrap_or(0.0) * ticker_price;

            let oi_breakdown = format!(
                "BNC{} OKX{} BBT{}",
                format_delta_short(bnc_delta_usd),
                format_delta_short(okx_delta_usd),
                format_delta_short(bbt_delta_usd)
            );

            lines.push(Line::from(vec![
                Span::styled(" OI: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{}{} ", format_delta(oi_delta_usd), oi_arrow),
                    Style::default().fg(oi_color),
                ),
                Span::styled(
                    format!("({})", oi_breakdown),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));

            // VWAP line: daily | session [SESSION]
            let vwap_daily_str = t.vwap_daily
                .map(|v| format!("${:.2}", v))
                .unwrap_or_else(|| "---".to_string());
            let vwap_session_str = t.vwap_session
                .map(|v| format!("${:.2}", v))
                .unwrap_or_else(|| "---".to_string());
            let session_label = t.current_session
                .map(|s| s.label())
                .unwrap_or("---");
            let dev_str = t.vwap_daily_deviation
                .map(|d| format!("({:+.2}%)", d))
                .unwrap_or_default();
            let dev_color = t.vwap_daily_deviation
                .map(|d| if d > 0.0 { Color::Green } else if d < 0.0 { Color::Red } else { Color::Gray })
                .unwrap_or(Color::Gray);

            lines.push(Line::from(vec![
                Span::styled(" dVWAP: ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{} ", vwap_daily_str), Style::default().fg(Color::White)),
                Span::styled(dev_str, Style::default().fg(dev_color)),
                Span::styled(" sVWAP: ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{} ", vwap_session_str), Style::default().fg(Color::Cyan)),
                Span::styled(format!("[{}]", session_label), Style::default().fg(Color::DarkGray)),
            ]));

            // tvVWAP line (TradingView style - HLC3 on 5m candles)
            let tv_vwap_str = t.tv_vwap
                .map(|v| format!("${:.2}", v))
                .unwrap_or_else(|| "---".to_string());
            let tv_dev_str = t.tv_vwap_deviation
                .map(|d| format!("({:+.2}%)", d))
                .unwrap_or_default();
            let tv_dev_color = t.tv_vwap_deviation
                .map(|d| if d > 0.0 { Color::Green } else if d < 0.0 { Color::Red } else { Color::Gray })
                .unwrap_or(Color::Gray);

            if t.candles_5m_len >= 12 {
                lines.push(Line::from(vec![
                    Span::styled(" tvVWAP: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(format!("{} ", tv_vwap_str), Style::default().fg(Color::Yellow)),
                    Span::styled(tv_dev_str, Style::default().fg(tv_dev_color)),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(" tvVWAP: ", Style::default().fg(Color::DarkGray)),
                    Span::styled("warming", Style::default().fg(Color::DarkGray)),
                ]));
            }

            // ATR + Volatility line (compact)
            let atr_str = t.atr_14
                .map(|a| format!("${:.2}", a))
                .unwrap_or_else(|| "---".to_string());
            let atr_pct_str = t.atr_14_pct
                .map(|p| format!("({:.2}%)", p))
                .unwrap_or_default();

            // Realized volatility (30m/1h)
            let rv_30m_str = t.realized_vol_30m
                .map(|v| format!("{:.2}%", v))
                .unwrap_or_else(|| "---".to_string());
            let rv_1h_str = t.realized_vol_1h
                .map(|v| format!("{:.2}%", v))
                .unwrap_or_else(|| "---".to_string());
            let rv_arrow = t.realized_vol_trend.arrow();
            let rv_label = t.realized_vol_trend.label();
            let rv_color = match t.realized_vol_trend {
                barter_trading_tuis::VolTrend::Expanding => Color::Red,
                barter_trading_tuis::VolTrend::Contracting => Color::Green,
                barter_trading_tuis::VolTrend::Stable => Color::Yellow,
            };

            lines.push(Line::from(vec![
                Span::styled(" ATR: ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{} ", atr_str), Style::default().fg(Color::Magenta)),
                Span::styled(format!("{} ", atr_pct_str), Style::default().fg(Color::DarkGray)),
                Span::styled(" RV: ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{}/{}{} ", rv_30m_str, rv_1h_str, rv_arrow), Style::default().fg(rv_color)),
                Span::styled(rv_label, Style::default().fg(rv_color)),
            ]));

            // L2 Book Imbalance line (compact: "Book: 63% BID | BNC:65% BBT:58%")
            let agg_imb = t.aggregated_book_imbalance;
            let (book_label, book_color) = if agg_imb > 55.0 {
                ("BID", Color::Green)
            } else if agg_imb < 45.0 {
                ("ASK", Color::Red)
            } else {
                ("BAL", Color::Yellow)
            };

            let bnc_imb = t.per_exchange_book_imbalance.get("BNC").copied().unwrap_or(50.0);
            let bbt_imb = t.per_exchange_book_imbalance.get("BBT").copied().unwrap_or(50.0);

            // Only show if we have book data
            if !t.per_exchange_book_imbalance.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled(" Book: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{:.0}% {} ", agg_imb, book_label),
                        Style::default().fg(book_color),
                    ),
                    Span::styled(
                        format!("BNC:{:.0}% BBT:{:.0}%", bnc_imb, bbt_imb),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }

            // Spacer between tickers for readability
            lines.push(Line::from(Span::raw("")));
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Waiting for data...",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

/// Format delta with short format for inline display
fn format_delta_short(value: f64) -> String {
    if value.abs() >= 1_000_000.0 {
        format!("{:+.0}M", value / 1_000_000.0)
    } else if value.abs() >= 1_000.0 {
        format!("{:+.0}K", value / 1_000.0)
    } else if value.abs() >= 1.0 {
        format!("{:+.0}", value)
    } else {
        "0".to_string()
    }
}

#[derive(Debug)]
struct AggWhale {
    time: chrono::DateTime<chrono::Utc>,
    ticker: String,
    side: Side,
    market_kind: String,
    volume_usd: f64,
    price: f64,
    exchanges: HashSet<String>,
    count: usize,
    is_mega: bool,
}

fn current_whale_threshold_hint(snapshot: &AggregatedSnapshot) -> f64 {
    let mut min_thr = f64::MAX;
    let mut found = false;
    for (ticker, snap) in &snapshot.tickers {
        let avg = snap.avg_trade_usd_5m;
        if avg > 0.0 {
            let thr = (avg * whale_multiplier_display()).max(whale_floor_display());
            if thr < min_thr {
                min_thr = thr;
            }
            found = true;
        }
    }
    if found {
        min_thr.max(whale_threshold_display())
    } else {
        whale_threshold_display()
    }
}

fn render_whale_panel(f: &mut ratatui::Frame, area: Rect, snapshot: &AggregatedSnapshot) {
    let mut block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Cyan));

    let available_rows = area.height.saturating_sub(2) as usize;
    let display_limit = available_rows.max(5);

    // Collect whales across tickers
    let mut whales = Vec::new();
    for (ticker, snap) in &snapshot.tickers {
        for w in &snap.whales {
            whales.push((ticker.as_str(), w));
        }
    }

    // Sort newest first
    whales.sort_by(|a, b| b.1.time.cmp(&a.1.time));

    // Aggregate within a short window by ticker+side+market_kind (except mega whales)
    let window_secs: i64 = std::env::var("WHALE_AGG_WINDOW_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let mega_threshold = mega_whale_threshold_display();
    let mut buckets: std::collections::HashMap<(String, String, String, i64), AggWhale> =
        std::collections::HashMap::new();

    for (ticker, whale) in whales {
        let is_mega = whale.volume_usd >= mega_threshold;
        if is_mega {
            let mut exchanges = HashSet::new();
            exchanges.insert(abbreviate_exchange_for_display(&whale.exchange).to_string());
            buckets.insert(
                (
                    ticker.to_string(),
                    whale.side.as_str().to_string(),
                    whale.market_kind.clone(),
                    whale.time.timestamp(), // unique key to avoid merge
                ),
                AggWhale {
                    time: whale.time,
                    ticker: ticker.to_string(),
                    side: whale.side.clone(),
                    market_kind: whale.market_kind.clone(),
                    volume_usd: whale.volume_usd,
                    price: whale.price,
                    exchanges,
                    count: 1,
                    is_mega: true,
                },
            );
            continue;
        }

        let bucket = whale.time.timestamp() / window_secs;
        let key = (
            ticker.to_string(),
            whale.side.as_str().to_string(),
            whale.market_kind.clone(),
            bucket,
        );

        let entry = buckets.entry(key).or_insert_with(|| {
            let mut exchanges = HashSet::new();
            exchanges.insert(abbreviate_exchange_for_display(&whale.exchange).to_string());
            AggWhale {
                time: whale.time,
                ticker: ticker.to_string(),
                side: whale.side.clone(),
                market_kind: whale.market_kind.clone(),
                volume_usd: 0.0,
                price: whale.price,
                exchanges,
                count: 0,
                is_mega: false,
            }
        });

        entry.volume_usd += whale.volume_usd;
        entry.count += 1;
        entry.time = entry.time.max(whale.time);
        entry.price = whale.price; // last price in bucket
        entry
            .exchanges
            .insert(abbreviate_exchange_for_display(&whale.exchange).to_string());
    }

    let mut aggregated: Vec<AggWhale> = buckets
        .into_iter()
        .map(|(_, mut v)| {
            // Promote to mega if aggregated volume crosses threshold
            if v.volume_usd >= mega_threshold {
                v.is_mega = true;
            }
            v
        })
        .collect();

    // Stable ordering: newest first, then larger volume
    aggregated.sort_by(|a, b| {
        b.time
            .cmp(&a.time)
            .then_with(|| b.volume_usd.partial_cmp(&a.volume_usd).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut lines = Vec::new();
    let threshold = current_whale_threshold_hint(snapshot);
    let threshold_k = threshold / 1_000.0;
    // Show floor explicitly to avoid mismatch with per-ticker adaptive thresholds
    let header = format!(" WHALE DETECTOR (adaptive ≥ ${:.0}K floor) ", threshold_k);
    block = block.title(header);

    for agg in aggregated.into_iter().take(display_limit) {
        let time_str = agg.time.with_timezone(&chrono::Utc).format("%H:%M:%S");
        let side_color = if agg.side == Side::Buy {
            Color::Green
        } else {
            Color::Red
        };
        let mega = agg.is_mega;

        let exch_list: Vec<String> = agg.exchanges.iter().cloned().collect();
        let exch_label = format!("[{}]", exch_list.join("/"));

        let count_label = if agg.count > 1 {
            format!(" (x{})", agg.count)
        } else {
            "".to_string()
        };

        lines.push(Line::from(vec![
            Span::raw(format!("{} ", time_str)),
            Span::styled(
                format!("{:3} ", agg.ticker),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:4} ", agg.side.as_str().to_uppercase()),
                Style::default().fg(side_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("${:.1}M", agg.volume_usd / 1_000_000.0),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(count_label),
            Span::raw(" "),
            Span::raw(format!("@${:.1}K ", agg.price / 1000.0)),
            Span::styled(exch_label, Style::default().fg(Color::Cyan)),
            Span::styled(
                format!(" ({})", agg.market_kind),
                Style::default().fg(Color::DarkGray),
            ),
            if mega {
                Span::styled(" ⚠️", Style::default().fg(Color::Red))
            } else {
                Span::raw("")
            },
        ]));
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No whale trades detected",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
}

fn render_cvd_panel(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    connected: bool,
) {
    let block = Block::default()
        .title(" CVD DIVERGENCE ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = Vec::new();

    for ticker in tickers() {
        if let Some(t) = snapshot.tickers.get(ticker) {
            // Get CVD values for all timeframes
            let cvd_1m = t.cvd_1m_total;
            let cvd_5m = t.cvd_5m_total;
            let cvd_15m = t.cvd_15m_total;
            let cvd_30s = t.cvd_30s;

            // Velocity values
            let vel_1m = t.cvd_velocity_1m;
            let vel_5m = t.cvd_velocity_5m;
            let vel_15m = t.cvd_velocity_15m;

            // Price direction for divergence detection
            let price_dir_1m = if t.tick_direction.uptick_pct >= 55.0 {
                "↑"
            } else if t.tick_direction.uptick_pct <= 45.0 {
                "↓"
            } else {
                "→"
            };

            // Flow signal from state (already computed)
            let flow_signal = &t.flow_signal;

            // Header per ticker
            lines.push(Line::from(Span::styled(
                format!("{}:", ticker),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )));

            // 1m line with velocity and flow signal
            let cvd_1m_dir = if cvd_1m > 0.0 { "↑" } else if cvd_1m < 0.0 { "↓" } else { "→" };
            let vel_1m_label = velocity_label(vel_1m, ticker);

            // Use 30s to detect early turning signal
            let signal_30s_opposing = (cvd_30s > 0.0 && cvd_1m < 0.0) || (cvd_30s < 0.0 && cvd_1m > 0.0);
            let signal_30s_strong = cvd_30s.abs() > cvd_threshold(ticker) / 2.0;

            let (flow_label, flow_color) = if signal_30s_opposing && signal_30s_strong {
                ("⚠ TURNING", Color::Yellow)
            } else {
                flow_signal_label(flow_signal)
            };

            let cvd_1m_color = if cvd_1m > 0.0 { Color::Green } else if cvd_1m < 0.0 { Color::Red } else { Color::Gray };

            lines.push(Line::from(vec![
                Span::styled("  1m:  ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:<8} {} ", format_delta(cvd_1m), cvd_1m_dir),
                    Style::default().fg(cvd_1m_color),
                ),
                Span::styled(
                    format!("{:<6} │ ", vel_1m_label),
                    Style::default().fg(velocity_color(vel_1m)),
                ),
                Span::styled(flow_label, Style::default().fg(flow_color)),
            ]));

            // 5m line with divergence signal
            let cvd_5m_dir = if cvd_5m > 0.0 { "↑" } else if cvd_5m < 0.0 { "↓" } else { "→" };
            let vel_5m_label = velocity_label(vel_5m, ticker);
            let divergence_5m = divergence_from_dirs(price_dir_1m, cvd_5m_dir);
            let (div_label, div_color) = divergence_label(divergence_5m);

            let cvd_5m_color = if cvd_5m > 0.0 { Color::Green } else if cvd_5m < 0.0 { Color::Red } else { Color::Gray };

            lines.push(Line::from(vec![
                Span::styled("  5m:  ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:<8} {} ", format_delta(cvd_5m), cvd_5m_dir),
                    Style::default().fg(cvd_5m_color),
                ),
                Span::styled(
                    format!("{:<6} │ ", vel_5m_label),
                    Style::default().fg(velocity_color(vel_5m)),
                ),
                Span::styled(div_label, Style::default().fg(div_color)),
            ]));

            // 15m line with velocity per minute
            let cvd_15m_dir = if cvd_15m > 0.0 { "↑" } else if cvd_15m < 0.0 { "↓" } else { "→" };
            let cvd_15m_color = if cvd_15m > 0.0 { Color::Green } else if cvd_15m < 0.0 { Color::Red } else { Color::Gray };

            lines.push(Line::from(vec![
                Span::styled("  15m: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:<8} {} ", format_delta(cvd_15m), cvd_15m_dir),
                    Style::default().fg(cvd_15m_color),
                ),
                Span::styled("       │ ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("Vel: {}/min", format_delta(vel_15m)),
                    Style::default().fg(velocity_color(vel_15m)),
                ),
            ]));

            // Spacer between tickers
            lines.push(Line::from(Span::raw("")));
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            if connected {
                "Waiting for CVD data..."
            } else {
                "Disconnected"
            },
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}

/// Get CVD threshold for a ticker (used for 30s signal detection)
fn cvd_threshold(ticker: &str) -> f64 {
    match ticker {
        "BTC" => 10_000_000.0,  // $10M for BTC
        "ETH" => 5_000_000.0,   // $5M for ETH
        _ => 1_000_000.0,       // $1M for others
    }
}

/// Velocity label based on rate of change
fn velocity_label(velocity: f64, ticker: &str) -> &'static str {
    let threshold = match ticker {
        "BTC" => 1_000_000.0,
        "ETH" => 500_000.0,
        _ => 100_000.0,
    };
    if velocity.abs() > threshold * 2.0 {
        if velocity > 0.0 { "ACCEL" } else { "DECEL" }
    } else if velocity.abs() > threshold {
        "STEADY"
    } else {
        "FLAT"
    }
}

/// Velocity color
fn velocity_color(velocity: f64) -> Color {
    if velocity > 0.0 {
        Color::Green
    } else if velocity < 0.0 {
        Color::Red
    } else {
        Color::Gray
    }
}

/// Flow signal label and color
fn flow_signal_label(signal: &FlowSignal) -> (&'static str, Color) {
    match signal {
        FlowSignal::Accumulation => ("ACCUMULATION", Color::Green),
        FlowSignal::Distribution => ("DISTRIBUTION", Color::Red),
        FlowSignal::Exhaustion => ("EXHAUSTION", Color::Yellow),
        FlowSignal::Confirmation => ("CONFIRMATION", Color::Blue),
        FlowSignal::Neutral => ("NEUTRAL", Color::Gray),
    }
}

fn divergence_label(signal: DivergenceSignal) -> (&'static str, Color) {
    match signal {
        DivergenceSignal::Bullish => ("BULLISH", Color::Green),
        DivergenceSignal::Bearish => ("BEARISH", Color::Red),
        DivergenceSignal::Aligned => ("ALIGNED", Color::Blue),
        DivergenceSignal::Neutral => ("NEUTRAL", Color::Yellow),
        DivergenceSignal::Unknown => ("---", Color::Gray),
    }
}

fn divergence_from_dirs(price_dir: &str, cvd_dir: &str) -> DivergenceSignal {
    match (price_dir, cvd_dir) {
        ("↑", "↑") => DivergenceSignal::Bullish,
        ("↓", "↓") => DivergenceSignal::Bearish,
        ("↑", "↓") => DivergenceSignal::Bearish,
        ("↓", "↑") => DivergenceSignal::Bullish,
        _ => DivergenceSignal::Neutral,
    }
}

/// Scale large numbers into a compact value + suffix (K/M/B/T)
fn scale_number(v: f64, clamp_to_b: bool) -> (f64, &'static str) {
    let abs = v.abs();
    if !clamp_to_b && abs >= 1_000_000_000_000.0 {
        (v / 1_000_000_000_000.0, "T")
    } else if abs >= 1_000_000_000.0 {
        (v / 1_000_000_000.0, "B")
    } else if abs >= 1_000_000.0 {
        (v / 1_000_000.0, "M")
    } else if abs >= 1_000.0 {
        (v / 1_000.0, "K")
    } else {
        (v, "")
    }
}
