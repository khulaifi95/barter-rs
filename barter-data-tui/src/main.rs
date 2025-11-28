use chrono::{DateTime, Utc};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::{SinkExt, StreamExt};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Gauge, List, ListItem, Paragraph, Sparkline},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    io,
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// Market event message from the server
#[derive(Debug, Clone, Deserialize, Serialize)]
struct MarketEventMessage {
    time_exchange: DateTime<Utc>,
    time_received: DateTime<Utc>,
    exchange: String,
    instrument: InstrumentInfo,
    kind: String,
    data: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct InstrumentInfo {
    base: String,
    quote: String,
    kind: String,
}

/// Side enum matching barter_instrument::Side
#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    fn to_string(&self) -> String {
        match self {
            Side::Buy => "Buy".to_string(),
            Side::Sell => "Sell".to_string(),
        }
    }
}

/// Liquidation event data
#[derive(Debug, Clone, Deserialize)]
struct LiquidationData {
    side: Side,
    price: f64,
    quantity: f64,
    time: DateTime<Utc>,
}

/// Open interest event data
#[derive(Debug, Clone, Deserialize)]
struct OpenInterestData {
    contracts: f64,
    #[allow(dead_code)]
    notional: Option<f64>,
    #[allow(dead_code)]
    time: Option<DateTime<Utc>>,
}

/// CVD event data
#[derive(Debug, Clone, Deserialize)]
struct CvdData {
    delta_base: f64,
    delta_quote: f64,
}

/// Trade event data
#[derive(Debug, Clone, Deserialize)]
struct TradeData {
    id: String,
    price: f64,
    amount: f64,
    side: Side,
}

/// Level from OrderBook
#[derive(Debug, Clone, Deserialize)]
struct Level {
    price: Decimal,
    amount: Decimal,
}

/// OrderBook L1 event data
#[derive(Debug, Clone, Deserialize)]
struct OrderBookL1Data {
    last_update_time: DateTime<Utc>,
    best_bid: Option<Level>,
    best_ask: Option<Level>,
}

/// Application state
#[derive(Clone)]
struct AppState {
    liquidations: VecDeque<LiquidationEvent>,
    open_interest: HashMap<String, OpenInterestStats>,
    cvd: HashMap<String, CvdStats>,
    trades: VecDeque<TradeEvent>,
    order_book_l1: HashMap<String, OrderBookL1Stats>,
    last_update: DateTime<Utc>,
    connected: bool,
}

#[derive(Clone)]
struct LiquidationEvent {
    time: DateTime<Utc>,
    exchange: String,
    instrument: String,
    side: String,
    price: f64,
    quantity: f64,
}

#[derive(Clone)]
struct TradeEvent {
    time: DateTime<Utc>,
    exchange: String,
    instrument: String,
    #[allow(dead_code)]
    id: String,
    side: String,
    price: f64,
    quantity: f64,
}

#[derive(Clone)]
struct OrderBookL1Stats {
    bid_price: f64,
    bid_quantity: f64,
    ask_price: f64,
    ask_quantity: f64,
    spread: f64,
    time: Option<DateTime<Utc>>,
}

impl OrderBookL1Stats {
    fn new() -> Self {
        Self {
            bid_price: 0.0,
            bid_quantity: 0.0,
            ask_price: 0.0,
            ask_quantity: 0.0,
            spread: 0.0,
            time: None,
        }
    }

    fn update(
        &mut self,
        bid_price: f64,
        bid_quantity: f64,
        ask_price: f64,
        ask_quantity: f64,
        time: Option<DateTime<Utc>>,
    ) {
        self.bid_price = bid_price;
        self.bid_quantity = bid_quantity;
        self.ask_price = ask_price;
        self.ask_quantity = ask_quantity;
        self.spread = if ask_price > 0.0 && bid_price > 0.0 {
            ask_price - bid_price
        } else {
            0.0
        };
        self.time = time;
    }

    fn spread_percentage(&self) -> f64 {
        if self.bid_price > 0.0 && self.spread > 0.0 {
            (self.spread / self.bid_price) * 100.0
        } else {
            0.0
        }
    }
}

#[derive(Clone)]
struct OpenInterestStats {
    current: f64,
    history: VecDeque<f64>,
    max_history: usize,
    change_pct: f64,
    trend: String,
}

