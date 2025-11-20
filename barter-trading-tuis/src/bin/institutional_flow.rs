/// Institutional Flow Monitor TUI
///
/// Purpose: Understanding smart money positioning
/// Refresh Rate: 1 second
/// Primary Use: Position sizing, trend confirmation
///
/// Panels:
/// 1. Smart Money Tracker - Net flow, aggressor ratio, exchange dominance
/// 2. Orderbook Depth Imbalance - Bid/ask quantities at 1%, 2%, 5% depth levels
/// 3. Momentum Signals - VWAP deviation, tick direction, trade size trends
///
/// Layout based on IMPLEMENTATION_PLAN.md lines 404-454

use barter_trading_tuis::{
    InstrumentInfo, MarketEventMessage,
    OrderBookL1Data, Side, TradeData, VolumeWindow, WebSocketClient,
};
use barter_trading_tuis::shared::websocket::ConnectionStatus;
use chrono::{DateTime, Utc};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph, Wrap},
    Frame, Terminal,
};
use std::{
    collections::{HashMap, VecDeque},
    error::Error,
    io,
    time::{Duration, Instant},
};

/// Main application state
struct App {
    /// Running flag
    should_quit: bool,
    /// Last update timestamp
    last_update: Instant,
    /// Market data aggregators by instrument
    instruments: HashMap<String, InstrumentData>,
    /// Connection status
    connected: bool,
}

/// Per-instrument data aggregator
#[derive(Debug, Clone)]
struct InstrumentData {
    /// Instrument information
    info: InstrumentInfo,
    /// Latest price
    latest_price: f64,
    /// VWAP window for calculations
    vwap_window: VolumeWindow,
    /// 5-minute net flow tracker
    net_flow_5m: NetFlowTracker,
    /// Aggressor ratio tracker (buy vs sell initiated)
    aggressor_tracker: AggressorTracker,
    /// Exchange volume tracker
    exchange_volumes: HashMap<String, f64>,
    /// Tick direction tracker
    tick_tracker: TickTracker,
    /// Trade size tracker
    trade_size_tracker: TradeSizeTracker,
    /// Orderbook L1 data
    orderbook_l1: Option<OrderBookL1Data>,
    /// Last update time
    last_update: DateTime<Utc>,
}

/// Net flow tracker for 5-minute windows
#[derive(Debug, Clone)]
struct NetFlowTracker {
    /// Flow entries with timestamp
    entries: VecDeque<(DateTime<Utc>, f64)>,
    /// Window duration (5 minutes)
    window_duration: chrono::Duration,
}

impl NetFlowTracker {
    fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            window_duration: chrono::Duration::minutes(5),
        }
    }

    fn add(&mut self, time: DateTime<Utc>, flow: f64) {
        self.entries.push_back((time, flow));
        self.cleanup(time);
    }

    fn cleanup(&mut self, now: DateTime<Utc>) {
        let cutoff = now - self.window_duration;
        while let Some((time, _)) = self.entries.front() {
            if *time < cutoff {
                self.entries.pop_front();
            } else {
                break;
            }
        }
    }

    fn net_flow(&self) -> f64 {
        self.entries.iter().map(|(_, flow)| flow).sum()
    }

    fn trend_indicator(&self) -> &'static str {
        let net = self.net_flow();
        if net > 1_000_000.0 {
            "↑↑"
        } else if net > 100_000.0 {
            "↑"
        } else if net > -100_000.0 {
            "→"
        } else if net > -1_000_000.0 {
            "↓"
        } else {
            "↓↓"
        }
    }
}

/// Aggressor ratio tracker (buy vs sell initiated trades)
#[derive(Debug, Clone)]
struct AggressorTracker {
    /// Buy initiated volume
    buy_volume: f64,
    /// Sell initiated volume
    sell_volume: f64,
    /// Recent trades for time-based windowing
    trades: VecDeque<(DateTime<Utc>, Side, f64)>,
    /// Window duration (1 minute for aggressor ratio)
    window_duration: chrono::Duration,
}

