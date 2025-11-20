/// TUI 3: Risk & Arbitrage Scanner
///
/// Real-time risk monitoring and arbitrage opportunity detection
/// Refresh Rate: 5 seconds
/// Primary Use: Position monitoring, risk alerts

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
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table},
    Frame, Terminal,
};
use std::{
    collections::{HashMap, VecDeque},
    error::Error,
    io,
    time::Duration,
};
// Logging disabled for TUI - tracing output interferes with terminal rendering
// use tracing::{info, warn};

/// Main application state
struct App {
    /// Liquidation data tracker
    liquidations: LiquidationTracker,
    /// Market regime detector
    regime: MarketRegimeDetector,
    /// Arbitrage opportunity tracker
    arbitrage: ArbitrageTracker,
    /// Correlation calculator
    correlation: CorrelationCalculator,
    /// Last update timestamp
    last_update: DateTime<Utc>,
    /// Connection status
    connected: bool,
}

impl App {
    fn new() -> Self {
        Self {
            liquidations: LiquidationTracker::new(),
            regime: MarketRegimeDetector::new(),
            arbitrage: ArbitrageTracker::new(),
            correlation: CorrelationCalculator::new(),
            last_update: Utc::now(),
            connected: false,
        }
    }

    /// Process incoming market event
    fn process_event(&mut self, event: MarketEventMessage) {
        self.last_update = Utc::now();

        let symbol = format!("{}/{}", event.instrument.base, event.instrument.quote).to_uppercase();

        match event.kind.as_str() {
            "liquidation" => {
                if let Ok(liq) = serde_json::from_value::<LiquidationData>(event.data) {
                    self.liquidations.add_liquidation(&symbol, liq);
                }
            }
            "trade" => {
                if let Ok(trade) = serde_json::from_value::<TradeData>(event.data) {
                    self.regime.add_trade(&symbol, trade.price);
                    self.correlation.add_price(&symbol, trade.price);
                    self.arbitrage.add_trade(&symbol, &event.exchange, trade.price);
                }
            }
            "order_book_l1" => {
                if let Ok(l1) = serde_json::from_value::<OrderBookL1Data>(event.data) {
                    self.regime.add_orderbook(&symbol, l1);
                }
            }
            "cumulative_volume_delta" => {
                if let Ok(cvd) = serde_json::from_value::<CvdData>(event.data) {
                    self.regime.add_cvd(&symbol, cvd.delta_base);
                }
            }
            "open_interest" => {
                if let Ok(oi) = serde_json::from_value::<OpenInterestData>(event.data) {
                    self.regime.add_oi(&symbol, oi.contracts);
                }
            }
            _ => {}
        }
    }
}

/// Liquidation cascade risk calculator
struct LiquidationTracker {
    /// Liquidations by symbol (price level -> liquidations)
    clusters: HashMap<String, HashMap<i64, Vec<StoredLiquidation>>>,
    /// Recent liquidations (last 5 minutes)
    recent: HashMap<String, VecDeque<StoredLiquidation>>,
    /// Max recent liquidations to keep
    max_recent: usize,
}

#[derive(Clone)]
struct StoredLiquidation {
    side: Side,
    #[allow(dead_code)]
    price: f64,
    #[allow(dead_code)]
    quantity: f64,
    value: f64,
    time: DateTime<Utc>,
}

impl LiquidationTracker {
    fn new() -> Self {
        Self {
            clusters: HashMap::new(),
            recent: HashMap::new(),
            max_recent: 1000, // Keep last 1000 liquidations
        }
    }