impl OpenInterestStats {
    fn new(max_history: usize) -> Self {
        Self {
            current: 0.0,
            history: VecDeque::with_capacity(max_history),
            max_history,
            change_pct: 0.0,
            trend: "—".to_string(),
        }
    }

    fn update(&mut self, value: f64) {
        let prev = self.current;
        self.current = value;

        if self.history.len() >= self.max_history {
            self.history.pop_front();
        }
        self.history.push_back(value);

        if prev > 0.0 {
            self.change_pct = ((value - prev) / prev) * 100.0;
            self.trend = if self.change_pct > 0.0 {
                "↑".to_string()
            } else if self.change_pct < 0.0 {
                "↓".to_string()
            } else {
                "—".to_string()
            };
        }
    }

    fn get_sparkline_data(&self) -> Vec<u64> {
        self.history.iter().map(|&v| (v / 1000.0) as u64).collect()
    }
}

#[derive(Clone)]
struct CvdStats {
    delta_base: f64,
    delta_quote: f64,
    history_base: VecDeque<f64>,
    history_quote: VecDeque<f64>,
    max_history: usize,
    buy_pressure: f64,
}

impl CvdStats {
    fn new(max_history: usize) -> Self {
        Self {
            delta_base: 0.0,
            delta_quote: 0.0,
            history_base: VecDeque::with_capacity(max_history),
            history_quote: VecDeque::with_capacity(max_history),
            max_history,
            buy_pressure: 0.0,
        }
    }

    fn update(&mut self, delta_base: f64, delta_quote: f64) {
        self.delta_base = delta_base;
        self.delta_quote = delta_quote;

        if self.history_base.len() >= self.max_history {
            self.history_base.pop_front();
            self.history_quote.pop_front();
        }
        self.history_base.push_back(delta_base);
        self.history_quote.push_back(delta_quote);

        // Calculate buy pressure (percentage of buying)
        let total = delta_base.abs();
        if total > 0.0 {
            self.buy_pressure = ((delta_base + total) / (2.0 * total)) * 100.0;
        }
    }

    fn get_sparkline_data(&self) -> Vec<u64> {
        self.history_base
            .iter()
            .map(|&v| {
                // Convert to absolute value and scale
                let scaled = (v.abs() / 10.0) as u64;
                scaled.max(1)
            })
            .collect()
    }
}

impl AppState {
    fn new() -> Self {
        Self {
            liquidations: VecDeque::with_capacity(100),
            open_interest: HashMap::new(),
            cvd: HashMap::new(),
            trades: VecDeque::with_capacity(50),
            order_book_l1: HashMap::new(),
            last_update: Utc::now(),
            connected: false,
        }
    }

    fn add_liquidation(&mut self, event: LiquidationEvent) {
        if self.liquidations.len() >= 100 {
            self.liquidations.pop_front();
        }
        self.liquidations.push_back(event);
        self.last_update = Utc::now();
    }

    fn update_open_interest(&mut self, key: String, value: f64) {
        self.open_interest
            .entry(key)
            .or_insert_with(|| OpenInterestStats::new(60))
            .update(value);
        self.last_update = Utc::now();
    }

    fn update_cvd(&mut self, key: String, delta_base: f64, delta_quote: f64) {
        self.cvd
            .entry(key)
            .or_insert_with(|| CvdStats::new(60))
            .update(delta_base, delta_quote);
        self.last_update = Utc::now();
    }

    fn add_trade(&mut self, event: TradeEvent) {
        if self.trades.len() >= 50 {
            self.trades.pop_front();
        }
        self.trades.push_back(event);
        self.last_update = Utc::now();
    }

