/// Market Microstructure Dashboard
///
/// Real-time orderflow and market activity monitoring
/// Refresh Rate: 250ms
/// Primary Use: Active trading decisions
///
/// Panels:
/// 1. Orderflow Imbalance (1m window)
/// 2. Spot vs Perp Basis
/// 3. Liquidation Clusters
/// 4. Funding Momentum
/// 5. Whale Detector (>$500K)
/// 6. CVD Divergence

use barter_trading_tuis::{
    CvdData, LiquidationData, MarketEventMessage, OpenInterestData, OrderBookL1Data, Side,
    TradeData, WebSocketClient, WebSocketConfig,
};
use chrono::{DateTime, Utc};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Terminal,
};
use std::{
    collections::{HashMap, VecDeque},
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

/// Supported trading pairs
const TICKERS: [&str; 3] = ["btc", "eth", "sol"];

/// Large trade threshold ($500K)
const WHALE_THRESHOLD: f64 = 500_000.0;

/// Mega whale threshold ($5M)
const MEGA_WHALE_THRESHOLD: f64 = 5_000_000.0;

/// Orderflow window size (1 minute at ~100 events/sec)
const ORDERFLOW_WINDOW_SIZE: usize = 6000;

/// Liquidation cluster price bucket size ($100)
const LIQ_BUCKET_SIZE: f64 = 100.0;

/// Maximum whale trades to display
const MAX_WHALE_DISPLAY: usize = 10;

/// Maximum liquidation clusters to display
const MAX_LIQ_CLUSTERS: usize = 5;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Disable logging in TUI mode (logs interfere with terminal display)
    // To enable logs, set env var: RUST_LOG=info and redirect to file:
    // RUST_LOG=info cargo run --bin market-microstructure 2> tui.log

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create WebSocket client
    let config = WebSocketConfig::new("ws://127.0.0.1:9001")
        .with_ping_interval(Duration::from_secs(30))
        .with_reconnect_delay(Duration::from_secs(2))
        .with_channel_buffer_size(5000);

    let client = WebSocketClient::with_config(config);
    let (mut event_rx, mut status_rx) = client.start();

    // Create app state (wrapped in Arc<Mutex> for sharing)
    let app = Arc::new(Mutex::new(AppState::new()));

    // Create channel for UI updates
    let (ui_tx, mut ui_rx) = mpsc::channel::<()>(100);

    // Spawn event processing task
    let ui_tx_clone = ui_tx.clone();
    let app_clone = Arc::clone(&app);
    let app_handle = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            if let Ok(mut app_state) = app_clone.lock() {
                app_state.process_event(event);
            }
            let _ = ui_tx_clone.send(()).await;
        }
    });

    // Spawn connection status monitor (silently track status)
    tokio::spawn(async move {
        while let Some(_status) = status_rx.recv().await {
            // Status changes tracked but not logged (would interfere with TUI)
        }
    });

    // Main UI loop
    let mut last_draw = Instant::now();
    let draw_interval = Duration::from_millis(250);

    let result = loop {
        // Handle keyboard input
        if event::poll(Duration::from_millis(10))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') {
                    break Ok(());
                }
            }
        }

        // Check for UI update trigger
        if ui_rx.try_recv().is_ok() || last_draw.elapsed() >= draw_interval {
            if let Ok(app_state) = app.lock() {
                terminal.draw(|f| {
                    let size = f.area();
                    render_ui(f, size, &app_state);
                })?;
            }
            last_draw = Instant::now();
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    };

    // Cleanup
    app_handle.abort();
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

/// Application state
#[derive(Debug)]
struct AppState {
    /// Per-ticker metrics
    tickers: HashMap<String, TickerMetrics>,
    /// Last update time
    last_update: Instant,
}

impl AppState {
    fn new() -> Self {
        let mut tickers = HashMap::new();
        for ticker in TICKERS {
            tickers.insert(ticker.to_uppercase(), TickerMetrics::new(ticker));
        }

        Self {
            tickers,
            last_update: Instant::now(),
        }
    }

