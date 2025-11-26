/// Risk & Arbitrage Scanner (Opus TUI #3)
///
/// Uses the shared aggregation engine to show liquidation cascade risk,
/// arbitrage/basis signals, market regime hints, and correlations.
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
    AggregatedSnapshot, Aggregator, BasisState, ConnectionStatus, WebSocketClient, WebSocketConfig,
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
    widgets::{Block, Borders, Paragraph, Row, Table, Wrap},
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

    let tick_rate = Duration::from_secs(5);
    let mut last_tick = Instant::now();

    loop {
        if last_tick.elapsed() >= tick_rate {
            let snapshot = {
                let guard = aggregator.lock().await;
                guard.snapshot()
            };
            let is_connected = connected.load(Ordering::Relaxed);
            terminal.draw(|f| render_ui(f, &snapshot, is_connected))?;
            last_tick = Instant::now();
        }

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
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

fn render_ui(f: &mut Frame, snapshot: &AggregatedSnapshot, connected: bool) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(f.area());

    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(chunks[0]);
    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(chunks[1]);

    render_risk_metrics(f, snapshot, top[0]);
    render_arbitrage(f, snapshot, top[1]);
    render_market_regime(f, snapshot, bottom[0], connected);
    render_correlation(f, snapshot, bottom[1]);
}