impl AggressorTracker {
    fn new() -> Self {
        Self {
            buy_volume: 0.0,
            sell_volume: 0.0,
            trades: VecDeque::new(),
            window_duration: chrono::Duration::minutes(1),
        }
    }

    fn add(&mut self, time: DateTime<Utc>, side: Side, volume_usd: f64) {
        self.trades.push_back((time, side.clone(), volume_usd));
        self.cleanup(time);
        self.recalculate();
    }

    fn cleanup(&mut self, now: DateTime<Utc>) {
        let cutoff = now - self.window_duration;
        while let Some((time, _, _)) = self.trades.front() {
            if *time < cutoff {
                self.trades.pop_front();
            } else {
                break;
            }
        }
    }

    fn recalculate(&mut self) {
        self.buy_volume = 0.0;
        self.sell_volume = 0.0;
        for (_, side, volume) in &self.trades {
            match side {
                Side::Buy => self.buy_volume += volume,
                Side::Sell => self.sell_volume += volume,
            }
        }
    }

    #[allow(dead_code)]
    fn buy_percentage(&self) -> f64 {
        let total = self.buy_volume + self.sell_volume;
        if total > 0.0 {
            (self.buy_volume / total) * 100.0
        } else {
            50.0
        }
    }

    #[allow(dead_code)]
    fn sell_percentage(&self) -> f64 {
        100.0 - self.buy_percentage()
    }

    #[allow(dead_code)]
    fn ratio(&self) -> f64 {
        if self.sell_volume > 0.0 {
            self.buy_volume / self.sell_volume
        } else if self.buy_volume > 0.0 {
            999.9
        } else {
            1.0
        }
    }
}

/// Tick direction tracker (upticks vs downticks)
#[derive(Debug, Clone)]
struct TickTracker {
    /// Previous price
    prev_price: Option<f64>,
    /// Uptick count
    upticks: u64,
    /// Downtick count
    downticks: u64,
    /// Recent ticks for windowing
    ticks: VecDeque<(DateTime<Utc>, bool)>,
    /// Window duration (1 minute)
    window_duration: chrono::Duration,
}

impl TickTracker {
    fn new() -> Self {
        Self {
            prev_price: None,
            upticks: 0,
            downticks: 0,
            ticks: VecDeque::new(),
            window_duration: chrono::Duration::minutes(1),
        }
    }

    fn add(&mut self, time: DateTime<Utc>, price: f64) {
        if let Some(prev) = self.prev_price {
            if price > prev {
                self.ticks.push_back((time, true)); // uptick
            } else if price < prev {
                self.ticks.push_back((time, false)); // downtick
            }
        }
        self.prev_price = Some(price);
        self.cleanup(time);
        self.recalculate();
    }

    fn cleanup(&mut self, now: DateTime<Utc>) {
        let cutoff = now - self.window_duration;
        while let Some((time, _)) = self.ticks.front() {
            if *time < cutoff {
                self.ticks.pop_front();
            } else {
                break;
            }
        }
    }

    fn recalculate(&mut self) {
        self.upticks = 0;
        self.downticks = 0;
        for (_, is_uptick) in &self.ticks {
            if *is_uptick {
                self.upticks += 1;
            } else {
                self.downticks += 1;
            }
        }
    }

    fn uptick_percentage(&self) -> f64 {
        let total = self.upticks + self.downticks;
        if total > 0 {
            (self.upticks as f64 / total as f64) * 100.0
        } else {
            50.0
        }
    }
}

/// Trade size tracker for trend detection
#[derive(Debug, Clone)]
struct TradeSizeTracker {
    /// Recent trade sizes
    sizes: VecDeque<(DateTime<Utc>, f64)>,
    /// Window duration (5 minutes)
    window_duration: chrono::Duration,
    /// Trade count for speed calculation
    trade_count: u64,
}