    fn process_event(&mut self, event: MarketEventMessage) {
        let ticker = event.instrument.base.to_uppercase();

        if let Some(metrics) = self.tickers.get_mut(&ticker) {
            match event.kind.as_str() {
                "trade" => {
                    if let Ok(trade) = serde_json::from_value::<TradeData>(event.data) {
                        metrics.process_trade(trade, &event.exchange, event.time_exchange);
                    }
                }
                "liquidation" => {
                    if let Ok(liq) = serde_json::from_value::<LiquidationData>(event.data) {
                        metrics.process_liquidation(liq, &event.exchange);
                    }
                }
                "cumulative_volume_delta" => {
                    if let Ok(cvd) = serde_json::from_value::<CvdData>(event.data) {
                        metrics.process_cvd(cvd);
                    }
                }
                "order_book_l1" => {
                    if let Ok(ob) = serde_json::from_value::<OrderBookL1Data>(event.data) {
                        metrics.process_orderbook(ob);
                    }
                }
                "open_interest" => {
                    if let Ok(oi) = serde_json::from_value::<OpenInterestData>(event.data) {
                        metrics.process_open_interest(oi);
                    }
                }
                _ => {}
            }
        }

        self.last_update = Instant::now();
    }
}

/// Metrics for a single ticker
#[derive(Debug)]
struct TickerMetrics {
    ticker: String,

    // Orderflow Imbalance
    orderflow: OrderflowMetrics,

    // Spot vs Perp Basis (placeholder - needs spot data)
    basis_usd: Option<f64>,
    basis_pct: Option<f64>,

    // Liquidation Clusters
    liquidations: Vec<LiquidationEvent>,
    liq_clusters: HashMap<i64, Vec<LiquidationEvent>>,

    // Funding Rate (placeholder - needs funding data)
    funding_rate: Option<f64>,

    // Whale Detector
    whale_trades: VecDeque<WhaleTrade>,

    // CVD Divergence
    cvd_history: VecDeque<CvdPoint>,
    price_history: VecDeque<PricePoint>,

    // Latest market data
    latest_price: Option<f64>,
    latest_spread: Option<f64>,
}

impl TickerMetrics {
    fn new(ticker: &str) -> Self {
        Self {
            ticker: ticker.to_uppercase(),
            orderflow: OrderflowMetrics::new(),
            basis_usd: None,
            basis_pct: None,
            liquidations: Vec::new(),
            liq_clusters: HashMap::new(),
            funding_rate: None,
            whale_trades: VecDeque::new(),
            cvd_history: VecDeque::new(),
            price_history: VecDeque::new(),
            latest_price: None,
            latest_spread: None,
        }
    }

    fn process_trade(&mut self, trade: TradeData, exchange: &str, time: DateTime<Utc>) {
        let volume_usd = trade.price * trade.amount;

        // Update orderflow
        self.orderflow.add_trade(&trade.side, volume_usd);

        // Check for whale trades
        if volume_usd >= WHALE_THRESHOLD {
            let whale = WhaleTrade {
                time,
                side: trade.side.clone(),
                volume_usd,
                price: trade.price,
                exchange: exchange.to_string(),
            };

            self.whale_trades.push_front(whale);
            if self.whale_trades.len() > MAX_WHALE_DISPLAY {
                self.whale_trades.pop_back();
            }
        }

        // Update price history for divergence detection
        self.latest_price = Some(trade.price);
        self.price_history.push_back(PricePoint {
            time,
            price: trade.price,
        });

        // Keep last 60 seconds
        let cutoff = Utc::now() - chrono::Duration::seconds(60);
        while let Some(point) = self.price_history.front() {
            if point.time < cutoff {
                self.price_history.pop_front();
            } else {
                break;
            }
        }
    }

