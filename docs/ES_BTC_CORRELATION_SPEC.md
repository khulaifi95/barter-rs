# ES/NQ vs BTC Correlation & Divergence Specification

## Overview

This document specifies how to calculate correlation, divergence, and lead/lag signals between traditional market indices (ES, NQ) and BTC for use in the barter-rs Scalper V2 TUI.

**Use Case**: BTC scalping using ES/NQ as leading indicators.

**Target**: Scalper V2 TUI binary (`scalper-v2`) - integrated directly, no separate script needed.

**Key Points**:
- All three symbols (ES, NQ, BTC) use 5-second bars aggregated from live trade/tick streams
- ES/NQ ticks come from `ibkr-bridge` WebSocket
- BTC ticks come from existing barter-data-server trade stream
- Panel shows ES/NQ prices, spread/divergence vs BTC, correlation, and lead/lag signals
- When you run `scalper-v2`, ES/NQ data appears automatically in the new panel

---

## Convention: ES as Base

Following institutional and TradingView standards, **ES (S&P 500 E-mini)** is used as the base/benchmark asset.

| Metric | Formula | Meaning |
|--------|---------|---------|
| `NQ/ES` | `NQ% - ES%` | NQ performance relative to market |
| `BTC/ES` | `BTC% - ES%` | BTC performance relative to market |

**Positive** = outperforming ES
**Negative** = underperforming ES (lagging)

---

## Data Sources

### ES/NQ Ticks (from ibkr-bridge)

ES/NQ tick data comes from `ibkr-bridge` via WebSocket:

```
Default: ws://127.0.0.1:8765/ws
Override: Set IBKR_BRIDGE_WS_URL environment variable
```

### BTC Ticks (from existing barter-data-server)

BTC trade data comes from the existing `barter-data-server` WebSocket feed that Scalper V2 already connects to. No additional connection needed - we tap into the existing trade stream.

```
Default: ws://127.0.0.1:9001 (via WS_URL env var)
```

### Messages (ibkr-bridge):

```json
// On connect: tick_backfill per symbol (300-500 ticks)
{"type": "tick_backfill", "symbol": "ES", "ticks": [...]}

// Live ticks (continuous)
{"ts": 1717000000000, "symbol": "ES", "type": "tick", "px": 5000.25, "sz": 1.0}
```

**Fields used for signals**: `px`, `sz`, `ts` only. Bid/ask fields are optional QA checks when present. L2/aggressor side not available from IBKR retail feed.

---

## Architecture: 5-Second Micro-Bars

**Problem**: Raw ticks are noisy for correlation calculations.

**Solution**: Aggregate ticks into 5-second micro-bars client-side in Rust for ALL THREE symbols.

```
Data Flow:
├── ibkr-bridge (ES/NQ ticks) ──┐
│                               ├──► 5s bar aggregators ──► TradMarketState ──► Correlation engine
└── barter-data-server (BTC trades) ─┘                              │
                                                                     ▼
                                                              render_trad_markets_panel
```

### All Symbols Use Same Aggregation:

| Symbol | Source | Aggregation |
|--------|--------|-------------|
| **ES** | ibkr-bridge ticks | 5s bars (client-side) |
| **NQ** | ibkr-bridge ticks | 5s bars (client-side) |
| **BTC** | barter-data-server trades | 5s bars (client-side) |

### Why 5-Second Bars:

| Approach | Noise Level | Speed | Recommendation |
|----------|-------------|-------|----------------|
| Raw ticks (250ms) | Very high | Fastest | ❌ Too noisy |
| **5-second bars** | Low | Fast enough | ✅ Use this |
| 1-minute bars | Very low | Too slow | ❌ Miss the move |

**Note**: Do NOT interpolate from 1m candles. Build 5s bars from live trade/tick streams for accuracy.

---

## Parameters (Finalized)

| Parameter | Value | Real Time | Notes |
|-----------|-------|-----------|-------|
| **Bar size** | 5 seconds | - | Aggregate ticks into OHLC |
| **Correlation window** | 12 bars | 60 seconds | Stable, still reactive |
| **Z-score history** | 60 bars | 5 minutes | For divergence σ calculation |
| **Lead/lag range** | ±6 bars | ±30 seconds | Cross-correlation sweep |
| **Display update** | 1 second | - | Human readable, no flicker |
| **Calculation update** | Every bar | 5 seconds | Internal state |
| **Alert display** | Immediate | - | Don't miss signals |

