/// Institutional Flow Monitor (Opus TUI #2)
///
/// Uses the shared aggregation engine to display smart money net flow,
/// aggressor ratios, exchange dominance, depth imbalance, and momentum signals.
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
    AggregatedSnapshot, Aggregator, ConnectionStatus, WebSocketClient, WebSocketConfig,
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
    widgets::{Block, Borders, Gauge, Paragraph, Wrap},
    Frame, Terminal,
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

/// Get strong flow threshold from STRONG_FLOW_THRESHOLD env var (default: $1,000,000)
fn strong_flow_threshold() -> f64 {
    std::env::var("STRONG_FLOW_THRESHOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000_000.0)
}

/// Get weak flow threshold from WEAK_FLOW_THRESHOLD env var (default: $100,000)
fn weak_flow_threshold() -> f64 {
    std::env::var("WEAK_FLOW_THRESHOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100_000.0)
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

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let aggregator = Arc::new(Mutex::new(Aggregator::new()));
    let connected = Arc::new(AtomicBool::new(false));

    // Backfill tvVWAP and ATR from historical data on startup (silently)
    {
        let ticker_list: Vec<&str> = tickers().iter().map(|s| s.as_str()).collect();
        let mut guard = aggregator.lock().await;
        let _ = guard.backfill_all(&ticker_list).await;
    }

    let ws_url = get_ws_url();
    let client =
        WebSocketClient::with_config(WebSocketConfig::new(ws_url).with_channel_buffer_size(50_000));
    let (mut event_rx, mut status_rx) = client.start();

    {
        let agg = Arc::clone(&aggregator);
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
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

    let mut last_render = Instant::now();
    let render_interval = Duration::from_secs(1);

    loop {
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
                    break;
                }
            }
        }

        if last_render.elapsed() >= render_interval {
            let snapshot = {
                let guard = aggregator.lock().await;
                guard.snapshot()
            };
            let is_connected = connected.load(Ordering::Relaxed);
            terminal.draw(|f| ui(f, &snapshot, is_connected))?;
            last_render = Instant::now();
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

fn ui(f: &mut Frame, snapshot: &AggregatedSnapshot, connected: bool) {
    let size = f.area();
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Min(0),
        ])
        .split(size);

    render_smart_money_tracker(f, snapshot, main_chunks[0]);
    render_orderbook_depth_imbalance(f, snapshot, main_chunks[1]);
    render_momentum_signals(f, snapshot, main_chunks[2]);
    render_footer(f, connected, main_chunks[3]);
}

fn render_smart_money_tracker(f: &mut Frame, snapshot: &AggregatedSnapshot, area: Rect) {
    let block = Block::default()
        .title("SMART MONEY TRACKER")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(33),
            Constraint::Percentage(34),
        ])
        .split(inner);

    render_net_flow_panel(f, snapshot, chunks[0]);
    render_aggressor_panel(f, snapshot, chunks[1]);
    render_exchange_dominance_panel(f, snapshot, chunks[2]);
}

fn render_net_flow_panel(f: &mut Frame, snapshot: &AggregatedSnapshot, area: Rect) {
    let block = Block::default()
        .title("NET FLOW (5m)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green));

    let mut lines = Vec::new();
    let strong = strong_flow_threshold();
    let weak = weak_flow_threshold();

    for ticker in tickers() {
        if let Some(t) = snapshot.tickers.get(ticker) {
            let flow = t.orderflow_5m.net_flow_per_min * 5.0;
            let trend = if flow > strong {
                "↑↑"
            } else if flow > weak {
                "↑"
            } else if flow < -strong {
                "↓↓"
            } else if flow < -weak {
                "↓"
            } else {
                "→"
            };
            let color = if flow >= 0.0 {
                Color::Green
            } else {
                Color::Red
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:>3}: ", ticker),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!("{:>+7.1}M ", flow / 1_000_000.0),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(trend, Style::default().fg(color)),
            ]));
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Waiting for flow data...",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
}