    fn process_liquidation(&mut self, liq: LiquidationData, exchange: &str) {
        let volume_usd = liq.price * liq.quantity;

        let liq_event = LiquidationEvent {
            time: liq.time,
            side: liq.side,
            price: liq.price,
            volume_usd,
            exchange: exchange.to_string(),
        };

        // Add to raw liquidations
        self.liquidations.push(liq_event.clone());

        // Keep last 5 minutes only
        let cutoff = Utc::now() - chrono::Duration::minutes(5);
        self.liquidations.retain(|l| l.time >= cutoff);

        // Update clusters (bucket by $100 price levels)
        let bucket = (liq_event.price / LIQ_BUCKET_SIZE).floor() as i64;
        self.liq_clusters
            .entry(bucket)
            .or_insert_with(Vec::new)
            .push(liq_event);

        // Clean old clusters
        for cluster in self.liq_clusters.values_mut() {
            cluster.retain(|l| l.time >= cutoff);
        }
        self.liq_clusters.retain(|_, v| !v.is_empty());
    }

    fn process_cvd(&mut self, cvd: CvdData) {
        let cvd_point = CvdPoint {
            time: Utc::now(),
            delta: cvd.delta_quote,
        };

        self.cvd_history.push_back(cvd_point);

        // Keep last 60 seconds
        let cutoff = Utc::now() - chrono::Duration::seconds(60);
        while let Some(point) = self.cvd_history.front() {
            if point.time < cutoff {
                self.cvd_history.pop_front();
            } else {
                break;
            }
        }
    }

    fn process_orderbook(&mut self, ob: OrderBookL1Data) {
        if let Some(mid) = ob.mid_price() {
            self.latest_price = Some(mid.to_string().parse().unwrap_or(0.0));
        }
        self.latest_spread = ob.spread_percentage();
    }

    fn process_open_interest(&mut self, _oi: OpenInterestData) {
        // Could track OI changes here if needed
    }

    /// Calculate CVD divergence signal
    fn cvd_divergence(&self) -> DivergenceSignal {
        if self.price_history.len() < 2 || self.cvd_history.len() < 2 {
            return DivergenceSignal::Unknown;
        }

        // Get price trend (last 30s)
        let now = Utc::now();
        let cutoff = now - chrono::Duration::seconds(30);

        let recent_prices: Vec<_> = self
            .price_history
            .iter()
            .filter(|p| p.time >= cutoff)
            .collect();

        let recent_cvds: Vec<_> = self
            .cvd_history
            .iter()
            .filter(|c| c.time >= cutoff)
            .collect();

        if recent_prices.len() < 2 || recent_cvds.len() < 2 {
            return DivergenceSignal::Unknown;
        }

        // Calculate trends
        let price_trend = recent_prices.last().unwrap().price - recent_prices.first().unwrap().price;
        let cvd_trend = recent_cvds.last().unwrap().delta - recent_cvds.first().unwrap().delta;

        // Classify divergence
        const THRESHOLD: f64 = 0.0001; // Minimum movement to consider

        let price_up = price_trend > THRESHOLD;
        let price_down = price_trend < -THRESHOLD;
        let cvd_up = cvd_trend > THRESHOLD;
        let cvd_down = cvd_trend < -THRESHOLD;

        match (price_up, price_down, cvd_up, cvd_down) {
            (false, true, true, false) => DivergenceSignal::Bullish, // Price down, CVD up
            (true, false, false, true) => DivergenceSignal::Bearish, // Price up, CVD down
            (true, false, true, false) => DivergenceSignal::Aligned, // Both up
            (false, true, false, true) => DivergenceSignal::Aligned, // Both down
            _ => DivergenceSignal::Neutral,
        }
    }
}

/// Orderflow metrics for 1-minute window
#[derive(Debug)]
struct OrderflowMetrics {
    buy_volume: f64,
    sell_volume: f64,
    window_start: Instant,
}

impl OrderflowMetrics {
    fn new() -> Self {
        Self {
            buy_volume: 0.0,
            sell_volume: 0.0,
            window_start: Instant::now(),
        }
    }

