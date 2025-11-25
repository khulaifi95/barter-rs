/// Market Microstructure Dashboard (Opus TUI #1)
///
/// Renders orderflow, basis, liquidation clusters, whales, and CVD signals
/// using the shared aggregation engine so all TUIs share one source of truth.
use std::{
    error::Error,
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};

use barter_trading_tuis::{
    AggregatedSnapshot, Aggregator, BasisState, ConnectionStatus, DivergenceSignal, Side,
    WebSocketClient, WebSocketConfig,
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
            Constraint::Percentage(33),
            Constraint::Percentage(33),
            Constraint::Percentage(34),
        ])
        .split(area);

    let row1 = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(rows[0]);
    let row2 = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(rows[1]);
    let row3 = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(rows[2]);

    render_orderflow_panel(f, row1[0], snapshot);
    render_basis_panel(f, row1[1], snapshot);
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
                    format!(" {:>3.0}% Δ {:>+.1}M/min  ", imb_1m, flow_1m / 1_000_000.0),
                    Style::default().fg(color_1m),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::raw("   5m "),
                Span::styled(bar_5m, Style::default().fg(color_5m)),
                Span::styled(
                    format!(" {:>3.0}% Δ {:>+.1}M/min  {}", imb_5m, flow_5m / 1_000_000.0, trend),
                    Style::default().fg(color_5m),
                ),
            ]));
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