    fn update_order_book_l1(
        &mut self,
        key: String,
        bid_price: f64,
        bid_quantity: f64,
        ask_price: f64,
        ask_quantity: f64,
        time: Option<DateTime<Utc>>,
    ) {
        self.order_book_l1
            .entry(key)
            .or_insert_with(OrderBookL1Stats::new)
            .update(bid_price, bid_quantity, ask_price, ask_quantity, time);
        self.last_update = Utc::now();
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let state = Arc::new(Mutex::new(AppState::new()));

    // Start WebSocket connection
    let state_clone = state.clone();
    tokio::spawn(async move {
        let _ = websocket_client(state_clone).await;
    });

    // Run TUI
    let res = run_app(&mut terminal, state).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    let _ = res;

    Ok(())
}

async fn websocket_client(state: Arc<Mutex<AppState>>) -> Result<(), Box<dyn std::error::Error>> {
    let url = "ws://127.0.0.1:9001";

    loop {
        match connect_async(url).await {
            Ok((ws_stream, _)) => {
                {
                    let mut s = state.lock().await;
                    s.connected = true;
                }

                let (mut write, mut read) = ws_stream.split();

                // Spawn ping task to keep connection alive (prevents timeouts)
                let (ping_tx, mut ping_rx) = tokio::sync::mpsc::channel::<()>(1);
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(Duration::from_secs(30));
                    loop {
                        interval.tick().await;
                        if write.send(Message::Ping(vec![].into())).await.is_err() {
                            break;
                        }
                    }
                    let _ = ping_tx.send(()).await; // Notify main loop if ping fails
                });

                loop {
                    tokio::select! {
                        msg = read.next() => {
                            let Some(msg) = msg else {
                                break;
                            };
                    match msg {
                        Ok(Message::Text(text)) => {
                            // First check if it's a welcome message
                            if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&text) {
                                if json_val.get("type").and_then(|v| v.as_str()) == Some("welcome") {
                                    // It's a welcome message, skip it
                                    eprintln!("Received welcome message");
                                    continue;
                                }
                            }

                            // Try to parse as market event
                            match serde_json::from_str::<MarketEventMessage>(&text) {
                                Ok(event) => {
                                    process_event(state.clone(), event).await;
                                }
                                Err(e) => {
                                    eprintln!("Failed to parse message: {}", e);
                                    eprintln!("Raw message: {}", text);
                                }
                            }
                        }
                        Ok(Message::Close(_)) => {
                            eprintln!("Server closed connection");
                            break;
                        }
                        Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {
                            // Heartbeat messages - ignore (tungstenite handles automatically)
                        }
                        Err(e) => {
                            // Log error but don't disconnect - let auto-reconnect handle it
                            eprintln!("WebSocket error (will reconnect): {}", e);
                            break;
                        }
                        _ => {}
                    }
                        }
                        _ = ping_rx.recv() => {
                            // Ping task died, connection likely dead
                            eprintln!("Ping task died, reconnecting...");
                            break;
                        }
                    }
                }

                {
                    let mut s = state.lock().await;
                    s.connected = false;
                }
            }
            Err(_) => {
                // Connection failed, retry after delay
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }

        // Wait before reconnecting
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn process_event(state: Arc<Mutex<AppState>>, event: MarketEventMessage) {
    let key = format!(
        "{}-{}/{}",
        event.exchange, event.instrument.base, event.instrument.quote
    );

    match event.kind.as_str() {
        "liquidation" => {
            if let Ok(liq) = serde_json::from_value::<LiquidationData>(event.data.clone()) {
                let mut s = state.lock().await;
                s.add_liquidation(LiquidationEvent {
                    time: liq.time,
                    exchange: event.exchange,
                    instrument: format!("{}/{}", event.instrument.base, event.instrument.quote),
                    side: liq.side.to_string(),
                    price: liq.price,
                    quantity: liq.quantity,
                });
            }
        }
        "open_interest" => {
            if let Ok(oi) = serde_json::from_value::<OpenInterestData>(event.data.clone()) {
                // Only track perp/futures instruments to match panel capacity
                if event.instrument.kind.to_lowercase().contains("perpetual")
                    || event.instrument.kind.to_lowercase().contains("future")
                {
                    let mut s = state.lock().await;
                    s.update_open_interest(key, oi.contracts);
                }
            }
        }
        "cumulative_volume_delta" => {
            if let Ok(cvd) = serde_json::from_value::<CvdData>(event.data) {
                // Perp-only for CVD in the legacy view
                if event.instrument.kind.to_lowercase().contains("perpetual")
                    || event.instrument.kind.to_lowercase().contains("future")
                {
                    let mut s = state.lock().await;
                    s.update_cvd(key, cvd.delta_base, cvd.delta_quote);
                }
            }
        }
        "trade" => {
            if let Ok(trade) = serde_json::from_value::<TradeData>(event.data.clone()) {
                let mut s = state.lock().await;
                s.add_trade(TradeEvent {
                    time: event.time_exchange,
                    exchange: event.exchange,
                    instrument: format!("{}/{}", event.instrument.base, event.instrument.quote),
                    id: trade.id,
                    side: trade.side.to_string(),
                    price: trade.price,
                    quantity: trade.amount,
                });
            }
        }
        "order_book_l1" => {
            if let Ok(ob) = serde_json::from_value::<OrderBookL1Data>(event.data.clone()) {
                // Only show perp/futures order books in the legacy panel to avoid overcrowding
                let kind = event.instrument.kind.to_lowercase();
                if !kind.contains("perpetual") && !kind.contains("future") {
                    return;
                }

                let bid_price = ob
                    .best_bid
                    .as_ref()
                    .map(|l| l.price.to_string().parse::<f64>().unwrap_or(0.0))
                    .unwrap_or(0.0);
                let bid_quantity = ob
                    .best_bid
                    .as_ref()
                    .map(|l| l.amount.to_string().parse::<f64>().unwrap_or(0.0))
                    .unwrap_or(0.0);
                let ask_price = ob
                    .best_ask
                    .as_ref()
                    .map(|l| l.price.to_string().parse::<f64>().unwrap_or(0.0))
                    .unwrap_or(0.0);
                let ask_quantity = ob
                    .best_ask
                    .as_ref()
                    .map(|l| l.amount.to_string().parse::<f64>().unwrap_or(0.0))
                    .unwrap_or(0.0);

                let mut s = state.lock().await;
                s.update_order_book_l1(
                    key,
                    bid_price,
                    bid_quantity,
                    ask_price,
                    ask_quantity,
                    Some(ob.last_update_time),
                );
            }
        }
        _ => {
            // Silently ignore unknown event types
        }
    }
}

async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    state: Arc<Mutex<AppState>>,
) -> io::Result<()> {
    let tick_rate = Duration::from_millis(250);
    let mut last_tick = std::time::Instant::now();

    loop {
        let state_snapshot = {
            let s = state.lock().await;
            s.clone()
        };

        // No debug logging - clean UI

        terminal.draw(|f| ui(f, &state_snapshot))?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if crossterm::event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
                    return Ok(());
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = std::time::Instant::now();
        }
    }
}

fn ui(f: &mut Frame, state: &AppState) {
    let size = f.area();

    // Main layout: top status bar, main content
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(size);

    // Status bar with gradient effect
    render_status_bar(f, chunks[0], state);

    // Main content: use a 2x3 grid layout
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .margin(1)
        .split(chunks[1]);

    // Left column: Liquidations (top), Trades (bottom)
    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(main_chunks[0]);

    // Right column: OrderBook L1 (top), OI and CVD (bottom)
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(main_chunks[1]);

    // Render left column
    render_liquidations(f, left_chunks[0], state);
    render_trades(f, left_chunks[1], state);

    // Render right column
    render_order_book_l1(f, right_chunks[0], state);

    // Bottom right: OI and CVD
    let bottom_right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(right_chunks[1]);

    render_open_interest(f, bottom_right_chunks[0], state);
    render_cvd(f, bottom_right_chunks[1], state);
}

fn render_status_bar(f: &mut Frame, area: Rect, state: &AppState) {
    let status_symbol = if state.connected { "●" } else { "○" };
    let status_color = if state.connected {
        Color::Rgb(0, 255, 127)
    } else {
        Color::Rgb(255, 69, 58)
    };
    let status_text = if state.connected {
        "CONNECTED"
    } else {
        "DISCONNECTED"
    };

    let status = Span::styled(
        format!(" {} {} ", status_symbol, status_text),
        Style::default()
            .fg(status_color)
            .add_modifier(Modifier::BOLD),
    );

    let time = Span::styled(
        format!(" ⏱  {} ", state.last_update.format("%H:%M:%S%.3f")),
        Style::default().fg(Color::Rgb(100, 149, 237)),
    );

    let title = Span::styled(
        " ◆ BARTER DATA TERMINAL ◆ ",
        Style::default()
            .fg(Color::Rgb(255, 215, 0))
            .add_modifier(Modifier::BOLD),
    );

    let help = Span::styled(" [Q] Quit ", Style::default().fg(Color::Rgb(128, 128, 128)));

    let status_line = Line::from(vec![status, time, title, help]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Rgb(138, 43, 226)))
        .style(Style::default().bg(Color::Rgb(18, 18, 28)));

    let paragraph = Paragraph::new(status_line)
        .block(block)
        .alignment(Alignment::Center);

    f.render_widget(paragraph, area);
}