impl TradeSizeTracker {
    fn new() -> Self {
        Self {
            sizes: VecDeque::new(),
            window_duration: chrono::Duration::minutes(5),
            trade_count: 0,
        }
    }

    fn add(&mut self, time: DateTime<Utc>, size_usd: f64) {
        self.sizes.push_back((time, size_usd));
        self.trade_count += 1;
        self.cleanup(time);
    }

    fn cleanup(&mut self, now: DateTime<Utc>) {
        let cutoff = now - self.window_duration;
        while let Some((time, _)) = self.sizes.front() {
            if *time < cutoff {
                self.sizes.pop_front();
            } else {
                break;
            }
        }
    }

    #[allow(dead_code)]
    fn avg_size(&self) -> f64 {
        if self.sizes.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.sizes.iter().map(|(_, size)| size).sum();
        sum / self.sizes.len() as f64
    }

    fn trend(&self) -> String {
        if self.sizes.len() < 2 {
            return "Insufficient data".to_string();
        }

        let half = self.sizes.len() / 2;
        let first_half: f64 = self.sizes.iter().take(half).map(|(_, size)| size).sum();
        let second_half: f64 = self.sizes.iter().skip(half).map(|(_, size)| size).sum();

        let first_avg = first_half / half as f64;
        let second_avg = second_half / (self.sizes.len() - half) as f64;

        if second_avg > first_avg * 1.1 {
            format!(
                "Increasing (avg ${:.0}K → ${:.0}K)",
                first_avg / 1000.0,
                second_avg / 1000.0
            )
        } else if second_avg < first_avg * 0.9 {
            format!(
                "Decreasing (avg ${:.0}K → ${:.0}K)",
                first_avg / 1000.0,
                second_avg / 1000.0
            )
        } else {
            format!("Stable (avg ${:.0}K)", second_avg / 1000.0)
        }
    }

    fn trades_per_sec(&self) -> f64 {
        if self.sizes.is_empty() {
            return 0.0;
        }
        self.sizes.len() as f64 / self.window_duration.num_seconds() as f64
    }

    fn speed_intensity(&self) -> &'static str {
        let tps = self.trades_per_sec();
        if tps > 100.0 {
            "HIGH"
        } else if tps > 20.0 {
            "MEDIUM"
        } else {
            "LOW"
        }
    }
}

impl InstrumentData {
    fn new(info: InstrumentInfo) -> Self {
        Self {
            info,
            latest_price: 0.0,
            vwap_window: VolumeWindow::new(300), // 5 minutes of data at 1 per second
            net_flow_5m: NetFlowTracker::new(),
            aggressor_tracker: AggressorTracker::new(),
            exchange_volumes: HashMap::new(),
            tick_tracker: TickTracker::new(),
            trade_size_tracker: TradeSizeTracker::new(),
            orderbook_l1: None,
            last_update: Utc::now(),
        }
    }

    fn handle_trade(&mut self, exchange: &str, trade: &TradeData, time: DateTime<Utc>) {
        self.latest_price = trade.price;
        let volume_usd = trade.price * trade.amount;

        // Update VWAP window
        self.vwap_window.add(trade.price, trade.amount);

        // Update net flow based on side
        let flow = match trade.side {
            Side::Buy => volume_usd,
            Side::Sell => -volume_usd,
        };
        self.net_flow_5m.add(time, flow);

        // Update aggressor tracker
        self.aggressor_tracker.add(time, trade.side.clone(), volume_usd);

        // Update exchange volumes
        *self.exchange_volumes.entry(exchange.to_string()).or_insert(0.0) += volume_usd;

        // Update tick direction
        self.tick_tracker.add(time, trade.price);

        // Update trade size tracker
        self.trade_size_tracker.add(time, volume_usd);

        self.last_update = time;
    }