    fn add_trade(&mut self, side: &Side, volume_usd: f64) {
        // Reset if window expired (1 minute)
        if self.window_start.elapsed() > Duration::from_secs(60) {
            self.buy_volume = 0.0;
            self.sell_volume = 0.0;
            self.window_start = Instant::now();
        }

        match side {
            Side::Buy => self.buy_volume += volume_usd,
            Side::Sell => self.sell_volume += volume_usd,
        }
    }

    fn total_volume(&self) -> f64 {
        self.buy_volume + self.sell_volume
    }

    fn imbalance_pct(&self) -> f64 {
        let total = self.total_volume();
        if total > 0.0 {
            (self.buy_volume / total) * 100.0
        } else {
            50.0
        }
    }

    fn net_flow(&self) -> f64 {
        self.buy_volume - self.sell_volume
    }

    fn trend_arrow(&self) -> &'static str {
        let imbalance = self.imbalance_pct();
        if imbalance >= 75.0 {
            "↑↑"
        } else if imbalance >= 60.0 {
            "↑"
        } else if imbalance <= 25.0 {
            "↓↓"
        } else if imbalance <= 40.0 {
            "↓"
        } else {
            "→"
        }
    }
}

/// Liquidation event with metadata
#[derive(Debug, Clone)]
struct LiquidationEvent {
    time: DateTime<Utc>,
    side: Side,
    price: f64,
    volume_usd: f64,
    exchange: String,
}

/// Whale trade (>$500K)
#[derive(Debug, Clone)]
struct WhaleTrade {
    time: DateTime<Utc>,
    side: Side,
    volume_usd: f64,
    price: f64,
    exchange: String,
}

/// CVD data point
#[derive(Debug, Clone)]
struct CvdPoint {
    time: DateTime<Utc>,
    delta: f64,
}

/// Price data point
#[derive(Debug, Clone)]
struct PricePoint {
    time: DateTime<Utc>,
    price: f64,
}

/// CVD divergence signal
#[derive(Debug, Clone, Copy, PartialEq)]
enum DivergenceSignal {
    Bullish,  // Price down, CVD up (accumulation)
    Bearish,  // Price up, CVD down (distribution)
    Aligned,  // Same direction
    Neutral,  // No clear trend
    Unknown,  // Not enough data
}

impl DivergenceSignal {
    fn display(&self) -> (&'static str, Color) {
        match self {
            DivergenceSignal::Bullish => ("BULLISH", Color::Green),
            DivergenceSignal::Bearish => ("BEARISH", Color::Red),
            DivergenceSignal::Aligned => ("ALIGNED", Color::Blue),
            DivergenceSignal::Neutral => ("NEUTRAL", Color::Yellow),
            DivergenceSignal::Unknown => ("---", Color::Gray),
        }
    }

    fn description(&self) -> &'static str {
        match self {
            DivergenceSignal::Bullish => "Price ↓ CVD ↑",
            DivergenceSignal::Bearish => "Price ↑ CVD ↓",
            DivergenceSignal::Aligned => "Price ≈ CVD",
            DivergenceSignal::Neutral => "Ranging",
            DivergenceSignal::Unknown => "N/A",
        }
    }
}

/// Render the main UI
fn render_ui(f: &mut ratatui::Frame, area: Rect, app: &AppState) {
    // Main layout: 3 rows
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(33),
            Constraint::Percentage(34),
        ])
        .split(area);

    // Row 1: Orderflow Imbalance | Spot vs Perp Basis
    let row1_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(rows[0]);

    render_orderflow_panel(f, row1_cols[0], app);
    render_basis_panel(f, row1_cols[1], app);

    // Row 2: Liquidation Clusters | Funding Momentum
    let row2_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(rows[1]);

    render_liquidation_panel(f, row2_cols[0], app);
    render_funding_panel(f, row2_cols[1], app);

    // Row 3: Whale Detector | CVD Divergence
    let row3_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(rows[2]);

    render_whale_panel(f, row3_cols[0], app);
    render_cvd_panel(f, row3_cols[1], app);
}