fn render_liquidations(f: &mut Frame, area: Rect, state: &AppState) {
    let items: Vec<ListItem> = state
        .liquidations
        .iter()
        .rev()
        .take(area.height.saturating_sub(4) as usize)
        .enumerate()
        .map(|(idx, liq)| {
            let is_buy = liq.side.to_lowercase().contains("buy");
            let color = if is_buy {
                Color::Rgb(0, 255, 127)
            } else {
                Color::Rgb(255, 69, 58)
            };
            let bg_color = if idx % 2 == 0 {
                Color::Rgb(25, 25, 35)
            } else {
                Color::Rgb(20, 20, 30)
            };

            let symbol = if is_buy { "▲" } else { "▼" };
            let exchange_color = match liq.exchange.as_str() {
                "Okx" => Color::Rgb(0, 120, 255),
                "BinanceFuturesUsd" => Color::Rgb(240, 185, 11),
                _ => Color::Rgb(255, 92, 0),
            };

            let line = Line::from(vec![
                Span::styled(
                    format!(" {} ", liq.time.format("%H:%M:%S")),
                    Style::default().fg(Color::Rgb(128, 128, 150)).bg(bg_color),
                ),
                Span::styled(
                    format!("{} ", symbol),
                    Style::default()
                        .fg(color)
                        .bg(bg_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("[{:^8}] ", liq.exchange),
                    Style::default().fg(exchange_color).bg(bg_color),
                ),
                Span::styled(
                    format!("{:<10} ", liq.instrument),
                    Style::default().fg(Color::Rgb(200, 200, 220)).bg(bg_color),
                ),
                Span::styled(
                    format!("${:>10.2} ", liq.price),
                    Style::default()
                        .fg(Color::Rgb(255, 215, 0))
                        .bg(bg_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" Qty:{:.4} ", liq.quantity),
                    Style::default().fg(Color::Rgb(255, 105, 180)).bg(bg_color),
                ),
            ]);

            ListItem::new(line).style(Style::default().bg(bg_color))
        })
        .collect();

    let title = Line::from(vec![
        Span::styled(
            " ⚡ ",
            Style::default()
                .fg(Color::Rgb(255, 215, 0))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "LIQUIDATIONS FEED",
            Style::default()
                .fg(Color::Rgb(255, 255, 255))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" ({}) ", state.liquidations.len()),
            Style::default().fg(Color::Rgb(128, 128, 150)),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(255, 69, 58)))
        .title_top(title.alignment(Alignment::Center))
        .style(Style::default().bg(Color::Rgb(15, 15, 25)));

    let list = List::new(items).block(block);

    f.render_widget(list, area);
}

fn render_open_interest(f: &mut Frame, area: Rect, state: &AppState) {
    let title = Line::from(vec![
        Span::styled(" 📊 ", Style::default().fg(Color::Rgb(100, 149, 237))),
        Span::styled(
            "OPEN INTEREST",
            Style::default()
                .fg(Color::Rgb(255, 255, 255))
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(100, 149, 237)))
        .title_top(title.alignment(Alignment::Center))
        .style(Style::default().bg(Color::Rgb(15, 15, 25)));

    if state.open_interest.is_empty() || area.height < 4 {
        let waiting = Paragraph::new(Text::from(vec![
            Line::from(""),
            Line::from(Span::styled(
                "⏳ Waiting for data...",
                Style::default()
                    .fg(Color::Rgb(128, 128, 150))
                    .add_modifier(Modifier::ITALIC),
            )),
        ]))
        .block(block)
        .alignment(Alignment::Center);

        f.render_widget(waiting, area);
        return;
    }

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height == 0 {
        return;
    }

    let chunk_height = inner.height / state.open_interest.len() as u16;
    let mut y_offset = 0;

    for (key, stats) in state.open_interest.iter() {
        // Skip if we're out of bounds
        if y_offset >= inner.height {
            break;
        }
        let trend_color = if stats.change_pct > 0.0 {
            Color::Rgb(0, 255, 127)
        } else if stats.change_pct < 0.0 {
            Color::Rgb(255, 69, 58)
        } else {
            Color::Rgb(128, 128, 150)
        };

        let trend_symbol = &stats.trend;

        // Create mini layout for each OI entry
        let mini_area = Rect {
            x: inner.x,
            y: inner.y + y_offset,
            width: inner.width,
            height: chunk_height.min(inner.height - y_offset),
        };

        let lines = vec![
            Line::from(vec![Span::styled(
                format!(" {} ", key),
                Style::default()
                    .fg(Color::Rgb(100, 200, 255))
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(vec![
                Span::styled(
                    format!("  Value: {:>14.0} ", stats.current),
                    Style::default().fg(Color::Rgb(255, 255, 255)),
                ),
                Span::styled(
                    format!(" {} ", trend_symbol),
                    Style::default()
                        .fg(trend_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:>7.2}%", stats.change_pct),
                    Style::default()
                        .fg(trend_color)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
        ];

        let paragraph = Paragraph::new(lines).style(Style::default().bg(Color::Rgb(15, 15, 25)));
        f.render_widget(paragraph, mini_area);

        // Render sparkline
        if !stats.history.is_empty() && mini_area.y + 2 < inner.y + inner.height {
            let sparkline_data = stats.get_sparkline_data();
            if !sparkline_data.is_empty() {
                let sparkline_area = Rect {
                    x: mini_area.x + 2,
                    y: mini_area.y + 2,
                    width: mini_area.width.saturating_sub(4),
                    height: 1,
                };

                if sparkline_area.y < inner.y + inner.height {
                    let sparkline = Sparkline::default()
                        .data(&sparkline_data)
                        .style(Style::default().fg(Color::Rgb(100, 149, 237)))
                        .max(sparkline_data.iter().max().copied().unwrap_or(100));

                    f.render_widget(sparkline, sparkline_area);
                }
            }
        }

        y_offset += chunk_height;
    }
}

fn render_cvd(f: &mut Frame, area: Rect, state: &AppState) {
    let title = Line::from(vec![
        Span::styled(" 💹 ", Style::default().fg(Color::Rgb(255, 105, 180))),
        Span::styled(
            "CUMULATIVE VOLUME DELTA",
            Style::default()
                .fg(Color::Rgb(255, 255, 255))
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(255, 105, 180)))
        .title_top(title.alignment(Alignment::Center))
        .style(Style::default().bg(Color::Rgb(15, 15, 25)));

    if state.cvd.is_empty() || area.height < 4 {
        let waiting = Paragraph::new(Text::from(vec![
            Line::from(""),
            Line::from(Span::styled(
                "⏳ Waiting for data...",
                Style::default()
                    .fg(Color::Rgb(128, 128, 150))
                    .add_modifier(Modifier::ITALIC),
            )),
        ]))
        .block(block)
        .alignment(Alignment::Center);

        f.render_widget(waiting, area);
        return;
    }

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height == 0 {
        return;
    }

    let chunk_height = inner.height / state.cvd.len() as u16;
    let mut y_offset = 0;

    for (key, stats) in state.cvd.iter() {
        // Skip if we're out of bounds
        if y_offset >= inner.height {
            break;
        }
        let pressure_color = if stats.buy_pressure > 60.0 {
            Color::Rgb(0, 255, 127)
        } else if stats.buy_pressure < 40.0 {
            Color::Rgb(255, 69, 58)
        } else {
            Color::Rgb(255, 215, 0)
        };

        let delta_color = if stats.delta_base >= 0.0 {
            Color::Rgb(0, 255, 127)
        } else {
            Color::Rgb(255, 69, 58)
        };

        // Create mini layout for each CVD entry
        let mini_area = Rect {
            x: inner.x,
            y: inner.y + y_offset,
            width: inner.width,
            height: chunk_height.min(inner.height - y_offset),
        };

        // Header
        let header = Paragraph::new(Line::from(vec![Span::styled(
            format!(" {} ", key),
            Style::default()
                .fg(Color::Rgb(255, 150, 200))
                .add_modifier(Modifier::BOLD),
        )]))
        .style(Style::default().bg(Color::Rgb(15, 15, 25)));

        let header_area = Rect {
            x: mini_area.x,
            y: mini_area.y,
            width: mini_area.width,
            height: 1,
        };
        f.render_widget(header, header_area);

        // Delta values
        let delta_text = vec![
            Line::from(vec![
                Span::styled(
                    "  Δ Base:  ",
                    Style::default().fg(Color::Rgb(150, 150, 170)),
                ),
                Span::styled(
                    format!("{:>12.4}", stats.delta_base),
                    Style::default()
                        .fg(delta_color)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled(
                    "  Δ Quote: ",
                    Style::default().fg(Color::Rgb(150, 150, 170)),
                ),
                Span::styled(
                    format!("{:>12.2}", stats.delta_quote),
                    Style::default().fg(Color::Rgb(200, 200, 220)),
                ),
            ]),
        ];

        let delta_area = Rect {
            x: mini_area.x,
            y: mini_area.y + 1,
            width: mini_area.width,
            height: 2,
        };
        let delta_paragraph = Paragraph::new(delta_text);
        f.render_widget(delta_paragraph, delta_area);

        // Buy pressure gauge
        if mini_area.y + 3 < inner.y + inner.height {
            let gauge_area = Rect {
                x: mini_area.x + 2,
                y: mini_area.y + 3,
                width: mini_area.width.saturating_sub(4),
                height: 1,
            };

            if gauge_area.y < inner.y + inner.height {
                let gauge = Gauge::default()
                    .block(Block::default())
                    .gauge_style(
                        Style::default()
                            .fg(pressure_color)
                            .bg(Color::Rgb(30, 30, 40)),
                    )
                    .ratio(stats.buy_pressure / 100.0)
                    .label(Span::styled(
                        format!(" Buy Pressure: {:.1}% ", stats.buy_pressure),
                        Style::default()
                            .fg(Color::Rgb(255, 255, 255))
                            .add_modifier(Modifier::BOLD),
                    ));

                f.render_widget(gauge, gauge_area);
            }
        }

        // Sparkline
        if !stats.history_base.is_empty() && mini_area.y + 4 < inner.y + inner.height {
            let sparkline_data = stats.get_sparkline_data();
            if !sparkline_data.is_empty() {
                let sparkline_area = Rect {
                    x: mini_area.x + 2,
                    y: mini_area.y + 4,
                    width: mini_area.width.saturating_sub(4),
                    height: 1,
                };

                if sparkline_area.y < inner.y + inner.height {
                    let sparkline = Sparkline::default()
                        .data(&sparkline_data)
                        .style(Style::default().fg(Color::Rgb(255, 105, 180)))
                        .max(sparkline_data.iter().max().copied().unwrap_or(100));

                    f.render_widget(sparkline, sparkline_area);
                }
            }
        }

        y_offset += chunk_height;
    }
}

fn render_trades(f: &mut Frame, area: Rect, state: &AppState) {
    let title = Line::from(vec![
        Span::styled(
            " 📈 ",
            Style::default()
                .fg(Color::Rgb(255, 255, 255))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "TRADES FEED",
            Style::default()
                .fg(Color::Rgb(255, 255, 255))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" ({}) ", state.trades.len()),
            Style::default().fg(Color::Rgb(128, 128, 150)),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(255, 255, 255)))
        .title_top(title.alignment(Alignment::Center))
        .style(Style::default().bg(Color::Rgb(15, 15, 25)));

    if state.trades.is_empty() {
        let waiting = Paragraph::new(Text::from(vec![
            Line::from(""),
            Line::from(Span::styled(
                "⏳ Waiting for trade data...",
                Style::default()
                    .fg(Color::Rgb(128, 128, 150))
                    .add_modifier(Modifier::ITALIC),
            )),
        ]))
        .block(block)
        .alignment(Alignment::Center);

        f.render_widget(waiting, area);
        return;
    }

    let items: Vec<ListItem> = state
        .trades
        .iter()
        .rev()
        .take(area.height.saturating_sub(4) as usize)
        .enumerate()
        .map(|(idx, trade)| {
            let is_buy = trade.side.to_lowercase().contains("buy");
            let color = if is_buy {
                Color::Rgb(0, 255, 127)
            } else {
                Color::Rgb(255, 69, 58)
            };
            let bg_color = if idx % 2 == 0 {
                Color::Rgb(25, 25, 35)
            } else {
                Color::Rgb(20, 20, 30)
            };

            let symbol = if is_buy { "▲" } else { "▼" };
            let exchange_color = match trade.exchange.as_str() {
                "Okx" => Color::Rgb(0, 120, 255),
                "BinanceFuturesUsd" => Color::Rgb(240, 185, 11),
                _ => Color::Rgb(255, 92, 0),
            };

            let line = Line::from(vec![
                Span::styled(
                    format!(" {} ", trade.time.format("%H:%M:%S")),
                    Style::default().fg(Color::Rgb(128, 128, 150)).bg(bg_color),
                ),
                Span::styled(
                    format!("{} ", symbol),
                    Style::default()
                        .fg(color)
                        .bg(bg_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("[{:^8}] ", trade.exchange),
                    Style::default().fg(exchange_color).bg(bg_color),
                ),
                Span::styled(
                    format!("{:<10} ", trade.instrument),
                    Style::default().fg(Color::Rgb(200, 200, 220)).bg(bg_color),
                ),
                Span::styled(
                    format!("${:>10.2} ", trade.price),
                    Style::default()
                        .fg(Color::Rgb(255, 255, 255))
                        .bg(bg_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" Qty:{:.4} ", trade.quantity),
                    Style::default().fg(Color::Rgb(100, 200, 255)).bg(bg_color),
                ),
            ]);

            ListItem::new(line).style(Style::default().bg(bg_color))
        })
        .collect();

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn render_order_book_l1(f: &mut Frame, area: Rect, state: &AppState) {
    let title = Line::from(vec![
        Span::styled(
            " 📊 ",
            Style::default()
                .fg(Color::Rgb(100, 255, 218))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "ORDERBOOK L1",
            Style::default()
                .fg(Color::Rgb(255, 255, 255))
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(100, 255, 218)))
        .title_top(title.alignment(Alignment::Center))
        .style(Style::default().bg(Color::Rgb(15, 15, 25)));

    if state.order_book_l1.is_empty() || area.height < 4 {
        let waiting = Paragraph::new(Text::from(vec![
            Line::from(""),
            Line::from(Span::styled(
                "⏳ Waiting for order book data...",
                Style::default()
                    .fg(Color::Rgb(128, 128, 150))
                    .add_modifier(Modifier::ITALIC),
            )),
        ]))
        .block(block)
        .alignment(Alignment::Center);

        f.render_widget(waiting, area);
        return;
    }

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height == 0 {
        return;
    }

    let chunk_height = inner.height / state.order_book_l1.len() as u16;
    let mut y_offset = 0;

    for (key, stats) in state.order_book_l1.iter() {
        // Skip if we're out of bounds
        if y_offset >= inner.height {
            break;
        }
        // Create mini layout for each OrderBook L1 entry
        let mini_area = Rect {
            x: inner.x,
            y: inner.y + y_offset,
            width: inner.width,
            height: chunk_height.min(inner.height - y_offset),
        };

        let spread_color = if stats.spread_percentage() < 0.01 {
            Color::Rgb(0, 255, 127)
        } else if stats.spread_percentage() < 0.05 {
            Color::Rgb(255, 215, 0)
        } else {
            Color::Rgb(255, 69, 58)
        };

        let lines = vec![
            Line::from(vec![Span::styled(
                format!(" {} ", key),
                Style::default()
                    .fg(Color::Rgb(100, 255, 218))
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(vec![
                Span::styled(
                    format!("  Bid: ${:>10.2} ", stats.bid_price),
                    Style::default().fg(Color::Rgb(255, 69, 58)),
                ),
                Span::styled(
                    format!(" {:>8.2} ", stats.bid_quantity),
                    Style::default().fg(Color::Rgb(255, 150, 150)),
                ),
            ]),
            Line::from(vec![
                Span::styled(
                    format!("  Ask: ${:>10.2} ", stats.ask_price),
                    Style::default().fg(Color::Rgb(0, 255, 127)),
                ),
                Span::styled(
                    format!(" {:>8.2} ", stats.ask_quantity),
                    Style::default().fg(Color::Rgb(150, 255, 150)),
                ),
            ]),
            Line::from(vec![
                Span::styled(
                    format!("  Spread: ${:>9.4} ", stats.spread),
                    Style::default()
                        .fg(spread_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {:>6.3}% ", stats.spread_percentage()),
                    Style::default()
                        .fg(spread_color)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
        ];

        let paragraph = Paragraph::new(lines).style(Style::default().bg(Color::Rgb(15, 15, 25)));
        f.render_widget(paragraph, mini_area);

        y_offset += chunk_height;
    }
}