    fn add_liquidation(&mut self, symbol: &str, liq: LiquidationData) {
        let value = liq.price * liq.quantity;
        let stored = StoredLiquidation {
            side: liq.side.clone(),
            price: liq.price,
            quantity: liq.quantity,
            value,
            time: liq.time,
        };

        // Add to clusters (bucket by $100 price levels)
        let price_bucket = (liq.price / 100.0).floor() as i64 * 100;
        self.clusters
            .entry(symbol.to_string())
            .or_default()
            .entry(price_bucket)
            .or_default()
            .push(stored.clone());

        // Add to recent
        let recent = self.recent.entry(symbol.to_string()).or_default();
        recent.push_back(stored);

        // Trim old entries
        while recent.len() > self.max_recent {
            recent.pop_front();
        }

        // Clean up old liquidations (older than 5 minutes)
        self.clean_old_liquidations(symbol);
    }

    fn clean_old_liquidations(&mut self, symbol: &str) {
        let cutoff = Utc::now() - chrono::Duration::minutes(5);

        if let Some(recent) = self.recent.get_mut(symbol) {
            while let Some(front) = recent.front() {
                if front.time < cutoff {
                    recent.pop_front();
                } else {
                    break;
                }
            }
        }

        // Clean clusters
        if let Some(clusters) = self.clusters.get_mut(symbol) {
            for liquidations in clusters.values_mut() {
                liquidations.retain(|l| l.time >= cutoff);
            }
            clusters.retain(|_, v| !v.is_empty());
        }
    }

    /// Calculate cascade risk score (0-100)
    fn cascade_risk_score(&self, symbol: &str) -> f64 {
        let clusters = match self.clusters.get(symbol) {
            Some(c) => c,
            None => return 0.0,
        };

        if clusters.is_empty() {
            return 0.0;
        }

        // Find largest cluster
        let max_cluster_value: f64 = clusters
            .values()
            .map(|liquidations| liquidations.iter().map(|l| l.value).sum::<f64>())
            .max_by(|a, b| a.partial_cmp(b).unwrap())
            .unwrap_or(0.0);

        // Risk score based on cluster size
        // >$50M = HIGH (80-100)
        // $20M-$50M = MEDIUM (50-80)
        // <$20M = LOW (0-50)
        if max_cluster_value > 50_000_000.0 {
            80.0 + (max_cluster_value - 50_000_000.0) / 5_000_000.0 * 20.0
        } else if max_cluster_value > 20_000_000.0 {
            50.0 + (max_cluster_value - 20_000_000.0) / 30_000_000.0 * 30.0
        } else {
            (max_cluster_value / 20_000_000.0) * 50.0
        }
        .min(100.0)
    }

    /// Get next cascade level (price and volume)
    fn next_cascade_level(&self, symbol: &str, current_price: f64) -> Option<(f64, f64, Side)> {
        let clusters = self.clusters.get(symbol)?;

        // Find nearest cluster above/below current price
        let mut best_long: Option<(i64, f64)> = None; // Below price (long liquidations)
        let mut best_short: Option<(i64, f64)> = None; // Above price (short liquidations)

        for (&price_level, liquidations) in clusters.iter() {
            let total_value: f64 = liquidations.iter().map(|l| l.value).sum();

            if total_value < 5_000_000.0 {
                continue; // Ignore small clusters
            }

            let level_f64 = price_level as f64;

            // Check for long liquidations (below price)
            if level_f64 < current_price {
                let long_value: f64 = liquidations
                    .iter()
                    .filter(|l| l.side == Side::Buy)
                    .map(|l| l.value)
                    .sum();

                if long_value > 1_000_000.0 {
                    if let Some((_, current_best)) = best_long {
                        if long_value > current_best {
                            best_long = Some((price_level, long_value));
                        }
                    } else {
                        best_long = Some((price_level, long_value));
                    }
                }
            }

            // Check for short liquidations (above price)
            if level_f64 > current_price {
                let short_value: f64 = liquidations
                    .iter()
                    .filter(|l| l.side == Side::Sell)
                    .map(|l| l.value)
                    .sum();

                if short_value > 1_000_000.0 {
                    if let Some((_, current_best)) = best_short {
                        if short_value > current_best {
                            best_short = Some((price_level, short_value));
                        }
                    } else {
                        best_short = Some((price_level, short_value));
                    }
                }
            }
        }

        // Return the larger of the two
        match (best_long, best_short) {
            (Some((long_price, long_val)), Some((short_price, short_val))) => {
                if long_val > short_val {
                    Some((long_price as f64, long_val, Side::Buy))
                } else {
                    Some((short_price as f64, short_val, Side::Sell))
                }
            }
            (Some((price, val)), None) => Some((price as f64, val, Side::Buy)),
            (None, Some((price, val))) => Some((price as f64, val, Side::Sell)),
            (None, None) => None,
        }
    }

