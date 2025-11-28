# Whale Detection System - Technical Deep Dive

## Architecture Overview

The whale detection system has 3 layers:

```
Layer 1: Event Processing [state.rs:199-284]
         ↓ All events processed here
         
Layer 2: Whale Aggregation [state.rs:409-503]
         ↓ Size filtering, buffer management
         
Layer 3: Snapshot & Display [state.rs:629-675]
         ↓ UI rendering prep
         
Layer 4: Render [market_microstructure.rs:399-475]
         ↓ Terminal display
```

---

## Layer 1: Event Processing

### File: `barter-trading-tuis/src/shared/state.rs`
### Method: `Aggregator::process_event()`
### Lines: 199-284

```rust
pub fn process_event(&mut self, event: MarketEventMessage) {
    let ticker = event.instrument.base.to_uppercase();
    let kind = event.instrument.kind.to_lowercase();
    let is_spot = kind.contains("spot");
    let is_perp = kind.contains("perp");

    let state = self
        .tickers
        .entry(ticker.clone())
        .or_insert_with(|| TickerState::new(ticker.clone()));

    match event.kind.as_str() {
        "trade" => {
            if let Ok(trade) = serde_json::from_value::<TradeData>(event.data) {
                state.push_trade(
                    trade,
                    &event.exchange,
                    event.time_exchange,
                    is_spot,
                    is_perp,
                );
            }
        }
        // ... other event types
    }
```

**Key Points:**
- ALL events passed to state.push_trade()
- No pre-filtering by exchange or size
- Market type (spot/perp) detected from instrument.kind

### Exchange Detection
Event.exchange values:
- "Okx"
- "BinanceFuturesUsd"
- "BinanceSpot"
- "BybitPerpetualsUsd"
- "BybitSpot"

---

## Layer 2: Whale Aggregation & Filtering

### File: `barter-trading-tuis/src/shared/state.rs`
### Method: `TickerState::push_trade()`
### Lines: 409-503

### Step 1: Calculate Notional (Line 417)
```rust
let usd = trade.price * trade.amount;
```

Simple multiplication: price * quantity = notional value in USD.

**Example:**
- BTC $50K, qty 2 BTC = $100K notional
- BTC $50K, qty 10 BTC = $500K notional

### Step 2: Size Filtering (Lines 437-464)

```rust
// Whale threshold (USD notional)
if usd >= whale_threshold() {
    self.whales.push_front(WhaleRecord {
        time,
        side: side.clone(),
        volume_usd: usd,
        price: trade.price,
        exchange: exchange.to_string(),
        market_kind: if is_spot {
            "SPOT".to_string()
        } else if is_perp {
            "PERP".to_string()
        } else {
            "OTHER".to_string()
        },
    });
    while self.whales.len() > max_whales() {
        self.whales.pop_back();
    }
    self.last_whale_exchange = Some(exchange.to_string());
    self.last_whale_kind = Some(...);
}
```

**Critical Section: Lines 438-455**

The entire whale filtering logic is in these 18 lines:

**Line 438: The Size Filter**
```rust
if usd >= whale_threshold() {
```

This is THE ONLY gate controlling whale detection:
- Default: $500,000 (line 28)
- Trades below $500K: ignored
- Trades above $500K: added to whale buffer

**Line 439: Push to Front**
```rust
self.whales.push_front(WhaleRecord {
```

Pushes newest whale to FRONT of VecDeque.

**Lines 453-455: FIFO Eviction**
```rust
while self.whales.len() > max_whales() {
    self.whales.pop_back();
}
```

The critical eviction logic:
- While buffer exceeds max (default 500)
- Drop oldest (pop_back removes from back)
- Newest always stay in buffer

**Eviction Example:**

```
Initial state: 499 whales in buffer
[newest] ← front
   ...
   [oldest] ← back

New OKX whale arrives:
push_front(new_whale) → buffer now 500

Check: 500 > max_whales() (500)? 
Answer: NO, don't evict

Next OKX whale arrives:
push_front(next_whale) → buffer now 501

Check: 501 > max_whales() (500)?
Answer: YES, evict!
pop_back() → remove oldest whale
Buffer now 500 again
```

### Step 3: Exchange Volume Tracking (Lines 432-435)

```rust
self.exchange_volume
    .push_back((time, exchange.to_string(), usd));
self.last_trade_by_exchange
    .insert(exchange.to_string(), trade.price);
```

ALL trades recorded here, regardless of size. This is separate from whale buffer.