---

## Calculations

### 1. 5-Second Bar Aggregation

```rust
use std::time::{Duration, Instant};

/// Aggregates ticks into 5-second OHLC bars
pub struct MicroBarAggregator {
    bar_duration: Duration,
    current_bar_start: Option<Instant>,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: f64,
}

impl MicroBarAggregator {
    pub fn new() -> Self {
        Self {
            bar_duration: Duration::from_secs(5),
            current_bar_start: None,
            open: 0.0,
            high: f64::MIN,
            low: f64::MAX,
            close: 0.0,
            volume: 0.0,
        }
    }

    /// Returns Some(bar) when a 5-second bar completes
    pub fn update(&mut self, price: f64, size: f64, ts: i64) -> Option<MicroBar> {
        let now = Instant::now();

        match self.current_bar_start {
            None => {
                // Start first bar
                self.current_bar_start = Some(now);
                self.open = price;
                self.high = price;
                self.low = price;
                self.close = price;
                self.volume = size;
                None
            }
            Some(start) if now.duration_since(start) >= self.bar_duration => {
                // Bar complete - emit and start new
                let bar = MicroBar {
                    ts,
                    open: self.open,
                    high: self.high,
                    low: self.low,
                    close: self.close,
                    volume: self.volume,
                };

                // Reset for new bar
                self.current_bar_start = Some(now);
                self.open = price;
                self.high = price;
                self.low = price;
                self.close = price;
                self.volume = size;

                Some(bar)
            }
            Some(_) => {
                // Update current bar
                self.high = self.high.max(price);
                self.low = self.low.min(price);
                self.close = price;
                self.volume += size;
                None
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct MicroBar {
    pub ts: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}
```

### 2. Percentage Returns (Bar-to-Bar)

```rust
/// Calculate percentage return from bar closes
fn calc_return(bars: &[MicroBar]) -> f64 {
    if bars.len() < 2 {
        return 0.0;
    }
    let first = bars[0].close;
    let last = bars[bars.len() - 1].close;
    (last - first) / first
}

/// Calculate bar-to-bar returns for correlation
fn calc_bar_returns(bars: &[MicroBar]) -> Vec<f64> {
    bars.windows(2)
        .map(|w| (w[1].close - w[0].close) / w[0].close)
        .collect()
}
```

### 3. Spread (Simple % Difference)

```rust
/// BTC/ES spread - how far BTC is from ES
/// Positive = BTC outperforming, Negative = BTC lagging
fn calc_spread(btc_return: f64, es_return: f64) -> f64 {
    btc_return - es_return
}

// Example:
// ES: +0.28%, BTC: +0.04%
// Spread: 0.04 - 0.28 = -0.24% (BTC lagging)
```

### 4. Correlation (Pearson on Returns)

```rust
/// Pearson correlation coefficient on bar returns
/// Returns value from -1.0 to +1.0
fn calc_correlation(returns_a: &[f64], returns_b: &[f64]) -> f64 {
    if returns_a.len() != returns_b.len() || returns_a.len() < 5 {
        return 0.0;
    }

    let n = returns_a.len() as f64;
    let mean_a = returns_a.iter().sum::<f64>() / n;
    let mean_b = returns_b.iter().sum::<f64>() / n;

    let mut cov = 0.0;
    let mut var_a = 0.0;
    let mut var_b = 0.0;

    for i in 0..returns_a.len() {
        let diff_a = returns_a[i] - mean_a;
        let diff_b = returns_b[i] - mean_b;
        cov += diff_a * diff_b;
        var_a += diff_a * diff_a;
        var_b += diff_b * diff_b;
    }

    if var_a < 1e-10 || var_b < 1e-10 {
        return 0.0;
    }

    cov / (var_a.sqrt() * var_b.sqrt())
}
```

**Interpretation:**

| Correlation | Meaning | Color |
|-------------|---------|-------|
| > 0.70 | Strong - ES reliably predicts BTC | Green |
| 0.40 - 0.70 | Moderate - ES somewhat predictive | Yellow |
| < 0.40 | Weak - ES/BTC decoupled | Red |