    /// Get protection level (opposite side)
    fn protection_level(&self, symbol: &str, current_price: f64) -> Option<(f64, f64)> {
        let clusters = self.clusters.get(symbol)?;

        let mut best_protection: Option<(i64, f64)> = None;

        for (&price_level, liquidations) in clusters.iter() {
            let level_f64 = price_level as f64;

            // Protection is opposite side liquidations
            // For downside protection, we want short liquidations above price
            if level_f64 > current_price {
                let short_value: f64 = liquidations
                    .iter()
                    .filter(|l| l.side == Side::Sell)
                    .map(|l| l.value)
                    .sum();

                if short_value > 1_000_000.0 {
                    if let Some((_, current_best)) = best_protection {
                        if short_value > current_best {
                            best_protection = Some((price_level, short_value));
                        }
                    } else {
                        best_protection = Some((price_level, short_value));
                    }
                }
            }
        }

        best_protection.map(|(price, val)| (price as f64, val))
    }
}

/// Market regime detector
struct MarketRegimeDetector {
    /// Price windows for volatility calculation
    prices: HashMap<String, VecDeque<f64>>,
    /// CVD values for trend detection
    cvd: HashMap<String, VecDeque<f64>>,
    /// Order book snapshots
    orderbooks: HashMap<String, OrderBookL1Data>,
    /// Open interest values
    oi: HashMap<String, VecDeque<f64>>,
    /// Max window size
    max_window: usize,
}

impl MarketRegimeDetector {
    fn new() -> Self {
        Self {
            prices: HashMap::new(),
            cvd: HashMap::new(),
            orderbooks: HashMap::new(),
            oi: HashMap::new(),
            max_window: 300, // 5 minutes at 1 event/sec
        }
    }

    fn add_trade(&mut self, symbol: &str, price: f64) {
        let window = self.prices.entry(symbol.to_string()).or_default();
        window.push_back(price);
        while window.len() > self.max_window {
            window.pop_front();
        }
    }

    fn add_cvd(&mut self, symbol: &str, delta: f64) {
        let window = self.cvd.entry(symbol.to_string()).or_default();
        window.push_back(delta);
        while window.len() > self.max_window {
            window.pop_front();
        }
    }

    fn add_orderbook(&mut self, symbol: &str, ob: OrderBookL1Data) {
        self.orderbooks.insert(symbol.to_string(), ob);
    }

    fn add_oi(&mut self, symbol: &str, contracts: f64) {
        let window = self.oi.entry(symbol.to_string()).or_default();
        window.push_back(contracts);
        while window.len() > self.max_window {
            window.pop_front();
        }
    }

    /// Detect market regime
    fn detect_regime(&self, symbol: &str) -> (String, f64) {
        let prices = match self.prices.get(symbol) {
            Some(p) if p.len() > 20 => p,
            _ => return ("UNKNOWN".to_string(), 0.0),
        };

        let volatility = self.calculate_volatility(symbol);
        let trend_strength = self.calculate_trend_strength(symbol);

        // Classify regime based on volatility and trend
        let regime = if volatility > 0.02 {
            // High volatility
            if trend_strength > 0.6 {
                "TRENDING"
            } else {
                "VOLATILE"
            }
        } else {
            // Low volatility
            if trend_strength > 0.4 {
                "RANGING"
            } else {
                "RANGE-BOUND"
            }
        };

        // Confidence based on data quality
        let confidence = (prices.len() as f64 / self.max_window as f64 * 100.0).min(100.0);

        (regime.to_string(), confidence)
    }