fn render_risk_metrics(f: &mut Frame, snapshot: &AggregatedSnapshot, area: Rect) {
    let block = Block::default()
        .title(" RISK METRICS ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::White));

    let btc = "BTC";
    let mut lines = Vec::new();
    if let Some(t) = snapshot.tickers.get(btc) {
        let risk_score = t.cascade_risk;
        let level = if risk_score > 70.0 {
            "HIGH"
        } else if risk_score > 40.0 {
            "MEDIUM"
        } else {
            "LOW"
        };
        let filled = ((risk_score / 100.0 * 10.0) as usize).min(10);
        let bar = format!("{}{}", "█".repeat(filled), "░".repeat(10 - filled));
        lines.push(Line::from(vec![
            Span::styled(
                "LIQUIDATION CASCADE RISK: ",
                Style::default().fg(Color::White),
            ),
            Span::styled(
                bar.clone(),
                Style::default().fg(if risk_score > 70.0 {
                    Color::Red
                } else if risk_score > 40.0 {
                    Color::Yellow
                } else {
                    Color::Green
                }),
            ),
            Span::raw(" "),
            Span::styled(
                level,
                Style::default()
                    .fg(if risk_score > 70.0 {
                        Color::Red
                    } else if risk_score > 40.0 {
                        Color::Yellow
                    } else {
                        Color::Green
                    })
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        if let Some(level) = &t.next_cascade_level {
            let pct = if let Some(price) = t.latest_price {
                ((level.price - price) / price) * 100.0
            } else {
                0.0
            };
            lines.push(Line::from(vec![
                Span::styled("Next Level: ", Style::default().fg(Color::Gray)),
                Span::styled(
                    format!("${:.0}", level.price),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("({:+.1}%)", pct),
                    Style::default().fg(if pct < 0.0 { Color::Red } else { Color::Green }),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("${:.1}M {:?}", level.total_usd / 1_000_000.0, level.side),
                    Style::default().fg(Color::Cyan),
                ),
            ]));
        }

        if let Some(level) = &t.protection_level {
            let pct = if let Some(price) = t.latest_price {
                ((level.price - price) / price) * 100.0
            } else {
                0.0
            };
            lines.push(Line::from(vec![
                Span::styled("Protection: ", Style::default().fg(Color::Gray)),
                Span::styled(
                    format!("${:.0}", level.price),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(format!("({:+.1}%)", pct), Style::default().fg(Color::Green)),
                Span::raw(" "),
                Span::styled(
                    format!("${:.1}M {:?}", level.total_usd / 1_000_000.0, level.side),
                    Style::default().fg(Color::Cyan),
                ),
            ]));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "Waiting for BTC data...",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

fn render_arbitrage(f: &mut Frame, snapshot: &AggregatedSnapshot, area: Rect) {
    let block = Block::default()
        .title(" ARBITRAGE & BASIS ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::White));

    let mut lines = Vec::new();
    for ticker in tickers() {
        if let Some(t) = snapshot.tickers.get(ticker) {
            if let Some(basis) = &t.basis {
                let (state_label, state_color) = match basis.state {
                    BasisState::Contango => ("CONTANGO", Color::Green),
                    BasisState::Backwardation => ("BACKWRD", Color::Red),
                    BasisState::Unknown => ("NEUTRAL", Color::Yellow),
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{:3}: ", ticker),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{:+.1}", basis.basis_usd),
                        Style::default().fg(state_color),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        format!("({:+.2}%)", basis.basis_pct),
                        Style::default().fg(state_color),
                    ),
                    Span::raw(" "),
                    Span::styled(state_label, Style::default().fg(state_color)),
                    if basis.steep {
                        Span::styled(" ⚠️", Style::default().fg(Color::Red))
                    } else {
                        Span::raw("")
                    },
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{:3}: ", ticker),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
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

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
}

fn render_market_regime(f: &mut Frame, snapshot: &AggregatedSnapshot, area: Rect, connected: bool) {
    let block = Block::default()
        .title(" MARKET REGIME (approx) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::White));

    let mut lines = Vec::new();
    if let Some(t) = snapshot.tickers.get("BTC") {
        let vol_state = if t.tick_direction.uptick_pct > 60.0 {
            "TRENDING"
        } else if t.tick_direction.uptick_pct < 40.0 {
            "DOWNTREND"
        } else {
            "RANGING"
        };

        lines.push(Line::from(vec![
            Span::styled("State: ", Style::default().fg(Color::Gray)),
            Span::styled(
                vol_state,
                Style::default().fg(match vol_state {
                    "TRENDING" => Color::Green,
                    "DOWNTREND" => Color::Red,
                    _ => Color::Yellow,
                }),
            ),
        ]));

        lines.push(Line::from(vec![
            Span::styled("CVD: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("Δ {:>+7.1}M", t.cvd.total_quote / 1_000_000.0),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw("  v "),
            Span::styled(
                format!("{:+.1}k/s", t.cvd.velocity_quote / 1_000.0),
                Style::default().fg(Color::Cyan),
            ),
        ]));
    } else {
        lines.push(Line::from(Span::styled(
            if connected {
                "Waiting for BTC data..."
            } else {
                "Disconnected"
            },
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
}

fn render_correlation(f: &mut Frame, snapshot: &AggregatedSnapshot, area: Rect) {
    let matrix = snapshot.correlation;
    let symbols = ["BTC", "ETH", "SOL"];

    let header_cells = ["", "BTC", "ETH", "SOL"].iter().map(|h| {
        ratatui::widgets::Cell::from(*h).style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    });
    let header = Row::new(header_cells).height(1);

    let rows = matrix.iter().enumerate().map(|(i, row)| {
        let cells = std::iter::once(symbols[i].to_string())
            .chain(row.iter().map(|&c| format!("{:.2}", c)))
            .enumerate()
            .map(|(j, content)| {
                let color = if j == 0 {
                    Color::Yellow
                } else {
                    let val = if j - 1 < row.len() { row[j - 1] } else { 0.0 };
                    if val >= 0.8 {
                        Color::Green
                    } else if val >= 0.5 {
                        Color::Cyan
                    } else if val >= 0.0 {
                        Color::Yellow
                    } else {
                        Color::Red
                    }
                };
                ratatui::widgets::Cell::from(content).style(Style::default().fg(color))
            });
        Row::new(cells).height(1)
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(6),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" CORRELATION MATRIX ")
            .border_style(Style::default().fg(Color::White)),
    );

    f.render_widget(table, area);
}