### 5. Divergence Z-Score

```rust
/// Z-score of current spread vs rolling history
/// High |z| = significant divergence from normal
fn calc_divergence_zscore(
    current_spread: f64,
    spread_history: &VecDeque<f64>,
) -> f64 {
    if spread_history.len() < 20 {
        return 0.0;
    }

    let n = spread_history.len() as f64;
    let mean: f64 = spread_history.iter().sum::<f64>() / n;
    let variance: f64 = spread_history.iter()
        .map(|x| (x - mean).powi(2))
        .sum::<f64>() / n;
    let std = variance.sqrt();

    if std < 1e-10 {
        return 0.0;
    }

    (current_spread - mean) / std
}
```

**Interpretation:**

| Z-Score | Meaning | Action | Color |
|---------|---------|--------|-------|
| < -1.5 | BTC significantly lagging | Look for BTC long | Red (signal) |
| -1.5 to +1.5 | Normal range | No signal | Green |
| > +1.5 | BTC significantly leading | Look for BTC short | Red (signal) |

### 6. Lead/Lag Detection (Cross-Correlation)

```rust
/// Find which asset leads by testing correlation at different lags
/// Returns (lag_bars, correlation) where positive lag = ES leads
fn calc_lead_lag(
    es_returns: &[f64],
    btc_returns: &[f64],
    max_lag: usize,  // 6 bars = 30 seconds
) -> (i32, f64) {
    let mut best_lag = 0i32;
    let mut best_corr = 0.0f64;

    for lag in -(max_lag as i32)..=(max_lag as i32) {
        let corr = if lag < 0 {
            // BTC leads: shift BTC back
            let abs_lag = (-lag) as usize;
            if abs_lag >= btc_returns.len() { continue; }
            calc_correlation(
                &es_returns[abs_lag..],
                &btc_returns[..btc_returns.len() - abs_lag],
            )
        } else if lag > 0 {
            // ES leads: shift ES back
            let abs_lag = lag as usize;
            if abs_lag >= es_returns.len() { continue; }
            calc_correlation(
                &es_returns[..es_returns.len() - abs_lag],
                &btc_returns[abs_lag..],
            )
        } else {
            calc_correlation(es_returns, btc_returns)
        };

        if corr.abs() > best_corr.abs() {
            best_corr = corr;
            best_lag = lag;
        }
    }

    (best_lag, best_corr)
}

/// Convert lag in bars to seconds for display
fn lag_to_seconds(lag_bars: i32, bar_duration_secs: u64) -> i32 {
    lag_bars * bar_duration_secs as i32
}

// Example: lag = 1 bar at 5s/bar = "ES → BTC (~5s)"
```

---

## Data Structures