### Step 4: CVD Calculation for Perpetuals (Lines 473-500)

```rust
if is_perp {
    let signed_quote = match side {
        Side::Buy => usd,
        Side::Sell => -usd,
    };
    *self
        .cvd_from_trades
        .entry(exchange.to_string())
        .or_insert(0.0) += signed_quote;
    // ... CVD history tracking ...
}
```

ALL perpetual trades contribute to CVD, regardless of size.

---

## Layer 3: Snapshot Creation

### File: `barter-trading-tuis/src/shared/state.rs`
### Method: `TickerState::to_snapshot()`
### Lines: 629-675

```rust
fn to_snapshot(&self) -> TickerSnapshot {
    let orderflow_1m = self.orderflow(60);
    let orderflow_5m = self.orderflow(300);
    let exchange_dominance = self.exchange_dominance(60);
    let vwap_1m = self.vwap(60);
    let vwap_5m = self.vwap(300);
    let whales: Vec<WhaleRecord> = self.whales.iter().cloned().take(20).collect();
    // ...
}
```

**Critical Line 635: The Display Filter**
```rust
let whales: Vec<WhaleRecord> = self.whales.iter().cloned().take(20).collect();
```

Takes only the first 20 whales from the VecDeque.

Since whales are stored in push_front order (newest at front), take(20) gets:
- Newest 20 whales
- All others (positions 21-500) discarded from snapshot

---

## Layer 4: Rendering

### File: `barter-trading-tuis/src/bin/market_microstructure.rs`
### Function: `render_whale_panel()`
### Lines: 399-475

```rust
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
        // ... render each whale row ...
    }
}
```

**Rendering Pipeline:**
1. Collect all whales from all tickers
2. Sort by time (newest first)
3. Take only enough to fit display
4. Format and render

---

## Critical Data Structures

### WhaleRecord (Lines 142-150)
```rust
#[derive(Clone, Debug)]
pub struct WhaleRecord {
    pub time: DateTime<Utc>,
    pub side: Side,
    pub volume_usd: f64,
    pub price: f64,
    pub exchange: String,
    pub market_kind: String,
}
```

No aggregation - each trade is a separate record.

### TickerState (Lines 364-383)
```rust
struct TickerState {
    ticker: String,
    trades: VecDeque<TradeRecord>,
    whales: VecDeque<WhaleRecord>,         // ← Per-ticker buffer
    liquidations: VecDeque<LiquidationRecord>,
    cvd_by_exchange: HashMap<String, f64>,
    cvd_from_trades: HashMap<String, f64>,
    cvd_history: VecDeque<CvdRecord>,
    oi_by_exchange: HashMap<String, f64>,
    // ... other fields ...
}
```

Key insight: **Each ticker has its own separate whale VecDeque**.

So BTC whales don't compete with ETH whales for buffer space.

---

## Configuration Functions

### whale_threshold() (Lines 21-30)
```rust
fn whale_threshold() -> f64 {
    static WHALE_THRESHOLD: OnceLock<f64> = OnceLock::new();
    *WHALE_THRESHOLD.get_or_init(|| {
        std::env::var("WHALE_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(500_000.0)
    })
}
```

Uses OnceLock for lazy initialization:
- Reads env var on first call
- Caches result for subsequent calls
- Default: $500,000

### max_whales() (Lines 32-41)
```rust
fn max_whales() -> usize {
    static MAX_WHALES: OnceLock<usize> = OnceLock::new();
    *MAX_WHALES.get_or_init(|| {
        std::env::var("MAX_WHALES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(500)
    })
}
```

Same pattern:
- Reads env var on first call
- Default: 500

### liq_danger_threshold() (Lines 43-52)
```rust
fn liq_danger_threshold() -> f64 {
    static LIQ_DANGER: OnceLock<f64> = OnceLock::new();
    *LIQ_DANGER.get_or_init(|| {
        std::env::var("LIQ_DANGER_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1_000_000.0)
    })
}
```

Used for liquidation cascade risk calculation, not whale filtering.

---

## Debug Logging

### File: `barter-trading-tuis/src/shared/state.rs`
### Lines: 250-283

