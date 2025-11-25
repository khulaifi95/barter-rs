/// Scalper Mode Dashboard (Opus TUI #4)
///
/// High-frequency execution TUI with 50ms refresh rate.
/// Focus: Delta velocity, imbalance, tape speed for 5s-30s scalping windows.
use std::{
    error::Error,
    io,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use barter_trading_tuis::{
    AggregatedSnapshot, Aggregator, ConnectionStatus, FlowSignal, Side, WebSocketClient,
    WebSocketConfig,
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

/// Available tickers for focus mode
const TICKERS: [&str; 3] = ["BTC", "ETH", "SOL"];

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

            terminal.draw(|f| {
                render_scalper_ui(f, f.area(), &snapshot, connected_now, focused_ticker)
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
) {
    // Layout: Header + Main metrics + Whale tape + Footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Header with price
            Constraint::Length(8),  // Delta velocity panel
            Constraint::Length(6),  // Imbalance + Tape speed
            Constraint::Min(8),     // Whale tape
            Constraint::Length(1),  // Footer
        ])
        .split(area);

    render_header(f, chunks[0], snapshot, connected, focused_ticker);
    render_delta_velocity(f, chunks[1], snapshot, focused_ticker);

    // Split row for imbalance and tape speed
    let metrics_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[2]);

    render_imbalance(f, metrics_row[0], snapshot, focused_ticker);
    render_tape_speed(f, metrics_row[1], snapshot, focused_ticker);
    render_whale_tape(f, chunks[3], snapshot, focused_ticker);
    render_footer(f, chunks[4], focused_ticker);
}

fn render_header(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    connected: bool,
    focused_ticker: &str,
) {
    let (price_str, delta_str) = if let Some(t) = snapshot.tickers.get(focused_ticker) {
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
        (price_fmt, delta_fmt)
    } else {
        ("---".to_string(), "---".to_string())
    };

    let status = if connected { "LIVE" } else { "DISCONNECTED" };
    let status_color = if connected { Color::Green } else { Color::Red };

    let block = Block::default()
        .title(format!(" SCALPER MODE - {} ", focused_ticker))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    let content = Line::from(vec![
        Span::styled(
            format!("Last: {} ", price_str),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(delta_str, Style::default().fg(Color::Yellow)),
        Span::raw("  "),
        Span::styled(format!("[{}]", status), Style::default().fg(status_color)),
    ]);

    let paragraph = Paragraph::new(content).block(block);
    f.render_widget(paragraph, area);
}

fn render_delta_velocity(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    focused_ticker: &str,
) {
    let block = Block::default()
        .title(" DELTA VELOCITY ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = Vec::new();

    if let Some(t) = snapshot.tickers.get(focused_ticker) {
        // Calculate delta for different windows
        // 30s data is available directly, approximate 5s and 15s from it
        let cvd_30s = t.cvd_30s;
        let cvd_1m = t.cvd_1m_total;

        // Approximate shorter windows (we'll use ratios based on 30s)
        let cvd_5s = cvd_30s * 0.17;  // ~5/30
        let cvd_15s = cvd_30s * 0.5;  // ~15/30

        // Calculate velocity ($/second)
        let velocity_5s = cvd_5s / 5.0;
        let velocity_15s = cvd_15s / 15.0;
        let velocity_30s = cvd_30s / 30.0;

        // Determine acceleration
        let acceleration = if velocity_5s > velocity_30s * 1.5 {
            ("ACCELERATING", Color::Green)
        } else if velocity_5s < velocity_30s * 0.5 {
            ("DECELERATING", Color::Red)
        } else {
            ("STEADY", Color::Yellow)
        };

        // Delta display
        let format_delta = |val: f64| -> (String, Color) {
            let (scaled, suffix) = scale_number(val.abs());
            let arrow = if val > 0.0 { "↑" } else if val < 0.0 { "↓" } else { "→" };
            let color = if val > 0.0 { Color::Green } else if val < 0.0 { Color::Red } else { Color::Gray };
            (format!("{:+.1}{} {}", if val > 0.0 { scaled } else { -scaled }, suffix, arrow), color)
        };

        let (d5_str, d5_color) = format_delta(cvd_5s);
        let (d15_str, d15_color) = format_delta(cvd_15s);
        let (d30_str, d30_color) = format_delta(cvd_30s);

        // Velocity line
        lines.push(Line::from(vec![
            Span::styled("      VELOCITY: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:+.1}K/s", velocity_30s / 1000.0),
                Style::default().fg(d30_color).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(acceleration.0, Style::default().fg(acceleration.1)),
        ]));

        lines.push(Line::from(Span::raw("")));

        // Multi-timeframe deltas
        lines.push(Line::from(vec![
            Span::styled("  5s: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{:>12}", d5_str), Style::default().fg(d5_color)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(" 15s: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{:>12}", d15_str), Style::default().fg(d15_color)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(" 30s: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{:>12}", d30_str), Style::default().fg(d30_color)),
        ]));

        // Signal
        let signal = match t.flow_signal {
            FlowSignal::Accumulation => ("ACCUMULATION", Color::Green),
            FlowSignal::Distribution => ("DISTRIBUTION", Color::Red),
            FlowSignal::Exhaustion => ("EXHAUSTION", Color::Yellow),
            FlowSignal::Confirmation => ("CONFIRMATION", Color::Blue),
            FlowSignal::Neutral => ("NEUTRAL", Color::Gray),
        };

        lines.push(Line::from(vec![
            Span::styled("SIGNAL: ", Style::default().fg(Color::DarkGray)),
            Span::styled(signal.0, Style::default().fg(signal.1).add_modifier(Modifier::BOLD)),
        ]));
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

fn render_whale_tape(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &AggregatedSnapshot,
    focused_ticker: &str,
) {
    let threshold_k = whale_threshold() / 1000.0;
    let block = Block::default()
        .title(format!(" WHALE TAPE (>${:.0}K, last 30s) ", threshold_k))
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

                // Exchange abbreviation
                let exch = match whale.exchange.as_str() {
                    "BinanceFuturesUsd" | "BinanceSpot" => "BNC",
                    "BybitPerpetualsUsd" | "BybitSpot" => "BBT",
                    "Okx" => "OKX",
                    other => &other[..3.min(other.len())],
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
                    Span::styled(
                        format!("{} ", vol_str),
                        Style::default().fg(side_color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{:4} ", whale.side.as_str().to_uppercase()),
                        Style::default().fg(side_color),
                    ),
                    Span::raw(format!("{} ", price_str)),
                    Span::styled(format!("{} ", exch), Style::default().fg(Color::Cyan)),
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
        Span::raw("]OL  │  Refresh: 50ms  │  [q] Quit"),
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