    fn handle_orderbook_l1(&mut self, orderbook: OrderBookL1Data) {
        self.orderbook_l1 = Some(orderbook);
    }

    fn ticker(&self) -> String {
        self.info.base.to_uppercase()
    }
}

impl App {
    fn new() -> Self {
        Self {
            should_quit: false,
            last_update: Instant::now(),
            instruments: HashMap::new(),
            connected: false,
        }
    }

    fn handle_event(&mut self, event: MarketEventMessage) {
        let key = format!("{}_{}", event.instrument.base, event.instrument.quote);

        let instrument = self
            .instruments
            .entry(key)
            .or_insert_with(|| InstrumentData::new(event.instrument.clone()));

        match event.kind.as_str() {
            "trade" => {
                if let Ok(trade) = serde_json::from_value::<TradeData>(event.data) {
                    instrument.handle_trade(&event.exchange, &trade, event.time_exchange);
                }
            }
            "order_book_l1" => {
                if let Ok(orderbook) = serde_json::from_value::<OrderBookL1Data>(event.data) {
                    instrument.handle_orderbook_l1(orderbook);
                }
            }
            _ => {}
        }

        self.last_update = Instant::now();
    }

    fn get_primary_instruments(&self) -> Vec<InstrumentData> {
        let priority = ["btc", "eth", "sol"];
        let mut result = Vec::new();

        for ticker in priority {
            for (_key, data) in &self.instruments {
                if data.info.base.to_lowercase() == ticker {
                    result.push(data.clone());
                    break;
                }
            }
        }

        result
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stderr)
        .init();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let app = App::new();

    // Run app
    let res = run_app(&mut terminal, app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error: {:?}", err);
    }

    Ok(())
}

#[tokio::main]
async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    mut app: App,
) -> Result<(), Box<dyn Error>> {
    // Start WebSocket client
    let client = WebSocketClient::new();
    let (mut event_rx, mut status_rx) = client.start();

    // Main event loop
    let mut last_render = Instant::now();
    let render_interval = Duration::from_secs(1);

    loop {
        // Check for quit events
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') {
                    app.should_quit = true;
                }
            }
        }

        // Handle WebSocket events
        while let Ok(event) = event_rx.try_recv() {
            app.handle_event(event);
        }

        // Handle connection status
        while let Ok(status) = status_rx.try_recv() {
            app.connected = matches!(status, ConnectionStatus::Connected);
        }

        // Render UI at fixed interval
        if last_render.elapsed() >= render_interval {
            terminal.draw(|f| ui(f, &app))?;
            last_render = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn ui(f: &mut Frame, app: &App) {
    let size = f.area();

    // Main layout: vertical split
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),  // Smart Money Tracker
            Constraint::Length(6),  // Orderbook Depth Imbalance
            Constraint::Length(6),  // Momentum Signals
            Constraint::Min(0),     // Footer
        ])
        .split(size);

    // Render panels
    render_smart_money_tracker(f, app, main_chunks[0]);
    render_orderbook_depth_imbalance(f, app, main_chunks[1]);
    render_momentum_signals(f, app, main_chunks[2]);
    render_footer(f, app, main_chunks[3]);
}

fn render_smart_money_tracker(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title("SMART MONEY TRACKER")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Split into 3 sub-panels
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(33),
            Constraint::Percentage(34),
        ])
        .split(inner);

    // Net Flow panel
    render_net_flow_panel(f, app, chunks[0]);

    // Aggressor panel
    render_aggressor_panel(f, app, chunks[1]);

    // Exchange Dominance panel
    render_exchange_dominance_panel(f, app, chunks[2]);
}