```rust
use std::collections::VecDeque;

/// Ring buffer for 5-second bars per symbol
pub struct BarBuffer {
    bars: VecDeque<MicroBar>,
    max_size: usize,
}

impl BarBuffer {
    pub fn new(max_size: usize) -> Self {
        Self {
            bars: VecDeque::with_capacity(max_size),
            max_size,
        }
    }

    pub fn push(&mut self, bar: MicroBar) {
        if self.bars.len() >= self.max_size {
            self.bars.pop_front();
        }
        self.bars.push_back(bar);
    }

    pub fn last_n(&self, n: usize) -> Vec<&MicroBar> {
        let start = self.bars.len().saturating_sub(n);
        self.bars.range(start..).collect()
    }

    pub fn returns(&self, n: usize) -> Vec<f64> {
        let bars: Vec<_> = self.last_n(n + 1);
        if bars.len() < 2 {
            return vec![];
        }
        bars.windows(2)
            .map(|w| (w[1].close - w[0].close) / w[0].close)
            .collect()
    }
}

/// Main state for ES/NQ/BTC correlation analysis
pub struct TradMarketState {
    // Bar aggregators (tick → 5s bar)
    es_aggregator: MicroBarAggregator,
    nq_aggregator: MicroBarAggregator,

    // Bar buffers (store last N bars)
    es_bars: BarBuffer,      // 60 bars = 5 minutes
    nq_bars: BarBuffer,
    btc_bars: BarBuffer,     // From existing crypto feed

    // Spread history for z-score
    spread_history: VecDeque<f64>,  // 60 samples = 5 minutes

    // Latest prices for display
    es_price: f64,
    nq_price: f64,
    btc_price: f64,

    // Computed signals (updated every bar)
    pub signals: CorrelationSignals,

    // Display throttle
    last_render: std::time::Instant,
}

impl TradMarketState {
    pub fn new() -> Self {
        Self {
            es_aggregator: MicroBarAggregator::new(),
            nq_aggregator: MicroBarAggregator::new(),
            es_bars: BarBuffer::new(60),
            nq_bars: BarBuffer::new(60),
            btc_bars: BarBuffer::new(60),
            spread_history: VecDeque::with_capacity(60),
            es_price: 0.0,
            nq_price: 0.0,
            btc_price: 0.0,
            signals: CorrelationSignals::default(),
            last_render: std::time::Instant::now(),
        }
    }

    /// Update with ES tick from ibkr-bridge
    pub fn update_es_tick(&mut self, price: f64, size: f64, ts: i64) {
        self.es_price = price;
        if let Some(bar) = self.es_aggregator.update(price, size, ts) {
            self.es_bars.push(bar);
            self.recompute_signals();
        }
    }

    /// Update with NQ tick from ibkr-bridge
    pub fn update_nq_tick(&mut self, price: f64, size: f64, ts: i64) {
        self.nq_price = price;
        if let Some(bar) = self.nq_aggregator.update(price, size, ts) {
            self.nq_bars.push(bar);
        }
    }

    /// Update with BTC bar from existing crypto feed
    pub fn update_btc_bar(&mut self, bar: MicroBar) {
        self.btc_price = bar.close;
        self.btc_bars.push(bar);
    }

    /// Recompute all signals (called when ES bar completes)
    fn recompute_signals(&mut self) {
        let window = 12;  // 12 bars = 60 seconds

        let es_returns = self.es_bars.returns(window);
        let nq_returns = self.nq_bars.returns(window);
        let btc_returns = self.btc_bars.returns(window);

        if es_returns.len() < 5 || btc_returns.len() < 5 {
            return;
        }

        // % returns over window
        let es_bars: Vec<_> = self.es_bars.last_n(window);
        let nq_bars: Vec<_> = self.nq_bars.last_n(window);
        let btc_bars: Vec<_> = self.btc_bars.last_n(window);

        let es_return = if es_bars.len() >= 2 {
            (es_bars.last().unwrap().close - es_bars[0].close) / es_bars[0].close
        } else { 0.0 };

        let nq_return = if nq_bars.len() >= 2 {
            (nq_bars.last().unwrap().close - nq_bars[0].close) / nq_bars[0].close
        } else { 0.0 };

        let btc_return = if btc_bars.len() >= 2 {
            (btc_bars.last().unwrap().close - btc_bars[0].close) / btc_bars[0].close
        } else { 0.0 };

        // Spreads
        let nq_es_spread = nq_return - es_return;
        let btc_es_spread = btc_return - es_return;

        // Update spread history for z-score
        if self.spread_history.len() >= 60 {
            self.spread_history.pop_front();
        }
        self.spread_history.push_back(btc_es_spread);

        // Correlations
        let es_nq_corr = calc_correlation(&es_returns, &nq_returns);
        let es_btc_corr = calc_correlation(&es_returns, &btc_returns);

        // Divergence z-score
        let divergence_z = calc_divergence_zscore(btc_es_spread, &self.spread_history);

        // Lead/lag (max 6 bars = 30 seconds)
        let (lead_lag_bars, _) = calc_lead_lag(&es_returns, &btc_returns, 6);
        let lead_lag_secs = lead_lag_bars * 5;  // Convert to seconds

        // EQ sync check
        let eq_sync = es_nq_corr > 0.85;

        self.signals = CorrelationSignals {
            es_price: self.es_price,
            nq_price: self.nq_price,
            btc_price: self.btc_price,
            es_return,
            nq_return,
            btc_return,
            nq_es_spread,
            btc_es_spread,
            es_nq_corr,
            es_btc_corr,
            divergence_z,
            lead_lag_bars,
            lead_lag_secs,
            eq_sync,
        };
    }

    /// Check if should render (1 second throttle)
    pub fn should_render(&mut self) -> bool {
        if self.last_render.elapsed() >= std::time::Duration::from_secs(1) {
            self.last_render = std::time::Instant::now();
            true
        } else {
            false
        }
    }
}

/// Computed signals for display
#[derive(Debug, Clone, Default)]
pub struct CorrelationSignals {
    // Prices
    pub es_price: f64,
    pub nq_price: f64,
    pub btc_price: f64,

    // Returns (over 60s window)
    pub es_return: f64,
    pub nq_return: f64,
    pub btc_return: f64,

    // Spreads
    pub nq_es_spread: f64,    // NQ - ES
    pub btc_es_spread: f64,   // BTC - ES (main signal)

    // Correlations
    pub es_nq_corr: f64,      // 0 to 1
    pub es_btc_corr: f64,     // 0 to 1

    // Divergence
    pub divergence_z: f64,    // Z-score

    // Lead/Lag
    pub lead_lag_bars: i32,   // Positive = ES leads
    pub lead_lag_secs: i32,   // In seconds for display

    // Derived
    pub eq_sync: bool,        // ES/NQ corr > 0.85
}
```