/// Panel 1: Orderflow Imbalance
fn render_orderflow_panel(f: &mut ratatui::Frame, area: Rect, app: &AppState) {
    let block = Block::default()
        .title(" ORDERFLOW IMBALANCE (1m) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = vec![];

    for ticker in TICKERS {
        let ticker_upper = ticker.to_uppercase();
        if let Some(metrics) = app.tickers.get(&ticker_upper) {
            let imbalance = metrics.orderflow.imbalance_pct();
            let net_flow = metrics.orderflow.net_flow();
            let trend = metrics.orderflow.trend_arrow();

            // Progress bar
            let filled = (imbalance / 10.0).round() as usize;
            let bar = format!(
                "[{}{}]",
                "█".repeat(filled.min(10)),
                "░".repeat(10 - filled.min(10))
            );

            // Color based on imbalance
            let color = if imbalance >= 60.0 {
                Color::Green
            } else if imbalance <= 40.0 {
                Color::Red
            } else {
                Color::Yellow
            };

            let line = Line::from(vec![
                Span::raw(format!("{:3}  ", ticker_upper)),
                Span::styled(bar, Style::default().fg(color)),
                Span::styled(
                    format!(" {:>3.0}% ", imbalance),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(if imbalance >= 50.0 { "BUY" } else { "SELL" }),
                Span::raw(format!(
                    "   Δ {:>+.1}M/min {}",
                    net_flow / 1_000_000.0,
                    trend
                )),
            ]);

            lines.push(line);
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Waiting for trade data...",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}

/// Panel 2: Spot vs Perp Basis
fn render_basis_panel(f: &mut ratatui::Frame, area: Rect, app: &AppState) {
    let block = Block::default()
        .title(" SPOT vs PERP BASIS ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = vec![Line::from(Span::styled(
        "Data not available",
        Style::default().fg(Color::DarkGray),
    ))];

    // Placeholder: would need spot price data
    // For now, show estimated basis from spread
    for ticker in TICKERS {
        let ticker_upper = ticker.to_uppercase();
        if let Some(metrics) = app.tickers.get(&ticker_upper) {
            if let (Some(_price), Some(spread_pct)) = (metrics.latest_price, metrics.latest_spread)
            {
                // Estimate basis from spread (placeholder)
                let est_basis_pct = spread_pct * 10.0; // Rough estimate
                let state = if est_basis_pct > 0.5 {
                    ("STEEP", Color::Red)
                } else if est_basis_pct > 0.0 {
                    ("CONTANGO", Color::Yellow)
                } else {
                    ("BACKWRD", Color::Blue)
                };

                lines = vec![Line::from(vec![
                    Span::raw(format!("{:3}  ", ticker_upper)),
                    Span::styled(
                        format!("{:>+.2}%  ", est_basis_pct),
                        Style::default().fg(state.1),
                    ),
                    Span::styled(state.0, Style::default().fg(state.1)),
                ])];
                break;
            }
        }
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}

/// Panel 3: Liquidation Clusters
fn render_liquidation_panel(f: &mut ratatui::Frame, area: Rect, app: &AppState) {
    let block = Block::default()
        .title(" LIQUIDATION CLUSTERS ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = vec![];

    // Find ticker with most recent liquidations
    let mut best_ticker = None;
    let mut max_liqs = 0;

    for (ticker, metrics) in &app.tickers {
        let total = metrics.liquidations.len();
        if total > max_liqs {
            max_liqs = total;
            best_ticker = Some((ticker, metrics));
        }
    }

    if let Some((ticker, metrics)) = best_ticker {
        // Group by price buckets and sort by volume
        let mut clusters: Vec<_> = metrics
            .liq_clusters
            .iter()
            .map(|(bucket, events)| {
                let price_level = (*bucket as f64) * LIQ_BUCKET_SIZE;
                let total_volume: f64 = events.iter().map(|e| e.volume_usd).sum();
                let long_count = events.iter().filter(|e| e.side == Side::Buy).count();
                let short_count = events.iter().filter(|e| e.side == Side::Sell).count();

                (price_level, total_volume, long_count, short_count)
            })
            .collect();

        clusters.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        lines.push(Line::from(Span::styled(
            format!("{} Liquidations:", ticker),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )));

        for (price, volume, longs, shorts) in
            clusters.iter().take(MAX_LIQ_CLUSTERS)
        {
            let danger = if *volume > 1_000_000.0 {
                " DANGER ZONE"
            } else {
                ""
            };

            let bar_width = (volume / 1_000_000.0).min(10.0) as usize;
            let bar = "█".repeat(bar_width);

            let color = if !danger.is_empty() {
                Color::Red
            } else {
                Color::Yellow
            };

            lines.push(Line::from(vec![
                Span::raw(format!("${:.1}K ", price / 1000.0)),
                Span::styled(format!("{:10}", bar), Style::default().fg(color)),
                Span::raw(format!(" ({} L, {} S)", longs, shorts)),
                Span::styled(danger, Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            ]));
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No liquidations detected",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}

/// Panel 4: Funding Momentum
fn render_funding_panel(f: &mut ratatui::Frame, area: Rect, _app: &AppState) {
    let block = Block::default()
        .title(" FUNDING MOMENTUM ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let lines = vec![Line::from(Span::styled(
        "Data not available",
        Style::default().fg(Color::DarkGray),
    ))];

    // Placeholder: would need funding rate data
    // Format would be:
    // BTC: 0.012% ↑↑ LONGS PAY
    // ETH: -0.008% ↓ SHORTS PAY
    // SOL: 0.045% ↑↑↑ EXTREME

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}

/// Panel 5: Whale Detector (>$500K)
fn render_whale_panel(f: &mut ratatui::Frame, area: Rect, app: &AppState) {
    let block = Block::default()
        .title(" WHALE DETECTOR (>$500K) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = vec![];

    // Collect all whale trades across tickers
    let mut all_whales: Vec<(&str, &WhaleTrade)> = vec![];
    for (ticker, metrics) in &app.tickers {
        for whale in &metrics.whale_trades {
            all_whales.push((ticker.as_str(), whale));
        }
    }

    // Sort by time (newest first)
    all_whales.sort_by(|a, b| b.1.time.cmp(&a.1.time));

    for (ticker, whale) in all_whales.iter().take(MAX_WHALE_DISPLAY) {
        let time_str = whale.time.format("%H:%M:%S");
        let side_color = if whale.side == Side::Buy {
            Color::Green
        } else {
            Color::Red
        };

        let mega_flag = if whale.volume_usd >= MEGA_WHALE_THRESHOLD {
            " ⚠️"
        } else {
            ""
        };

        let exchange_short = match whale.exchange.as_str() {
            "BinanceFuturesUsd" => "BNC",
            "Okx" => "OKX",
            "Bybit" => "BBT",
            _ => "???",
        };

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
                Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("@${:.1}K ", whale.price / 1000.0)),
            Span::styled(
                format!("[{}]", exchange_short),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(mega_flag, Style::default().fg(Color::Red)),
        ]));
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No whale trades detected",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}

/// Panel 6: CVD Divergence
fn render_cvd_panel(f: &mut ratatui::Frame, area: Rect, app: &AppState) {
    let block = Block::default()
        .title(" CVD DIVERGENCE ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let mut lines = vec![];

    for ticker in TICKERS {
        let ticker_upper = ticker.to_uppercase();
        if let Some(metrics) = app.tickers.get(&ticker_upper) {
            let signal = metrics.cvd_divergence();
            let (label, color) = signal.display();
            let desc = signal.description();

            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:3}: ", ticker_upper),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("{} ", desc)),
                Span::styled(
                    label,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Waiting for CVD data...",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}