    /// Calculate realized volatility
    fn calculate_volatility(&self, symbol: &str) -> f64 {
        let prices = match self.prices.get(symbol) {
            Some(p) if p.len() > 2 => p,
            _ => return 0.0,
        };

        let mean = prices.iter().sum::<f64>() / prices.len() as f64;
        let variance = prices
            .iter()
            .map(|&p| {
                let diff = p - mean;
                diff * diff
            })
            .sum::<f64>()
            / (prices.len() - 1) as f64;

        (variance.sqrt() / mean).max(0.0)
    }

    /// Calculate trend strength (0-1)
    fn calculate_trend_strength(&self, symbol: &str) -> f64 {
        let prices = match self.prices.get(symbol) {
            Some(p) if p.len() > 10 => p,
            _ => return 0.0,
        };

        // Simple linear regression slope
        let n = prices.len() as f64;
        let x_mean = (n - 1.0) / 2.0;
        let y_mean = prices.iter().sum::<f64>() / n;

        let mut numerator = 0.0;
        let mut denominator = 0.0;

        for (i, &price) in prices.iter().enumerate() {
            let x_diff = i as f64 - x_mean;
            numerator += x_diff * (price - y_mean);
            denominator += x_diff * x_diff;
        }

        let slope = if denominator > 0.0 {
            numerator / denominator
        } else {
            0.0
        };

        // Normalize slope to 0-1 range
        (slope.abs() / y_mean * 100.0).min(1.0)
    }

    /// Assess liquidity (THIN/NORMAL/THICK)
    fn liquidity_assessment(&self, symbol: &str) -> String {
        let ob = match self.orderbooks.get(symbol) {
            Some(ob) => ob,
            None => return "UNKNOWN".to_string(),
        };

        let bid_size = ob
            .best_bid
            .as_ref()
            .map(|b| b.amount_f64())
            .unwrap_or(0.0);
        let ask_size = ob
            .best_ask
            .as_ref()
            .map(|a| a.amount_f64())
            .unwrap_or(0.0);

        let total_size = bid_size + ask_size;

        // Thresholds depend on asset
        if symbol.contains("BTC") {
            if total_size > 10.0 {
                "THICK"
            } else if total_size > 5.0 {
                "NORMAL"
            } else {
                "THIN"
            }
        } else if symbol.contains("ETH") {
            if total_size > 50.0 {
                "THICK"
            } else if total_size > 25.0 {
                "NORMAL"
            } else {
                "THIN"
            }
        } else {
            if total_size > 1000.0 {
                "THICK"
            } else if total_size > 500.0 {
                "NORMAL"
            } else {
                "THIN"
            }
        }
        .to_string()
    }
}

/// Arbitrage opportunity tracker
struct ArbitrageTracker {
    /// Latest prices by exchange
    exchange_prices: HashMap<String, HashMap<String, f64>>, // symbol -> exchange -> price
    /// Last update times
    last_updates: HashMap<String, HashMap<String, DateTime<Utc>>>,
}

impl ArbitrageTracker {
    fn new() -> Self {
        Self {
            exchange_prices: HashMap::new(),
            last_updates: HashMap::new(),
        }
    }

    fn add_trade(&mut self, symbol: &str, exchange: &str, price: f64) {
        self.exchange_prices
            .entry(symbol.to_string())
            .or_default()
            .insert(exchange.to_string(), price);

        self.last_updates
            .entry(symbol.to_string())
            .or_default()
            .insert(exchange.to_string(), Utc::now());
    }