fn render_basis_panel(f: &mut ratatui::Frame, area: Rect, snapshot: &AggregatedSnapshot) {
    let block = Block::default()
        .title(" SPOT vs PERP BASIS ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = Vec::new();

    for ticker in tickers() {
        if let Some(t) = snapshot.tickers.get(ticker) {
            if let Some(basis) = &t.basis {
                let (state_label, state_color) = match basis.state {
                    BasisState::Contango => ("CONTANGO", Color::Yellow),
                    BasisState::Backwardation => ("BACKWRD", Color::Blue),
                    BasisState::Unknown => ("NEUTRAL", Color::Gray),
                };

                let mut label = state_label.to_string();
                if basis.steep {
                    label.push_str(" STEEP");
                }

                // Format absolute basis in USD and percent
                let usd = basis.basis_usd;
                let usd_str = format!("{:+.2}", usd);

                lines.push(Line::from(vec![
                    Span::raw(format!("{:3}  ", ticker)),
                    Span::styled(
                        format!("${} ({:+.2}%) ", usd_str, basis.basis_pct),
                        Style::default().fg(state_color),
                    ),
                    Span::styled(label, Style::default().fg(state_color)),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::raw(format!("{:3}  ", ticker)),
                    Span::styled("N/A", Style::default().fg(Color::DarkGray)),
                ]));
            }
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Waiting for basis data...",
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Cache last rendered lines to avoid flicker; update only on content change
    static BASIS_CACHE: OnceLock<std::sync::Mutex<Option<Vec<Line>>>> = OnceLock::new();
    let cache = BASIS_CACHE.get_or_init(|| std::sync::Mutex::new(None));

    let snapshot_text: Vec<String> = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.clone()).collect::<Vec<_>>().join(""))
        .collect();

    let mut cached = cache.lock().unwrap();
    let update = cached
        .as_ref()
        .map(|old| {
            let old_text: Vec<String> = old
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.clone()).collect::<Vec<_>>().join(""))
                .collect();
            old_text != snapshot_text
        })
        .unwrap_or(true);

    if update {
        *cached = Some(lines.clone());
    }

    let to_render = cached.as_ref().cloned().unwrap_or_else(|| lines.clone());
    let paragraph = Paragraph::new(to_render).block(block).wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
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
                // Keep bars compact so multiple rows fit nicely
                let bar_space = area.width.saturating_sub(36).max(10) as usize;
                let bar_len = ((normalized * bar_space as f64).ceil() as usize).max(1);
                let bar = "█".repeat(bar_len);
                let danger = cluster.total_usd > liq_display_danger_threshold();
                let color = if danger { Color::Red } else { Color::Yellow };

                let usd_display = if cluster.total_usd >= 1_000_000.0 {
                    format!("{:.1}M", cluster.total_usd / 1_000_000.0)
                } else {
                    format!("{:.0}K", cluster.total_usd / 1_000.0)
                };

                lines.push(Line::from(vec![
                    Span::raw(format!("${:.0}  ", cluster.price_level)),
                    Span::styled(bar, Style::default().fg(color)),
                    Span::raw(format!(
                        " ({:>5} | {} L / {} S)",
                        usd_display, cluster.long_count, cluster.short_count
                    )),
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
        .title(" MARKET STATS (5m) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = Vec::new();

    for ticker in tickers() {
        if let Some(t) = snapshot.tickers.get(ticker) {
            // Dominance: take top 3 and show exchange-kind tags
            let mut ex: Vec<_> = t.exchange_dominance.iter().collect();
            ex.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));
            let dom_str = ex
                .iter()
                .take(3)
                .map(|(k, v)| format!("{} {:.0}%", abbreviate_exchange_kind(k), v))
                .collect::<Vec<_>>()
                .join(" | ");

            // Spread proxy
            let spread_str = t
                .latest_spread_pct
                .map(|s| format!("{:.2}%", s))
                .unwrap_or_else(|| "--".to_string());

            // 5m stats
            let vol_display = format!("${:.1}M", t.vol_5m / 1_000_000.0);
            let avg_display = format!("${:.0}K/trade", t.avg_trade_usd_5m / 1_000.0);
            let trades_display = format!("{:.0} t/5m", t.trades_5m as f64);

            // Speed label
            let speed_label = if t.trade_speed >= 8.0 {
                "HIGH"
            } else if t.trade_speed >= 4.0 {
                "MED"
            } else {
                "LOW"
            };

            lines.push(Line::from(Span::styled(
                format!("{:3}:", ticker),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::raw(format!(
                "   {}  {}  {}  {}",
                trades_display, vol_display, avg_display, ""
            ))));
            lines.push(Line::from(Span::raw(format!("   {}", dom_str))));
            lines.push(Line::from(Span::raw(format!(
                "   Speed: {:.1} t/s ({})  Spread: {}",
                t.trade_speed, speed_label, spread_str
            ))));
            lines.push(Line::from(Span::raw("")));
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No market stats available",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
}

fn render_whale_panel(f: &mut ratatui::Frame, area: Rect, snapshot: &AggregatedSnapshot) {
    let threshold_k = whale_threshold_display() / 1_000.0;
    let block = Block::default()
        .title(format!(" WHALE DETECTOR (> ${:.0}K) ", threshold_k))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let available_rows = area.height.saturating_sub(2) as usize;
    let display_limit = available_rows.max(5);

    let mut whales = Vec::new();
    for (ticker, snap) in &snapshot.tickers {
        for w in &snap.whales {
            whales.push((ticker.as_str(), w));
        }
    }

    whales.sort_by(|a, b| b.1.time.cmp(&a.1.time));

    let mut lines = Vec::new();
    for (ticker, whale) in whales.into_iter().take(display_limit) {
        let time_str = whale.time.with_timezone(&chrono::Utc).format("%H:%M:%S");
        let side_color = if whale.side == Side::Buy {
            Color::Green
        } else {
            Color::Red
        };
        let mega = whale.volume_usd >= mega_whale_threshold();

        // Build exchange label from exchange abbreviation + market kind (SPOT/PERP/OTHER)
        let exchange_abbrev = match whale.exchange.as_str() {
            "BinanceFuturesUsd" | "BinanceSpot" => "BNC",
            "BybitPerpetualsUsd" | "BybitSpot" => "BBT",
            "Okx" => "OKX",
            other => other,
        };
        let exch_label = format!("{}-{}", exchange_abbrev, whale.market_kind);

        lines.push(Line::from(vec![
            Span::raw(format!("{} ", time_str)),
            Span::styled(
                format!("{:3} ", ticker),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:4} ", whale.side.as_str().to_uppercase()),
                Style::default().fg(side_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("${:.1}M ", whale.volume_usd / 1_000_000.0),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("@${:.1}K ", whale.price / 1000.0)),
            Span::styled(
                format!("[{}]", exch_label),
                Style::default().fg(Color::Cyan),
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
    let log_debug = std::env::var("LOG_CVD_DEBUG").is_ok();
    let mut binance_lines: Vec<Line> = Vec::new();

    for ticker in tickers() {
        if let Some(t) = snapshot.tickers.get(ticker) {
            // Two horizons: 5m (main) and 1m (fast)
            // Price direction: use tick direction over last minute
            let price_dir = if t.tick_direction.uptick_pct >= 55.0 {
                "↑"
            } else if t.tick_direction.uptick_pct <= 45.0 {
                "↓"
            } else {
                "→"
            };

            // Helper to render one horizon
            let render_line = |label: &str,
                               cvd_total: f64,
                               price_dir: &str,
                               cvd_label: &str,
                               state: (&'static str, Color)| {
                let (cvd_val, cvd_suf) = scale_number(cvd_total, true);
                Line::from(vec![
                    Span::raw(format!("   {}  {:+.2}{} | Price {} CVD {}  ", label, cvd_val, cvd_suf, price_dir, cvd_label)),
                    Span::styled(
                        state.0,
                        Style::default().fg(state.1).add_modifier(Modifier::BOLD),
                    ),
                ])
            };

            // 5m uses stored total_quote (pruned to 5m)
            let cvd_total_5m = t.cvd.total_quote;
            let cvd_dir_5m = if cvd_total_5m > 0.0 {
                "↑"
            } else if cvd_total_5m < 0.0 {
                "↓"
            } else {
                "→"
            };
            let state_5m = divergence_label(divergence_from_dirs(price_dir, cvd_dir_5m));

            // 1m uses rolling CVD total over 60s (from aggregator)
            let cvd_total_1m = t.cvd_1m_total;
            let cvd_dir_1m = if cvd_total_1m > 0.0 {
                "↑"
            } else if cvd_total_1m < 0.0 {
                "↓"
            } else {
                "→"
            };
            let state_1m = divergence_label(divergence_from_dirs(price_dir, cvd_dir_1m));

            if log_debug {
                eprintln!(
                    "[CVD-DEBUG] {} 5m raw={} 1m raw={} price_dir={} cvd_dir5={} cvd_dir1={}",
                    ticker, cvd_total_5m, cvd_total_1m, price_dir, cvd_dir_5m, cvd_dir_1m
                );
            }

            // Header per ticker
            lines.push(Line::from(Span::styled(
                format!("{:3}:", ticker),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(render_line("5m", cvd_total_5m, price_dir, cvd_dir_5m, state_5m));
            lines.push(render_line("1m", cvd_total_1m, price_dir, cvd_dir_1m, state_1m));
            lines.push(Line::from(Span::raw("")));

            // Collect Binance-only detail for bottom section (flexible name match)
            let bin_total: f64 = t
                .cvd_per_exchange_5m
                .iter()
                .filter(|(ex, _)| {
                    let ex_l = ex.to_lowercase();
                    ex_l.contains("binance") && (ex_l.contains("perp") || ex_l.contains("future") || ex_l.contains("futures"))
                })
                .map(|(_, v)| *v)
                .sum();
            // Use 5m price direction to align with 5m CVD for this section
            let price_dir_5m = if t.tick_direction_5m.uptick_pct >= 55.0 {
                "↑"
            } else if t.tick_direction_5m.uptick_pct <= 45.0 {
                "↓"
            } else {
                "→"
            };
            let dir = if bin_total > 0.0 { "↑" } else if bin_total < 0.0 { "↓" } else { "→" };
            let (val, suf) = scale_number(bin_total, true);
            let state = divergence_label(divergence_from_dirs(price_dir_5m, dir));
            binance_lines.push(Line::from(vec![
                Span::styled(
                    format!("{:3}:", ticker),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" {:+.2}{} | Price {} CVD {}", val, suf, price_dir_5m, dir)),
                Span::styled(
                    format!(" {}", state.0),
                    Style::default().fg(state.1).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
    }

    lines.push(Line::from(Span::styled(
        "Binance (5m):",
        Style::default().add_modifier(Modifier::BOLD),
    )));

    // If no Binance entries were produced (e.g., no trades received), show a placeholder
    if binance_lines.is_empty() {
        lines.push(Line::from(Span::raw("   No Binance trades in the last 5m")));
    }

    lines.extend(binance_lines);

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

/// Convert a verbose exchange name into the compact tag used in whales, including market kind.
fn abbreviate_exchange_kind(name: &str) -> String {
    let lower = name.to_lowercase();
    let ex = if lower.contains("binance") {
        "BNC"
    } else if lower.contains("bybit") {
        "BBT"
    } else if lower.contains("okx") {
        "OKX"
    } else {
        name
    };

    let kind = if lower.contains("spot") {
        "SPOT"
    } else if lower.contains("perp") || lower.contains("future") || lower.contains("futures") {
        "PERP"
    } else {
        // Default to PERP for okx when kind not explicit; otherwise OTHER
        if lower.contains("okx") {
            "PERP"
        } else {
            "OTHER"
        }
    };

    format!("{}-{}", ex, kind)
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