---

## Integration with Tokio Async Stack

Following existing barter-rs patterns (tokio + tokio_tungstenite + mpsc):

```rust
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

/// Messages from ibkr-bridge
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum IbkrMessage {
    #[serde(rename = "tick")]
    Tick {
        symbol: String,
        ts: i64,
        px: f64,
        sz: f64,
        #[serde(default)]
        bid: Option<f64>,
        #[serde(default)]
        ask: Option<f64>,
    },
    #[serde(rename = "tick_backfill")]
    TickBackfill {
        symbol: String,
        ticks: Vec<TickData>,
    },
}

#[derive(Debug, Deserialize)]
struct TickData {
    ts: i64,
    px: f64,
    sz: f64,
}

/// Spawn ibkr-bridge WebSocket handler
pub fn spawn_ibkr_feed(
    state: Arc<Mutex<TradMarketState>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let url = "ws://127.0.0.1:8765/ws";

        loop {
            match connect_async(url).await {
                Ok((ws_stream, _)) => {
                    log::info!("Connected to ibkr-bridge at {}", url);
                    let (_, mut read) = ws_stream.split();

                    while let Some(msg) = read.next().await {
                        match msg {
                            Ok(Message::Text(text)) => {
                                if let Ok(ibkr_msg) = serde_json::from_str::<IbkrMessage>(&text) {
                                    let mut state = state.lock().await;
                                    match ibkr_msg {
                                        IbkrMessage::Tick { symbol, ts, px, sz, .. } => {
                                            match symbol.as_str() {
                                                "ES" => state.update_es_tick(px, sz, ts),
                                                "NQ" => state.update_nq_tick(px, sz, ts),
                                                _ => {}
                                            }
                                        }
                                        IbkrMessage::TickBackfill { symbol, ticks } => {
                                            for tick in ticks {
                                                match symbol.as_str() {
                                                    "ES" => state.update_es_tick(tick.px, tick.sz, tick.ts),
                                                    "NQ" => state.update_nq_tick(tick.px, tick.sz, tick.ts),
                                                    _ => {}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Ok(Message::Close(_)) => {
                                log::warn!("ibkr-bridge connection closed");
                                break;
                            }
                            Err(e) => {
                                log::error!("ibkr-bridge error: {}", e);
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    log::error!("Failed to connect to ibkr-bridge: {}", e);
                }
            }

            // Reconnect after delay
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    })
}
```

---

## Display Panel (Scalper V2 TUI)

### Approved Layout:

```
┌─ TRAD MARKETS (ES/NQ) ── 5s bars, 60s window ───────────────────────────┐
│                                                                          │
│  ES  6050.25 ▲+0.28%  │  NQ  21450.75 ▲+0.35%  │  EQ: SYNC             │
│                                                                          │
├──────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  BTC vs MARKET                                                           │
│  ════════════════════════════════════════════════════════════════════   │
│                                                                          │
│  SPREAD:  BTC/ES  -0.24%        NQ/ES  +0.07%                           │
│           ████████████░░░░░░░░  (BTC lagging)                           │
│           -1%      0      +1%                                            │
│                                                                          │
│  DIV:     -1.8σ   ⚠ SIGNAL                                              │
│           ══════════════●═════                                           │
│           -2σ    -1σ    0    +1σ    +2σ                                 │
│                                                                          │
├──────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  CORRELATION (60s)    LEAD                                               │
│  ES/NQ:  0.94 ●       ES → BTC (~5s)                                    │
│  ES/BTC: 0.51 ●       NQ confirms ✓                                     │
│                                                                          │
├──────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ⚠ BTC LAGGING -1.8σ  │  EQ SYNC  │  LOOK LONG                         │
│                                                                          │
└──────────────────────────────────────────────────────────────────────────┘
```

### Ratatui Rendering:

```rust
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph},
    Frame,
};

pub fn render_trad_markets_panel(f: &mut Frame, area: Rect, signals: &CorrelationSignals) {
    let block = Block::default()
        .title(" TRAD MARKETS (ES/NQ) ── 5s bars, 60s window ")
        .borders(Borders::ALL);

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Split into sections
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),  // ES/NQ prices
            Constraint::Length(1),  // Separator
            Constraint::Length(6),  // BTC vs Market
            Constraint::Length(1),  // Separator
            Constraint::Length(3),  // Correlation/Lead
            Constraint::Length(1),  // Separator
            Constraint::Length(2),  // Signal line
        ])
        .split(inner);

    // Row 1: ES/NQ prices + EQ sync
    let es_color = if signals.es_return >= 0.0 { Color::Green } else { Color::Red };
    let nq_color = if signals.nq_return >= 0.0 { Color::Green } else { Color::Red };
    let eq_color = if signals.eq_sync { Color::Green } else { Color::Yellow };

    let es_arrow = if signals.es_return >= 0.0 { "▲" } else { "▼" };
    let nq_arrow = if signals.nq_return >= 0.0 { "▲" } else { "▼" };

    let header = Line::from(vec![
        Span::raw("  ES  "),
        Span::styled(format!("{:.2}", signals.es_price), Style::default().fg(Color::White)),
        Span::raw(" "),
        Span::styled(format!("{}{:+.2}%", es_arrow, signals.es_return * 100.0), Style::default().fg(es_color)),
        Span::raw("  │  NQ  "),
        Span::styled(format!("{:.2}", signals.nq_price), Style::default().fg(Color::White)),
        Span::raw(" "),
        Span::styled(format!("{}{:+.2}%", nq_arrow, signals.nq_return * 100.0), Style::default().fg(nq_color)),
        Span::raw("  │  EQ: "),
        Span::styled(
            if signals.eq_sync { "SYNC" } else { "MIXED" },
            Style::default().fg(eq_color).add_modifier(Modifier::BOLD)
        ),
    ]);
    f.render_widget(Paragraph::new(header), chunks[0]);

    // Row 2: BTC vs Market section
    let spread_pct = signals.btc_es_spread * 100.0;
    let spread_color = if spread_pct.abs() < 0.15 { Color::Green }
                       else if spread_pct.abs() < 0.25 { Color::Yellow }
                       else { Color::Red };

    let btc_vs_market = vec![
        Line::from("  BTC vs MARKET"),
        Line::from("  ════════════════════════════════════════════"),
        Line::from(vec![
            Span::raw("  SPREAD:  BTC/ES  "),
            Span::styled(format!("{:+.2}%", spread_pct), Style::default().fg(spread_color)),
            Span::raw("        NQ/ES  "),
            Span::styled(format!("{:+.2}%", signals.nq_es_spread * 100.0), Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::raw("           "),
            Span::styled(render_spread_bar(spread_pct), Style::default().fg(spread_color)),
            Span::raw(if spread_pct < 0.0 { "  (BTC lagging)" } else { "  (BTC leading)" }),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw("  DIV:     "),
            Span::styled(
                format!("{:+.1}σ", signals.divergence_z),
                Style::default().fg(if signals.divergence_z.abs() > 1.5 { Color::Red } else { Color::Green })
            ),
            if signals.divergence_z.abs() > 1.5 {
                Span::styled("   ⚠ SIGNAL", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
            } else {
                Span::raw("")
            },
        ]),
    ];
    f.render_widget(Paragraph::new(btc_vs_market), chunks[2]);

    // Row 3: Correlation/Lead
    let es_nq_color = if signals.es_nq_corr > 0.85 { Color::Green }
                      else if signals.es_nq_corr > 0.70 { Color::Yellow }
                      else { Color::Red };
    let es_btc_color = if signals.es_btc_corr > 0.50 { Color::Green }
                       else if signals.es_btc_corr > 0.30 { Color::Yellow }
                       else { Color::Red };

    let lead_text = if signals.lead_lag_secs > 0 {
        format!("ES → BTC (~{}s)", signals.lead_lag_secs)
    } else if signals.lead_lag_secs < 0 {
        format!("BTC → ES (~{}s)", -signals.lead_lag_secs)
    } else {
        "SYNC".to_string()
    };

    let corr_lead = vec![
        Line::from(vec![
            Span::raw("  CORRELATION (60s)    LEAD"),
        ]),
        Line::from(vec![
            Span::raw("  ES/NQ:  "),
            Span::styled(format!("{:.2}", signals.es_nq_corr), Style::default().fg(es_nq_color)),
            Span::raw(" ●       "),
            Span::styled(lead_text, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::raw("  ES/BTC: "),
            Span::styled(format!("{:.2}", signals.es_btc_corr), Style::default().fg(es_btc_color)),
            Span::raw(" ●       "),
            Span::styled(
                if signals.eq_sync { "NQ confirms ✓" } else { "NQ diverging ✗" },
                Style::default().fg(if signals.eq_sync { Color::Green } else { Color::Yellow })
            ),
        ]),
    ];
    f.render_widget(Paragraph::new(corr_lead), chunks[4]);

    // Row 4: Signal line
    let signal_text = if signals.divergence_z < -1.5 && signals.eq_sync {
        ("⚠ BTC LAGGING", "LOOK LONG", Color::Green)
    } else if signals.divergence_z > 1.5 && signals.eq_sync {
        ("⚠ BTC LEADING", "LOOK SHORT", Color::Red)
    } else {
        ("  NEUTRAL", "NO SIGNAL", Color::White)
    };

    let signal_line = Line::from(vec![
        Span::raw("  "),
        Span::styled(signal_text.0, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::raw(format!(" {:.1}σ  │  EQ ", signals.divergence_z)),
        Span::styled(
            if signals.eq_sync { "SYNC" } else { "MIXED" },
            Style::default().fg(if signals.eq_sync { Color::Green } else { Color::Yellow })
        ),
        Span::raw("  │  "),
        Span::styled(signal_text.1, Style::default().fg(signal_text.2).add_modifier(Modifier::BOLD)),
    ]);
    f.render_widget(Paragraph::new(signal_line), chunks[6]);
}

/// Render spread bar visualization
fn render_spread_bar(spread_pct: f64) -> String {
    // -1% to +1% range, 20 chars wide
    let normalized = ((spread_pct / 1.0) + 1.0) / 2.0;  // 0.0 to 1.0
    let position = (normalized * 20.0).clamp(0.0, 20.0) as usize;

    let mut bar = String::new();
    for i in 0..20 {
        if i < position {
            bar.push('█');
        } else {
            bar.push('░');
        }
    }
    bar
}
```