```rust
// Debug: track whales per exchange when a whale was added
if let Some(ticker_state) = self.tickers.get(&ticker) {
    if let Some(kind) = ticker_state.last_whale(&event.exchange) {
        let counters = self
            .whale_counts
            .entry(event.exchange.clone())
            .or_insert_with(WhaleCounters::default);
        counters.total += 1;
        match kind {
            "SPOT" => counters.spot += 1,
            "PERP" => counters.perp += 1,
            _ => counters.other += 1,
        }
    }
}

// Periodically log whale distribution (debug; can be removed later)
let now = Utc::now();
if (now - self.last_whale_log).num_seconds() >= 30 {
    let mut counts: Vec<_> = self.whale_counts.iter().collect();
    counts.sort_by(|a, b| b.1.total.cmp(&a.1.total));
    let summary: Vec<String> = counts
        .iter()
        .map(|(ex, c)| {
            format!(
                "{}:{} (spot {} / perp {} / other {})",
                ex, c.total, c.spot, c.perp, c.other
            )
        })
        .collect();
    println!("[whale-debug] last 30s whales: {}", summary.join(", "));
    self.whale_counts.clear();
    self.last_whale_log = now;
}
```

**What happens:**
1. Every time a whale is added, exchange counter incremented
2. Every 30 seconds, print summary sorted by total count
3. Reset counters for next 30-second window

**Output example:**
```
[whale-debug] last 30s whales: Okx:534 (spot 200 / perp 334 / other 0), BinanceFuturesUsd:12 (spot 0 / perp 12 / other 0), BybitPerpetualsUsd:8 (spot 0 / perp 8 / other 0)
```

**Interpretation:**
- Okx: 534 whales in last 30 sec
- BinanceFuturesUsd: 12 whales in last 30 sec
- BybitPerpetualsUsd: 8 whales in last 30 sec

If Okx > 480, buffer is overflowing every 30 seconds.

---

## Pruning Strategy

### File: `barter-trading-tuis/src/shared/state.rs`
### Method: `TickerState::prune()`
### Lines: 583-627

```rust
fn prune(&mut self, now: DateTime<Utc>) {
    let trade_cutoff = now - ChronoDuration::seconds(TRADE_RETENTION_SECS);
    while let Some(front) = self.trades.front() {
        if front.time < trade_cutoff {
            self.trades.pop_front();
        } else {
            break;
        }
    }
    // ... similar for exchange_volume ...
    
    let liq_cutoff = now - ChronoDuration::seconds(LIQ_RETENTION_SECS);
    while let Some(front) = self.liquidations.front() {
        if front.time < liq_cutoff {
            self.liquidations.pop_front();
        } else {
            break;
        }
    }
    // ... similar for cvd_history, price_history ...
}
```

**Critical:** Whale buffer is NOT explicitly pruned by time!

Whales are only removed by:
1. FIFO eviction when buffer exceeds max_whales()
2. Implicit: very old whales eventually pushed out by newer ones

---

## Comparison: Why OI/Liq Work But Whales Don't

### Liquidations
- Data structure: `VecDeque<LiquidationRecord>` (no size limit)
- Filter: None (all liquidations retained)
- Retention: 10 minutes (LIQ_RETENTION_SECS)
- Display: Top clusters by USD value
- Result: ALL liquidations visible

### Open Interest
- Data structure: `HashMap<String, f64>` (per-exchange)
- Filter: None
- Retention: Permanent (never pruned)
- Display: Sum of all exchanges
- Result: Latest OI always visible

### Whales
- Data structure: `VecDeque<WhaleRecord>` (size limit 500)
- Filter: Size >= $500K
- Retention: 15 minutes (time-based) + FIFO (space-based)
- Display: Newest 20 only
- Result: May be hidden by buffer overflow + display limit

---

## Performance Implications

### Memory Usage
- 500 whales * 3 tickers = 1500 WhaleRecord objects max
- Per WhaleRecord: ~100 bytes = ~150 KB worst case
- Negligible memory footprint

### CPU Usage
- O(n) for FIFO eviction where n = buffer size
- O(n log n) for whale sort in render_whale_panel
- Called every 250 ms (market_microstructure draw_interval)
- Negligible CPU impact

### Issue: Not Performance, But Design
Buffer overflow is a DESIGN issue, not a performance issue.

---

## Summary Table: All 3 Filtering Layers

| Layer | Type | Code | Impact |
|-------|------|------|--------|
| 1. Size Filter | Hard gate | `state.rs:438` | Trades < $500K never enter buffer |
| 2. FIFO Eviction | Soft limit | `state.rs:454` | Oldest whales dropped at overflow |
| 3. Display Limit | UI filter | `state.rs:635` | Only newest 20 shown |

All three must pass for a trade to appear on screen.

Failing any one = invisible trade.