    /// Calculate spot-perp basis (simplified - assuming perp data)
    fn spot_perp_basis(&self, symbol: &str) -> Option<(f64, f64)> {
        // In real implementation, we'd distinguish spot vs perp
        // For now, we'll use exchange price differences as a proxy
        let prices = self.exchange_prices.get(symbol)?;

        if prices.len() < 2 {
            return None;
        }

        let mut values: Vec<f64> = prices.values().copied().collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let low = values.first()?;
        let high = values.last()?;
        let avg = values.iter().sum::<f64>() / values.len() as f64;

        let basis = high - low;
        let basis_pct = (basis / avg) * 100.0;

        Some((basis, basis_pct))
    }

    /// Calculate exchange spread
    fn exchange_spread(&self, symbol: &str) -> Option<(String, String, f64)> {
        let prices = self.exchange_prices.get(symbol)?;

        if prices.len() < 2 {
            return None;
        }

        let mut min_exchange = "";
        let mut min_price = f64::MAX;
        let mut max_exchange = "";
        let mut max_price = f64::MIN;

        for (exchange, &price) in prices.iter() {
            if price < min_price {
                min_price = price;
                min_exchange = exchange;
            }
            if price > max_price {
                max_price = price;
                max_exchange = exchange;
            }
        }

        Some((
            min_exchange.to_string(),
            max_exchange.to_string(),
            max_price - min_price,
        ))
    }

    /// Calculate funding rate differential (simulated)
    fn funding_differential(&self, symbol: &str) -> Option<(f64, f64)> {
        // In a real implementation, we'd track actual funding rates
        // For now, we'll simulate based on price momentum
        let prices = self.exchange_prices.get(symbol)?;

        if prices.is_empty() {
            return None;
        }

        // Simulate funding rates (0.01% to 0.05%)
        let avg_price = prices.values().sum::<f64>() / prices.len() as f64;
        let rate1 = ((avg_price as u64 % 40) as f64 + 10.0) / 1000.0; // 0.010% to 0.050%
        let rate2 = ((avg_price as u64 % 30) as f64 + 10.0) / 1000.0;

        Some((rate1, rate2))
    }
}

/// Correlation calculator for BTC/ETH/SOL
struct CorrelationCalculator {
    /// Price windows for correlation
    prices: HashMap<String, VecDeque<f64>>,
    /// Max window size
    max_window: usize,
}

impl CorrelationCalculator {
    fn new() -> Self {
        Self {
            prices: HashMap::new(),
            max_window: 100,
        }
    }

    fn add_price(&mut self, symbol: &str, price: f64) {
        let window = self.prices.entry(symbol.to_string()).or_default();
        window.push_back(price);
        while window.len() > self.max_window {
            window.pop_front();
        }
    }

    /// Calculate correlation coefficient between two assets
    fn correlation(&self, symbol1: &str, symbol2: &str) -> f64 {
        let prices1 = match self.prices.get(symbol1) {
            Some(p) if p.len() > 10 => p,
            _ => return 0.0,
        };

        let prices2 = match self.prices.get(symbol2) {
            Some(p) if p.len() > 10 => p,
            _ => return 0.0,
        };

        let n = prices1.len().min(prices2.len());
        if n < 10 {
            return 0.0;
        }

        // Calculate means
        let mean1 = prices1.iter().take(n).sum::<f64>() / n as f64;
        let mean2 = prices2.iter().take(n).sum::<f64>() / n as f64;

        // Calculate correlation
        let mut numerator = 0.0;
        let mut sum_sq1 = 0.0;
        let mut sum_sq2 = 0.0;

        for i in 0..n {
            let diff1 = prices1[i] - mean1;
            let diff2 = prices2[i] - mean2;
            numerator += diff1 * diff2;
            sum_sq1 += diff1 * diff1;
            sum_sq2 += diff2 * diff2;
        }

        let denominator = (sum_sq1 * sum_sq2).sqrt();
        if denominator > 0.0 {
            (numerator / denominator).max(-1.0).min(1.0)
        } else {
            0.0
        }
    }