---

## Color Coding Summary

| Element | Green | Yellow | Red |
|---------|-------|--------|-----|
| ES/NQ Correlation | > 0.85 (SYNC) | 0.70-0.85 | < 0.70 (MIXED) |
| ES/BTC Correlation | > 0.50 | 0.30-0.50 | < 0.30 |
| BTC/ES Spread | \|spread\| < 0.15% | 0.15-0.25% | > 0.25% |
| Divergence Z | \|z\| < 1.0 | 1.0-1.5 | \|z\| > 1.5 (SIGNAL) |

---

## Signal Interpretation

### For BTC Scalping:

| Condition | Signal | Confidence |
|-----------|--------|------------|
| ES up, BTC flat, DIV < -1.5σ, EQ SYNC | **Long BTC** | High |
| ES down, BTC flat, DIV > +1.5σ, EQ SYNC | **Short BTC** | High |
| ES up, BTC flat, EQ MIXED | Wait | Low - noisy signal |
| DIV in normal range (-1.5 to +1.5) | No signal | - |

### EQ SYNC Check:
- ES/NQ correlation > 0.85 = **SYNC** (clean signal, trade with confidence)
- ES/NQ correlation < 0.85 = **MIXED** (equities diverging, reduce size or wait)

---

## Integration into Scalper V2