fn render_net_flow_panel(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title("NET FLOW (5min)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green));

    let instruments = app.get_primary_instruments();
    let mut lines = Vec::new();

    for inst in instruments.iter().take(3) {
        let flow = inst.net_flow_5m.net_flow();
        let trend = inst.net_flow_5m.trend_indicator();
        let color = if flow > 0.0 { Color::Green } else { Color::Red };

        lines.push(Line::from(vec![
            Span::styled(format!("{:>3}: ", inst.ticker()), Style::default().fg(Color::White)),
            Span::styled(
                format!("{:>+10.1}M ", flow / 1_000_000.0),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(trend, Style::default().fg(color)),
        ]));
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}

fn render_aggressor_panel(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title("AGGRESSOR")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    // Aggregate across all instruments for overall market sentiment
    let instruments = app.get_primary_instruments();
    let mut total_buy = 0.0;
    let mut total_sell = 0.0;

    for inst in &instruments {
        total_buy += inst.aggressor_tracker.buy_volume;
        total_sell += inst.aggressor_tracker.sell_volume;
    }

    let buy_pct = if total_buy + total_sell > 0.0 {
        (total_buy / (total_buy + total_sell)) * 100.0
    } else {
        50.0
    };
    let sell_pct = 100.0 - buy_pct;
    let ratio = if total_sell > 0.0 {
        total_buy / total_sell
    } else {
        999.9
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
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("Ratio: "),
            Span::styled(
                format!("{:.1}:1", ratio),
                Style::default().fg(Color::Cyan),
            ),
        ]),
    ];

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

fn render_exchange_dominance_panel(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title("EXCHANGE DOMINANCE")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    // Aggregate volume across all exchanges
    let mut exchange_totals: HashMap<String, f64> = HashMap::new();
    for inst in app.instruments.values() {
        for (exchange, volume) in &inst.exchange_volumes {
            *exchange_totals.entry(exchange.clone()).or_insert(0.0) += volume;
        }
    }

    let total_volume: f64 = exchange_totals.values().sum();
    let mut exchanges: Vec<_> = exchange_totals.iter().collect();
    exchanges.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Render each exchange as a labeled bar
    let mut y_offset = 0;
    for (_i, (exchange, volume)) in exchanges.iter().take(3).enumerate() {
        let percentage = if total_volume > 0.0 {
            (*volume / total_volume) * 100.0
        } else {
            0.0
        };

        let bar_area = Rect::new(
            inner.x,
            inner.y + y_offset,
            inner.width,
            1,
        );

        if bar_area.y < inner.y + inner.height {
            let label = format!("{:>8}: {:>3.0}%", exchange, percentage);
            let gauge = Gauge::default()
                .gauge_style(Style::default().fg(Color::Blue).bg(Color::Black))
                .ratio(percentage / 100.0)
                .label(label);

            f.render_widget(gauge, bar_area);
        }

        y_offset += 1;
    }
}

fn render_orderbook_depth_imbalance(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title("ORDERBOOK DEPTH IMBALANCE")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let instruments = app.get_primary_instruments();
    let mut lines = Vec::new();

    // Note: We only have L1 data, so we'll simulate depth levels based on L1
    // In a real system with full orderbook data, you'd calculate actual depth
    for inst in instruments.iter().take(1) {
        if let Some(ref ob) = inst.orderbook_l1 {
            if let (Some(bid), Some(ask)) = (&ob.best_bid, &ob.best_ask) {
                let bid_qty = bid.amount_f64();
                let ask_qty = ask.amount_f64();
                let bid_value = bid.price_f64() * bid_qty;
                let ask_value = ask.price_f64() * ask_qty;

                // Simulate 3 depth levels (1%, 2%, 5%) based on L1
                // In reality, this would come from full orderbook data
                let levels = [
                    (1, bid_value * 2.0, ask_value * 1.5),
                    (2, bid_value * 4.0, ask_value * 3.0),
                    (5, bid_value * 8.0, ask_value * 6.0),
                ];

                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{} ", inst.ticker()),
                        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("BID         ASK", Style::default().fg(Color::Gray)),
                ]));

                for (depth, bid_val, ask_val) in levels {
                    let ratio = bid_val / ask_val;
                    let interpretation = if ratio > 2.0 {
                        "BUYERS"
                    } else if ratio < 0.5 {
                        "STRONG ASK"
                    } else {
                        "BALANCED"
                    };

                    let bid_bars = ((bid_val / (bid_val + ask_val)) * 10.0) as usize;
                    let ask_bars = 10 - bid_bars;

                    lines.push(Line::from(vec![
                        Span::styled(format!("{:>1}%   ", depth), Style::default().fg(Color::Gray)),
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
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Waiting for orderbook data...",
            Style::default().fg(Color::Gray),
        )));
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}

fn render_momentum_signals(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title("MOMENTUM SIGNALS")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let instruments = app.get_primary_instruments();
    let mut lines = Vec::new();

    // VWAP Deviation
    let mut vwap_parts = Vec::new();
    vwap_parts.push(Span::styled("• VWAP DEVIATION: ", Style::default().fg(Color::White)));
    for inst in instruments.iter().take(3) {
        if let Some(vwap) = inst.vwap_window.vwap() {
            if inst.latest_price > 0.0 {
                let deviation_pct = ((inst.latest_price - vwap) / vwap) * 100.0;
                let color = if deviation_pct > 0.0 { Color::Green } else { Color::Red };
                let symbol = if deviation_pct > 0.0 { "above" } else { "below" };

                vwap_parts.push(Span::styled(
                    format!("{} {:+.2}% {} ", inst.ticker(), deviation_pct.abs(), symbol),
                    Style::default().fg(color),
                ));
                vwap_parts.push(Span::raw("| "));
            }
        }
    }
    lines.push(Line::from(vwap_parts));

    // Tick Direction
    let mut tick_parts = Vec::new();
    tick_parts.push(Span::styled("• TICK DIRECTION: ", Style::default().fg(Color::White)));
    if let Some(inst) = instruments.first() {
        let up_pct = inst.tick_tracker.uptick_percentage();
        tick_parts.push(Span::styled(
            format!("↑{} ", inst.tick_tracker.upticks),
            Style::default().fg(Color::Green),
        ));
        tick_parts.push(Span::styled(
            format!("↓{} ", inst.tick_tracker.downticks),
            Style::default().fg(Color::Red),
        ));
        tick_parts.push(Span::styled(
            format!("({:.0}% upticks)", up_pct),
            Style::default().fg(Color::Cyan),
        ));
    }
    lines.push(Line::from(tick_parts));

    // Trade Size Trend
    let mut size_parts = Vec::new();
    size_parts.push(Span::styled("• TRADE SIZE TREND: ", Style::default().fg(Color::White)));
    if let Some(inst) = instruments.first() {
        size_parts.push(Span::styled(
            inst.trade_size_tracker.trend(),
            Style::default().fg(Color::Cyan),
        ));
    }
    lines.push(Line::from(size_parts));

    // Time & Sales Speed
    let mut speed_parts = Vec::new();
    speed_parts.push(Span::styled("• TIME&SALES SPEED: ", Style::default().fg(Color::White)));
    if let Some(inst) = instruments.first() {
        let tps = inst.trade_size_tracker.trades_per_sec();
        let intensity = inst.trade_size_tracker.speed_intensity();
        let color = match intensity {
            "HIGH" => Color::Red,
            "MEDIUM" => Color::Yellow,
            _ => Color::Green,
        };

        speed_parts.push(Span::styled(
            format!("{:.0} trades/sec ", tps),
            Style::default().fg(Color::White),
        ));
        speed_parts.push(Span::styled(
            format!("({})", intensity),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
    lines.push(Line::from(speed_parts));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let status = if app.connected { "CONNECTED" } else { "DISCONNECTED" };
    let status_color = if app.connected { Color::Green } else { Color::Red };

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