fn render_aggressor_panel(f: &mut Frame, snapshot: &AggregatedSnapshot, area: Rect) {
    let block = Block::default()
        .title("AGGRESSOR (1m)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let mut total_buy = 0.0;
    let mut total_sell = 0.0;

    for ticker in tickers() {
        if let Some(t) = snapshot.tickers.get(ticker) {
            total_buy += t.orderflow_1m.buy_usd;
            total_sell += t.orderflow_1m.sell_usd;
        }
    }

    let buy_pct = if total_buy + total_sell > 0.0 {
        (total_buy / (total_buy + total_sell)) * 100.0
    } else {
        50.0
    };
    let sell_pct = 100.0 - buy_pct;
    let ratio = if total_sell > 0.0 {
        total_buy / total_sell
    } else if total_buy > 0.0 {
        999.9
    } else {
        1.0
    };

    let lines = vec![
        Line::from(vec![
            Span::raw("BUY:  "),
            Span::styled(
                format!("{:>5.1}%", buy_pct),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("SELL: "),
            Span::styled(
                format!("{:>5.1}%", sell_pct),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("Ratio: "),
            Span::styled(format!("{:.1}:1", ratio), Style::default().fg(Color::Cyan)),
        ]),
    ];

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

fn render_exchange_dominance_panel(f: &mut Frame, snapshot: &AggregatedSnapshot, area: Rect) {
    let block = Block::default()
        .title("EXCHANGE DOMINANCE (last 60s)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    // Aggregate raw volume per exchange across all tickers, then recalculate %
    let mut volume_by_exchange: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    let mut total_volume = 0.0;

    for t in snapshot.tickers.values() {
        let ticker_total = t.vol_5m.max(1.0); // Use 5m volume as proxy
        for (ex, pct) in &t.exchange_dominance {
            // Convert % back to approximate volume
            let ex_vol = ticker_total * (*pct / 100.0);
            *volume_by_exchange.entry(ex.clone()).or_insert(0.0) += ex_vol;
            total_volume += ex_vol;
        }
    }

    // Convert back to percentages (now they'll sum to ~100%) and ensure BNC/BBT/OKX are visible if present
    let mut exchanges: Vec<(String, f64)> = volume_by_exchange
        .iter()
        .map(|(ex, vol)| {
            let pct = if total_volume > 0.0 {
                (vol / total_volume) * 100.0
            } else {
                0.0
            };
            (ex.clone(), pct)
        })
        .collect();
    exchanges.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    // Reorder to always show known exchanges first if they exist
    let mut prioritized: Vec<(String, f64)> = Vec::new();
    for key in ["binance", "bybit", "okx"] {
        if let Some(pos) = exchanges.iter().position(|(ex, _)| ex.to_lowercase().contains(key)) {
            prioritized.push(exchanges[pos].clone());
        }
    }
    for item in exchanges.iter() {
        if !prioritized.iter().any(|(ex, _)| ex == &item.0) {
            prioritized.push(item.clone());
        }
    }

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut y = 0;
    for (exchange, pct) in exchanges.iter().take(inner.height as usize) {
        let bar_area = Rect::new(inner.x, inner.y + y, inner.width, 1);
        // Abbreviate exchange name
        let ex_abbr = if exchange.to_lowercase().contains("binance") {
            "Binance"
        } else if exchange.to_lowercase().contains("bybit") {
            "Bybit"
        } else if exchange.to_lowercase().contains("okx") {
            "OKX"
        } else {
            exchange.as_str()
        };
        let label = format!("{:>10}: {:>5.1}%", ex_abbr, pct);
        let gauge = Gauge::default()
            .gauge_style(Style::default().fg(Color::Blue).bg(Color::Black))
            .ratio((pct / 100.0).clamp(0.0, 1.0))
            .label(label);
        f.render_widget(gauge, bar_area);
        y += 1;
    }
}

fn render_orderbook_depth_imbalance(f: &mut Frame, snapshot: &AggregatedSnapshot, area: Rect) {
    let block = Block::default()
        .title("ORDERBOOK DEPTH IMBALANCE (L1 proxy)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = Vec::new();
    for ticker in tickers() {
        if let Some(t) = snapshot.tickers.get(ticker) {
            if let (Some((bid, bid_size)), Some((ask, ask_size))) = (t.best_bid, t.best_ask) {
                let bid_val = bid * bid_size;
                let ask_val = ask * ask_size;
                let ratio = if ask_val > 0.0 {
                    bid_val / ask_val
                } else {
                    0.0
                };
                let interpretation = if ratio > 2.0 {
                    "BUYERS"
                } else if ratio < 0.5 {
                    "STRONG ASK"
                } else {
                    "BALANCED"
                };

                let bid_bars = ((bid_val / (bid_val + ask_val)) * 10.0) as usize;
                let ask_bars = 10usize.saturating_sub(bid_bars);

                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{:>3} ", ticker),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("${:.1}M ", bid_val / 1_000_000.0),
                        Style::default().fg(Color::Green),
                    ),
                    Span::styled("█".repeat(bid_bars), Style::default().fg(Color::Green)),
                    Span::raw("   "),
                    Span::styled(
                        format!("${:.1}M ", ask_val / 1_000_000.0),
                        Style::default().fg(Color::Red),
                    ),
                    Span::styled("█".repeat(ask_bars), Style::default().fg(Color::Red)),
                    Span::raw("  "),
                    Span::styled(interpretation, Style::default().fg(Color::Cyan)),
                ]));
            }
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Waiting for orderbook data...",
            Style::default().fg(Color::Gray),
        )));
    }

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
}