### No Separate Script Needed

The ES/NQ/BTC correlation panel is integrated directly into the `scalper-v2` binary. When you run `cargo run --bin scalper-v2`, it will:

1. Connect to `ibkr-bridge` WebSocket (default `ws://127.0.0.1:8765/ws`)
2. Connect to `barter-data-server` WebSocket (existing connection)
3. Aggregate ES/NQ ticks and BTC trades into 5s bars
4. Compute correlation/divergence signals
5. Display in the new TRAD MARKETS panel

### Layout Integration

The Scalper V2 layout splits the whale pane area horizontally (50/50):

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ Header: BTC price, status, metrics                                           │
├──────────────────────────────────────────────────────────────────────────────┤
│ BIG SIGNAL: Buy/Sell pressure                                                │
├──────────────────────────────────────────────────────────────────────────────┤
│ NET FLOW                    │ BOOK (L2)                                      │
├──────────────────────────────────────────────────────────────────────────────┤
│ EXCHANGES (30s)             │ VOL                                            │
├─────────────────────────────┬────────────────────────────────────────────────┤
│ WHALES (existing)           │ TRAD MARKETS (ES/NQ) ← NEW PANEL               │
│                             │                                                │
│                             │  ES/NQ prices, spread, divergence,             │
│                             │  correlation, lead/lag signals                 │
├─────────────────────────────┴────────────────────────────────────────────────┤
│ Footer: [B]TC [E]TH [S]OL │ SCALPER V2 │ [q] Quit                           │
└──────────────────────────────────────────────────────────────────────────────┘
```

### File Structure for Implementation

```
barter-trading-tuis/
└── src/
    ├── shared/
    │   └── trad_markets/        # NEW MODULE
    │       ├── mod.rs           # Module exports
    │       ├── state.rs         # TradMarketState, CorrelationSignals
    │       ├── aggregator.rs    # MicroBarAggregator, BarBuffer (for ES/NQ/BTC)
    │       ├── calc.rs          # calc_correlation, calc_divergence_zscore, calc_lead_lag
    │       ├── feed.rs          # spawn_ibkr_feed (WebSocket handler for ES/NQ)
    │       └── widget.rs        # render_trad_markets_panel
    └── bin/
        └── scalper_v2.rs        # MODIFIED: add TradMarketState, spawn feed, render panel
```

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `IBKR_BRIDGE_WS_URL` | `ws://127.0.0.1:8765/ws` | ibkr-bridge WebSocket URL |
| `WS_URL` | `ws://127.0.0.1:9001` | barter-data-server WebSocket URL (existing) |

---

## References

- [NQ vs ES - TradingView](https://www.tradingview.com/script/A4oxDv7z-NQ-vs-ES/)
- [Pairs Trading Basics - IBKR Quant](https://www.interactivebrokers.com/campus/ibkr-quant-news/pairs-trading-basics-correlation-cointegration-and-strategy-part-i/)
- [ICT SMT Divergence](https://innercircletrader.net/tutorials/ict-smt-divergence-smart-money-technique/)
- [BTC/SPX Correlation - Coinbase Institutional](https://www.coinbase.com/institutional/research-insights/research/monthly-outlook/monthly-outlook-august-2024)

---

## Changelog

- 2024-12-09: Initial specification created
- 2024-12-09: Updated with 5s micro-bars, 60s window, 1s render throttle
- 2024-12-09: Added complete Rust implementation code
- 2024-12-09: Added Ratatui widget code for Scalper V2
- 2024-12-09: Added file structure for implementation
- 2024-12-09: Clarified BTC uses 5s bars from live trades (not 1m candles)
- 2024-12-09: Added integration details - no separate script, runs inside scalper-v2
- 2024-12-09: Added layout diagram showing 50/50 split with whale pane
- 2024-12-09: Added environment variable documentation