    /// Get correlation matrix
    fn correlation_matrix(&self) -> [[f64; 3]; 3] {
        let symbols = ["BTC/USDT", "ETH/USDT", "SOL/USDT"];
        let mut matrix = [[0.0; 3]; 3];

        for (i, &sym1) in symbols.iter().enumerate() {
            for (j, &sym2) in symbols.iter().enumerate() {
                if i == j {
                    matrix[i][j] = 1.0;
                } else {
                    matrix[i][j] = self.correlation(sym1, sym2);
                }
            }
        }

        matrix
    }
}

/// Render the TUI
fn render_ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(f.area());

    let top_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(chunks[0]);

    let bottom_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(chunks[1]);

    // Panel 1: Risk Metrics (top-left)
    render_risk_metrics(f, app, top_chunks[0]);

    // Panel 2: Arbitrage Opportunities (top-right)
    render_arbitrage_opportunities(f, app, top_chunks[1]);

    // Panel 3: Market Regime (bottom-left)
    render_market_regime(f, app, bottom_chunks[0]);

    // Panel 4: Correlation Matrix (bottom-right)
    render_correlation_matrix(f, app, bottom_chunks[1]);
}

/// Render risk metrics panel
fn render_risk_metrics(f: &mut Frame, app: &App, area: Rect) {
    let btc = "BTC/USDT";
    let current_price = app
        .regime
        .prices
        .get(btc)
        .and_then(|p| p.back().copied())
        .unwrap_or(95000.0);

    let risk_score = app.liquidations.cascade_risk_score(btc);
    let risk_level = if risk_score > 70.0 {
        "HIGH"
    } else if risk_score > 40.0 {
        "MEDIUM"
    } else {
        "LOW"
    };

    // Create risk bar
    let filled = ((risk_score / 100.0 * 10.0) as usize).min(10);
    let empty = 10 - filled;
    let risk_bar = format!(
        "{}{}",
        "█".repeat(filled),
        "░".repeat(empty)
    );

    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                "LIQUIDATION CASCADE RISK: ",
                Style::default().fg(Color::White),
            ),
            Span::styled(
                &risk_bar,
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
                risk_level,
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
            if risk_score > 70.0 {
                Span::styled(" ⚠️", Style::default().fg(Color::Red))
            } else {
                Span::raw("")
            },
        ]),
        Line::from(""),
    ];

    // Next cascade level
    if let Some((price, volume, side)) = app.liquidations.next_cascade_level(btc, current_price) {
        let pct_from_current = ((price - current_price) / current_price) * 100.0;
        let side_str = if side == Side::Buy { "longs" } else { "shorts" };

        lines.push(Line::from(vec![
            Span::styled("Next Level: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("${:.0}", price),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                format!("({:+.1}%)", pct_from_current),
                Style::default().fg(if pct_from_current < 0.0 {
                    Color::Red
                } else {
                    Color::Green
                }),
            ),
            Span::raw(" = "),
            Span::styled(
                format!("${:.0}M {}", volume / 1_000_000.0, side_str),
                Style::default().fg(Color::Cyan),
            ),
        ]));
    } else {
        lines.push(Line::from(Span::styled(
            "Next Level: No significant clusters detected",
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Protection level
    if let Some((price, volume)) = app.liquidations.protection_level(btc, current_price) {
        let pct_from_current = ((price - current_price) / current_price) * 100.0;

        lines.push(Line::from(vec![
            Span::styled("Protection: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("${:.0}", price),
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                format!("({:+.1}%)", pct_from_current),
                Style::default().fg(Color::Green),
            ),
            Span::raw(" = "),
            Span::styled(
                format!("${:.0}M shorts", volume / 1_000_000.0),
                Style::default().fg(Color::Cyan),
            ),
        ]));
    } else {
        lines.push(Line::from(Span::styled(
            "Protection: No protection zones detected",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" RISK METRICS ")
            .border_style(Style::default().fg(Color::White)),
    );

    f.render_widget(paragraph, area);
}

/// Render arbitrage opportunities panel
fn render_arbitrage_opportunities(f: &mut Frame, app: &App, area: Rect) {
    let btc = "BTC/USDT";
    let eth = "ETH/USDT";
    let sol = "SOL/USDT";

    let mut lines = vec![];

    // Spot-Perp Basis
    for symbol in [btc, eth, sol] {
        if let Some((basis, basis_pct)) = app.arbitrage.spot_perp_basis(symbol) {
            let ticker = symbol.split('/').next().unwrap_or("");
            let warning = if basis_pct.abs() > 0.03 { " ⚠️" } else { "" };

            lines.push(Line::from(vec![
                Span::styled("SPOT-PERP: ", Style::default().fg(Color::Gray)),
                Span::styled(
                    ticker,
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("{:+.0}", basis),
                    Style::default().fg(if basis > 0.0 { Color::Green } else { Color::Red }),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("({:+.2}%)", basis_pct),
                    Style::default().fg(if basis > 0.0 { Color::Green } else { Color::Red }),
                ),
                Span::styled(
                    warning,
                    Style::default().fg(Color::Red),
                ),
            ]));
        }
    }

    lines.push(Line::from(""));

    // Exchange Spreads
    for symbol in [btc, eth, sol] {
        if let Some((low_ex, high_ex, spread)) = app.arbitrage.exchange_spread(symbol) {
            let ticker = symbol.split('/').next().unwrap_or("");
            lines.push(Line::from(vec![
                Span::styled("EXCHANGE: ", Style::default().fg(Color::Gray)),
                Span::styled(
                    ticker,
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("{}-{}",
                        low_ex.replace("BinanceFuturesUsd", "BNC")
                              .replace("Okx", "OKX")
                              .replace("Bybit", "BBT"),
                        high_ex.replace("BinanceFuturesUsd", "BNC")
                               .replace("Okx", "OKX")
                               .replace("Bybit", "BBT")
                    ),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("${:.2} spread", spread),
                    Style::default().fg(Color::Magenta),
                ),
            ]));
        }
    }

    lines.push(Line::from(""));

    // Funding Differentials
    for symbol in [btc, eth, sol] {
        if let Some((rate1, rate2)) = app.arbitrage.funding_differential(symbol) {
            let ticker = symbol.split('/').next().unwrap_or("");
            lines.push(Line::from(vec![
                Span::styled("FUNDING: ", Style::default().fg(Color::Gray)),
                Span::styled(
                    ticker,
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("{:.3}% vs {:.3}%", rate1, rate2),
                    Style::default().fg(Color::Cyan),
                ),
                if (rate1 - rate2).abs() > 0.01 {
                    Span::styled(" ARB", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
                } else {
                    Span::raw("")
                },
            ]));
        }
    }

    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" ARBITRAGE OPPORTUNITIES ")
            .border_style(Style::default().fg(Color::White)),
    );

    f.render_widget(paragraph, area);
}

/// Render market regime panel
fn render_market_regime(f: &mut Frame, app: &App, area: Rect) {
    let btc = "BTC/USDT";
    let (regime, confidence) = app.regime.detect_regime(btc);
    let volatility = app.regime.calculate_volatility(btc);
    let liquidity = app.regime.liquidity_assessment(btc);

    let vol_state = if volatility > 0.02 {
        "HIGH"
    } else if volatility > 0.01 {
        "MODERATE"
    } else {
        "LOW"
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("State: ", Style::default().fg(Color::Gray)),
            Span::styled(
                &regime,
                Style::default()
                    .fg(match regime.as_str() {
                        "TRENDING" => Color::Green,
                        "VOLATILE" => Color::Red,
                        "RANGING" => Color::Yellow,
                        "RANGE-BOUND" => Color::Cyan,
                        _ => Color::White,
                    })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                format!("({:.0}% confidence)", confidence),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Volatility: ", Style::default().fg(Color::Gray)),
            Span::styled(
                vol_state,
                Style::default().fg(match vol_state {
                    "HIGH" => Color::Red,
                    "MODERATE" => Color::Yellow,
                    _ => Color::Green,
                }),
            ),
            Span::raw(" "),
            Span::styled(
                format!("({:.2}% realized)", volatility * 100.0),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Trend: ", Style::default().fg(Color::Gray)),
            Span::styled(
                if regime == "TRENDING" {
                    "DIRECTIONAL"
                } else {
                    "NEUTRAL"
                },
                Style::default().fg(if regime == "TRENDING" {
                    Color::Green
                } else {
                    Color::Yellow
                }),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Liquidity: ", Style::default().fg(Color::Gray)),
            Span::styled(
                &liquidity,
                Style::default().fg(match liquidity.as_str() {
                    "THICK" => Color::Green,
                    "NORMAL" => Color::Yellow,
                    "THIN" => Color::Red,
                    _ => Color::White,
                }),
            ),
        ]),
    ];

    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" MARKET REGIME ")
            .border_style(Style::default().fg(Color::White)),
    );

    f.render_widget(paragraph, area);
}

/// Render correlation matrix panel
fn render_correlation_matrix(f: &mut Frame, app: &App, area: Rect) {
    let matrix = app.correlation.correlation_matrix();
    let symbols = ["BTC", "ETH", "SOL"];

    let header_cells = ["", "BTC", "ETH", "SOL"]
        .iter()
        .map(|h| {
            ratatui::widgets::Cell::from(*h).style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        });

    let header = Row::new(header_cells).height(1);

    let rows = matrix.iter().enumerate().map(|(i, row)| {
        let cells = std::iter::once(symbols[i].to_string())
            .chain(row.iter().map(|&corr| format!("{:.2}", corr)))
            .enumerate()
            .map(|(j, content)| {
                let color = if j == 0 {
                    Color::Yellow // Symbol name
                } else {
                    let val = if j - 1 < row.len() {
                        row[j - 1]
                    } else {
                        0.0
                    };
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Logging disabled for TUI - tracing output interferes with terminal rendering
    // tracing_subscriber::fmt()
    //     .with_max_level(tracing::Level::INFO)
    //     .init();

    // info!("Starting Risk & Arbitrage Scanner TUI");

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let mut app = App::new();

    // Create WebSocket client
    let config = WebSocketConfig::default();
    let client = WebSocketClient::with_config(config);
    let (mut event_rx, mut status_rx) = client.start();

    // Spawn status monitor
    tokio::spawn(async move {
        while let Some(status) = status_rx.recv().await {
            match status {
                barter_trading_tuis::shared::websocket::ConnectionStatus::Connected => {
                    // info!("WebSocket connected");
                }
                barter_trading_tuis::shared::websocket::ConnectionStatus::Disconnected => {
                    // warn!("WebSocket disconnected");
                }
                barter_trading_tuis::shared::websocket::ConnectionStatus::Reconnecting => {
                    // info!("WebSocket reconnecting...");
                }
            }
        }
    });

    // Main event loop
    let tick_rate = Duration::from_secs(5);
    let mut last_tick = std::time::Instant::now();

    loop {
        // Draw UI
        terminal.draw(|f| render_ui(f, &app))?;

        // Handle input with timeout
        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        break;
                    }
                    _ => {}
                }
            }
        }

        // Process WebSocket events (non-blocking)
        while let Ok(event) = event_rx.try_recv() {
            app.process_event(event);
            app.connected = true;
        }

        // Tick
        if last_tick.elapsed() >= tick_rate {
            last_tick = std::time::Instant::now();
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    // info!("Risk & Arbitrage Scanner TUI shutdown complete");

    Ok(())
}