fn render_momentum_signals(f: &mut Frame, snapshot: &AggregatedSnapshot, area: Rect) {
    let block = Block::default()
        .title("MOMENTUM SIGNALS")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let mut lines = Vec::new();

    // VWAP deviation
    let mut vwap_parts = Vec::new();
    vwap_parts.push(Span::styled(
        "• VWAP DEVIATION: ",
        Style::default().fg(Color::White),
    ));
    for ticker in tickers() {
        if let Some(t) = snapshot.tickers.get(ticker) {
            // Use Binance perp price for consistent VWAP deviation calculation
            if let (Some(vwap), Some(price)) = (t.vwap_1m, t.binance_perp_last.or(t.latest_price)) {
                let dev = ((price - vwap) / vwap) * 100.0;
                let color = if dev >= 0.0 { Color::Green } else { Color::Red };
                vwap_parts.push(Span::styled(
                    format!("{} {:+.2}% ", ticker, dev),
                    Style::default().fg(color),
                ));
            }
        }
    }
    lines.push(Line::from(vwap_parts));

    // Tick direction
    let mut tick_parts = Vec::new();
    tick_parts.push(Span::styled(
        "• TICK DIRECTION: ",
        Style::default().fg(Color::White),
    ));
    if let Some(t) = snapshot.tickers.get("BTC") {
        tick_parts.push(Span::styled(
            format!("↑{} ", t.tick_direction.upticks),
            Style::default().fg(Color::Green),
        ));
        tick_parts.push(Span::styled(
            format!("↓{} ", t.tick_direction.downticks),
            Style::default().fg(Color::Red),
        ));
        tick_parts.push(Span::styled(
            format!("({:.0}% upticks)", t.tick_direction.uptick_pct),
            Style::default().fg(Color::Cyan),
        ));
    }
    lines.push(Line::from(tick_parts));

    // Trade size trend + speed (approx)
    let mut size_parts = Vec::new();
    size_parts.push(Span::styled(
        "• TRADE SIZE: ",
        Style::default().fg(Color::White),
    ));
    if let Some(t) = snapshot.tickers.get("BTC") {
        size_parts.push(Span::styled(
            format!("avg ${:.0}K ", t.avg_trade_usd / 1_000.0),
            Style::default().fg(Color::Cyan),
        ));
        size_parts.push(Span::styled(
            format!("speed {:.1} t/s", t.trade_speed),
            Style::default().fg(Color::Yellow),
        ));
    }
    lines.push(Line::from(size_parts));

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
}

fn render_footer(f: &mut Frame, connected: bool, area: Rect) {
    let status = if connected {
        "CONNECTED"
    } else {
        "DISCONNECTED"
    };
    let status_color = if connected { Color::Green } else { Color::Red };

    let lines = vec![Line::from(vec![
        Span::styled("Status: ", Style::default().fg(Color::Gray)),
        Span::styled(
            status,
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  |  "),
        Span::styled("Press 'q' to quit", Style::default().fg(Color::Gray)),
    ])];

    let paragraph = Paragraph::new(lines).block(Block::default());
    f.render_widget(paragraph, area);
}
