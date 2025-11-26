# TUI AGGREGATION REFACTOR - COMPREHENSIVE PLAN
**Version:** 1.0
**Date:** 2025-11-22
**Lead:** Codex (Sonnet assisting with docs/tests)
**Goal:** Single shared aggregation engine feeding all three Opus TUIs with consistent metrics

---

## 🎯 EXECUTIVE SUMMARY

### Problem Statement
Three TUI binaries (market-microstructure, institutional-flow, risk-scanner) each maintain separate state/caches, leading to:
- **Inconsistent metrics** across TUIs (liquidation clusters differ, dominance calculations vary)
- **Duplicated logic** (3 implementations of orderflow, 2 of liquidation clustering)
- **Fragile architecture** (per-event logging spam, Utc::now() pruning bug, backpressure)
- **Dead code** (state.rs exists with 930 lines but is not exported or used)

### Solution
Centralize all aggregation in `shared/state.rs`, export it, and convert TUIs to thin renderers consuming snapshots.

### Success Criteria
- ✅ All 3 TUIs use same Aggregator instance
- ✅ Liquidation clusters identical across TUIs
- ✅ No per-event logging
- ✅ Event timestamps used for pruning (not Utc::now())
- ✅ Basis uses real spot/perp mids (not simulated)
- ✅ Unit tests pass for all metrics
- ✅ 24-hour uptime test with no crashes/memory leaks

### Estimated Timeline
- **Phase 1-3:** 2-3 days (architecture, wiring, metrics)
- **Phase 4:** 0.5-1 day (testing)
- **Phase 5:** 0.25 day (docs)
- **Total:** 3-4 days with buffer

---

## 📊 CURRENT STATE ANALYSIS

### Verified Facts (From Agent Analysis)

**state.rs Export Status:**
- ❌ NOT in `shared/mod.rs` (only aggregation, types, websocket)
- ❌ NOT in `lib.rs` exports
- ❌ ZERO imports in any TUI binary
- ✅ 930 lines of working code (but unused)

**Per-TUI State Duplication:**

| TUI | State Structures | Lines | Processes Liqs? |
|-----|-----------------|-------|-----------------|
| market_microstructure | HashMap<String, TickerMetrics> | 284-377 | YES |
| institutional_flow | HashMap<String, InstrumentData> | 40-74 | NO |
| risk_scanner | 4 separate trackers | 34-685 | YES |

**Timestamp Handling Bug:**
- All 3 TUIs use `Utc::now()` for pruning windows
- state.rs also uses `Utc::now()` (lines 322, 396, 410, 455-498)
- Event timestamps (`time_exchange`, `time_received`) ignored
- **Impact:** Windows drift when network latency changes

**Logging Spam:**
- market_microstructure: Active eprintln! in dispatcher (lines 108-157)
- institutional_flow: None (clean)
- risk_scanner: All logging commented out (clean)

**Data Flow:**
```
Server → WebSocket → [TUI1 cache] → Metrics → Render
                  ├→ [TUI2 cache] → Metrics → Render
                  └→ [TUI3 cache] → Metrics → Render

state.rs (unused) → ∅
```

---

## 🏗️ PHASE 1: AGGREGATION ENGINE CONSOLIDATION

**Owner:** Codex
**Estimated Time:** 8-12 hours
**Dependencies:** None

### Task 1.1: Fix Timestamp Handling in state.rs

**File:** `barter-trading-tuis/src/shared/state.rs`

**Current Code (Lines 314-342):**
```rust
fn push_trade(
    &mut self,
    trade: TradeData,
    exchange: &str,
    _time: DateTime<Utc>,  // ❌ IGNORED parameter
    is_spot: bool,
    is_perp: bool,
) {
    let now = Utc::now();  // ❌ Uses system time
    let record = TradeRecord {
        time: now,  // ❌ Should use event timestamp
        ...
    }
    ...
}
```

**Required Change:**
```rust
fn push_trade(
    &mut self,
    trade: TradeData,
    exchange: &str,
    event_time: DateTime<Utc>,  // ✅ Use this
    is_spot: bool,
    is_perp: bool,
) {
    let record = TradeRecord {
        time: event_time,  // ✅ Event timestamp
        ...
    }
    ...
    self.prune(event_time);  // ✅ Prune based on event time
}
```

**Apply to ALL methods:**
- `push_liquidation(...)` - Lines 395-407
- `push_cvd(...)` - Lines 409-421
- `push_orderbook(...)` - Lines 428-453

**Prune Method (Lines 455-499):**
```rust
// Change signature from:
fn prune(&mut self, now: DateTime<Utc>)

// To:
fn prune(&mut self, event_time: DateTime<Utc>)

// And use event_time for all cutoff calculations:
let trade_cutoff = event_time - ChronoDuration::seconds(TRADE_RETENTION_SECS);
let liq_cutoff = event_time - ChronoDuration::seconds(LIQ_RETENTION_SECS);
// etc.
```

**Acceptance Test:**
```rust
#[test]
fn test_event_timestamp_pruning() {
    let mut aggregator = Aggregator::new();

    // Add event at T0
    let t0 = Utc::now();
    aggregator.process_event(make_trade_event("BTC", 95000.0, t0));

    // Add event at T0 + 20 minutes (simulating delayed processing)
    let t20 = t0 + chrono::Duration::minutes(20);
    aggregator.process_event(make_trade_event("BTC", 96000.0, t20));

    let snapshot = aggregator.snapshot();
    let btc = &snapshot.tickers["BTC"];

    // T0 trade should be pruned (15min retention)
    // T20 trade should exist
    assert_eq!(btc.orderflow_1m.trades_per_sec > 0.0, true);
}
```

**Deliverable:**
- [ ] All `Utc::now()` replaced with `event_time` parameter
- [ ] `prune()` uses event timestamps
- [ ] Unit test passes

---

### Task 1.2: Export state.rs in Module System

**File:** `barter-trading-tuis/src/shared/mod.rs`

**Current Code:**
```rust
pub mod aggregation;
pub mod types;
pub mod websocket;
```

**Add:**
```rust
pub mod aggregation;
pub mod state;      // ← ADD THIS
pub mod types;
pub mod websocket;
```

**File:** `barter-trading-tuis/src/lib.rs`

**Current Code (Lines 22-23):**
```rust
pub use shared::aggregation::{calculate_vwap, VolumeWindow};
```

**Add Exports:**
```rust
pub use shared::aggregation::{calculate_vwap, VolumeWindow};

// Core aggregation engine
pub use shared::state::{
    Aggregator,
    AggregatedSnapshot,
    TickerSnapshot,
};

// Metric types
pub use shared::state::{
    OrderflowStats,
    BasisStats,
    BasisState,
    LiquidationCluster,
    CascadeLevel,
    WhaleRecord,
    CvdSummary,
    TickDirection,
    DivergenceSignal,
};
```

**Acceptance Test:**
```bash
# In any TUI binary:
use barter_trading_tuis::{Aggregator, AggregatedSnapshot};

fn main() {
    let aggregator = Aggregator::new();
    let snapshot = aggregator.snapshot();
    println!("{:?}", snapshot);
}
```

**Deliverable:**
- [ ] state.rs exported in mod.rs
- [ ] All types re-exported in lib.rs
- [ ] Compiles without errors

---

### Task 1.3: Add Telemetry to Aggregator

**Rationale:** Replace per-event logging with lightweight counters

**File:** `barter-trading-tuis/src/shared/state.rs`

**Add to Aggregator struct (after line 139):**
```rust
pub struct Aggregator {
    tickers: HashMap<String, TickerState>,
    exchange_last_seen: HashMap<String, DateTime<Utc>>,

    // Telemetry (NEW)
    pub events_received: u64,
    pub events_processed: u64,
    pub events_skipped: u64,  // Parse errors, unknown types
    pub last_lag_warning: Option<DateTime<Utc>>,
}
```

**Update new() (line 144):**
```rust
Self {
    tickers: HashMap::new(),
    exchange_last_seen: HashMap::new(),
    events_received: 0,
    events_processed: 0,
    events_skipped: 0,
    last_lag_warning: None,
}
```

**Update process_event() (lines 150-193):**
```rust
pub fn process_event(&mut self, event: MarketEventMessage) {
    self.events_received += 1;

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
                state.push_trade(trade, &event.exchange, event.time_exchange, is_spot, is_perp);
                self.events_processed += 1;
            } else {
                self.events_skipped += 1;
            }
        }
        "liquidation" => {
            if let Ok(liq) = serde_json::from_value::<LiquidationData>(event.data) {
                state.push_liquidation(liq, &event.exchange);
                self.events_processed += 1;
            } else {
                self.events_skipped += 1;
            }
        }
        // ... similar for other event types
        _ => {
            self.events_skipped += 1;
        }
    }

    // Track exchange heartbeat
    self.exchange_last_seen
        .insert(event.exchange.clone(), event.time_received);
}
```

**Add telemetry to snapshot (after line 209):**
```rust
#[derive(Clone, Debug, Default)]
pub struct AggregatedSnapshot {
    pub tickers: HashMap<String, TickerSnapshot>,
    pub correlation: [[f64; 3]; 3],
    pub exchange_health: HashMap<String, bool>,

    // Telemetry (NEW)
    pub events_received: u64,
    pub events_processed: u64,
    pub events_skipped: u64,
    pub processing_rate_pct: f64,  // processed / received * 100
}
```

**Update snapshot() method:**
```rust
pub fn snapshot(&self) -> AggregatedSnapshot {
    let processing_rate_pct = if self.events_received > 0 {
        (self.events_processed as f64 / self.events_received as f64) * 100.0
    } else {
        0.0
    };

    AggregatedSnapshot {
        tickers: tickers_out,
        correlation,
        exchange_health,
        events_received: self.events_received,
        events_processed: self.events_processed,
        events_skipped: self.events_skipped,
        processing_rate_pct,
    }
}
```

**Deliverable:**
- [ ] Telemetry counters added
- [ ] Snapshot includes telemetry
- [ ] Test verifies counters increment

---

### Task 1.4: Verify Core Metrics in state.rs

**Goal:** Ensure all Opus metrics are correctly implemented

**Checklist (from state.rs analysis):**

**Ready:**
- [x] Orderflow (1m/5m windows)
- [x] VWAP (1m/5m)
- [x] Liquidation clusters ($100 buckets)
- [x] Cascade risk scoring
- [x] Whale tracking (>$500K)
- [x] CVD total & velocity
- [x] Tick direction
- [x] Exchange dominance
- [x] Basis calculation (spot vs perp)
- [x] Correlation matrix

**Needs Fix:**
- [ ] Timestamp handling (Task 1.1)

**Missing (deferred to Phase 3):**
- [ ] Funding rates (requires server changes)
- [ ] Volatility metrics (nice to have)

**Acceptance Test:**
```rust
#[test]
fn test_opus_metrics_complete() {
    let mut agg = Aggregator::new();

    // Feed synthetic data
    add_test_trades(&mut agg);
    add_test_liquidations(&mut agg);

    let snapshot = agg.snapshot();
    let btc = &snapshot.tickers["BTC"];

    // Verify all metrics present
    assert!(btc.orderflow_1m.buy_usd > 0.0);
    assert!(btc.vwap_1m.is_some());
    assert!(btc.liquidations.len() > 0);
    assert!(btc.cascade_risk >= 0.0 && btc.cascade_risk <= 100.0);
    assert!(btc.whales.len() >= 0);
    assert!(btc.cvd.total_quote != 0.0);
    assert!(btc.tick_direction.upticks + btc.tick_direction.downticks > 0);
}
```

**Deliverable:**
- [ ] All Opus-required metrics verified
- [ ] Unit test passes
- [ ] Documentation updated with what's missing

---

## 🔌 PHASE 2: WIRE TUIs TO SHARED AGGREGATOR

**Owner:** Codex
**Estimated Time:** 8-12 hours
**Dependencies:** Phase 1 complete

### Task 2.1: Market Microstructure TUI Refactor

**File:** `barter-trading-tuis/src/bin/market_microstructure.rs`

**Step 1: Remove Local State (Lines 283-345)**

**Delete:**
```rust
struct AppState {
    tickers: HashMap<String, TickerMetrics>,  // ❌ DELETE
    last_update: Instant,
}

struct TickerMetrics { ... }  // ❌ DELETE all 100+ lines
struct OrderflowMetrics { ... }  // ❌ DELETE
// ... all local data structures
```

**Replace with:**
```rust
use barter_trading_tuis::{Aggregator, AggregatedSnapshot};

struct AppState {
    aggregator: Aggregator,  // ✅ Shared aggregation
    last_update: Instant,
}

impl AppState {
    fn new() -> Self {
        Self {
            aggregator: Aggregator::new(),
            last_update: Instant::now(),
        }
    }
}
```

**Step 2: Remove Dual-Channel Pattern (Lines 96-170)**

**Delete:**
```rust
// Dedicated channels to avoid liqs being drowned by trades
let (liq_tx, mut liq_rx) = mpsc::channel::<MarketEventMessage>(200_000);  // ❌ DELETE
let (other_tx, mut other_rx) = mpsc::channel::<MarketEventMessage>(200_000);  // ❌ DELETE

// Dispatcher: split liquidations vs everything else
tokio::spawn(async move {  // ❌ DELETE entire dispatcher task
    ...
});

// Process liquidations with dedicated lane
tokio::spawn(async move {  // ❌ DELETE separate liq processor
    ...
});
```

**Replace with:**
```rust
// Single event processing task
let app_clone = Arc::clone(&app);
tokio::spawn(async move {
    while let Some(event) = event_rx.recv().await {
        let mut app = app_clone.lock().await;
        app.aggregator.process_event(event);
    }
});
```

**Step 3: Remove Per-Event Logging (Lines 108-157)**

**Delete all eprintln! calls:**
```rust
eprintln!("📡 DISPATCHER: ...");  // ❌ DELETE
eprintln!("⚡ LIQ PROCESSOR: ...");  // ❌ DELETE
```

**Step 4: Update Render Functions**

**Change from:**
```rust
fn render_orderflow_panel(f: &mut Frame, area: Rect, app: &AppState) {
    for ticker in TICKERS {
        if let Some(metrics) = app.tickers.get(&ticker_upper) {  // ❌ OLD
            let imbalance = metrics.orderflow.imbalance_pct();
            ...
        }
    }
}
```

**To:**
```rust
fn render_orderflow_panel(f: &mut Frame, area: Rect, snapshot: &AggregatedSnapshot) {
    for ticker in TICKERS {
        if let Some(data) = snapshot.tickers.get(&ticker.to_uppercase()) {  // ✅ NEW
            let imbalance = data.orderflow_1m.imbalance_pct;
            ...
        }
    }
}
```

**Update main UI loop (Lines 183-210):**
```rust
if ui_rx.try_recv().is_ok() || last_draw.elapsed() >= draw_interval {
    // Get snapshot
    let snapshot = {
        let app_state = app.lock().await;
        app_state.aggregator.snapshot()  // ✅ Single source of truth
    };

    terminal.draw(|f| {
        let size = f.area();
        render_ui(f, size, &snapshot);  // ✅ Pass snapshot, not app
    })?;

    last_draw = Instant::now();
}
```

**Acceptance Test:**
```bash
# Build and run
cargo build --release --bin market-microstructure
cargo run --release --bin market-microstructure

# Verify:
# 1. No eprintln output to stderr
# 2. Panels render correctly
# 3. Liquidation clusters appear (during volatile periods)
# 4. No crashes after 1 hour
```

**Deliverable:**
- [ ] Local state removed
- [ ] Dual-channel pattern removed
- [ ] Logging removed
- [ ] Renders from snapshot
- [ ] Compiles and runs

---

### Task 2.2: Institutional Flow TUI Refactor

**File:** `barter-trading-tuis/src/bin/institutional_flow.rs`

**Step 1: Remove Local State (Lines 40-430)**

**Delete:**
```rust
struct App {
    instruments: HashMap<String, InstrumentData>,  // ❌ DELETE
    ...
}

struct InstrumentData { ... }  // ❌ DELETE
struct NetFlowTracker { ... }  // ❌ DELETE
struct AggressorTracker { ... }  // ❌ DELETE
struct TickTracker { ... }  // ❌ DELETE
struct TradeSizeTracker { ... }  // ❌ DELETE
```

**Replace with:**
```rust
use barter_trading_tuis::{Aggregator, AggregatedSnapshot};

struct App {
    aggregator: Aggregator,
    last_update: Instant,
    connected: bool,
}

impl App {
    fn new() -> Self {
        Self {
            aggregator: Aggregator::new(),
            last_update: Instant::now(),
            connected: false,
        }
    }
}
```

**Step 2: Add Liquidation Processing**

**Currently (Lines 449-461):**
```rust
match event.kind.as_str() {
    "trade" => { ... }
    "order_book_l1" => { ... }
    _ => {}  // ❌ Liquidations silently ignored
}
```

**Change to:**
```rust
fn handle_event(&mut self, event: MarketEventMessage) {
    self.aggregator.process_event(event);  // ✅ All events handled
    self.last_update = Instant::now();
}
```

**Step 3: Update Render Functions**

**Change from:**
```rust
fn render_net_flow_panel(f: &mut Frame, app: &App, area: Rect) {
    let instruments = app.get_primary_instruments();  // ❌ OLD
    for inst in instruments {
        let flow = inst.net_flow_5m.net_flow();
        ...
    }
}
```

**To:**
```rust
fn render_net_flow_panel(f: &mut Frame, snapshot: &AggregatedSnapshot, area: Rect) {
    for ticker in ["BTC", "ETH", "SOL"] {
        if let Some(data) = snapshot.tickers.get(ticker) {  // ✅ NEW
            let flow = data.orderflow_5m.net_flow_per_min;
            ...
        }
    }
}
```

**Deliverable:**
- [ ] Local trackers removed
- [ ] Now processes liquidations
- [ ] Renders from snapshot
- [ ] Compiles and runs

---

### Task 2.3: Risk Scanner TUI Refactor

**File:** `barter-trading-tuis/src/bin/risk_scanner.rs`

**Step 1: Remove Local Trackers (Lines 34-685)**

**Delete:**
```rust
struct App {
    liquidations: LiquidationTracker,       // ❌ DELETE
    regime: MarketRegimeDetector,           // ❌ DELETE
    arbitrage: ArbitrageTracker,            // ❌ DELETE
    correlation: CorrelationCalculator,     // ❌ DELETE
    ...
}

struct LiquidationTracker { ... }  // ❌ DELETE entire struct
struct MarketRegimeDetector { ... }  // ❌ DELETE
struct ArbitrageTracker { ... }  // ❌ DELETE
struct CorrelationCalculator { ... }  // ❌ DELETE
```

**Replace with:**
```rust
use barter_trading_tuis::{Aggregator, AggregatedSnapshot};

struct App {
    aggregator: Aggregator,
    last_update: DateTime<Utc>,
    connected: bool,
}

impl App {
    fn new() -> Self {
        Self {
            aggregator: Aggregator::new(),
            last_update: Utc::now(),
            connected: false,
        }
    }

    fn process_event(&mut self, event: MarketEventMessage) {
        self.last_update = event.time_received;
        self.aggregator.process_event(event);
    }
}
```

**Step 2: Update Render Functions**

**Liquidation Panel (Lines 718-852):**
```rust
// Change from:
let risk_score = app.liquidations.cascade_risk_score(btc);
let (price, volume, side) = app.liquidations.next_cascade_level(btc, current_price);

// To:
let btc_data = &snapshot.tickers["BTC"];
let risk_score = btc_data.cascade_risk;
let (price, volume, side) = btc_data.next_cascade_level.as_ref().map(|level| {
    (level.price, level.total_usd, level.side.clone())
});
```

**Correlation Panel (Lines 1069-1127):**
```rust
// Change from:
let matrix = app.correlation.correlation_matrix();

// To:
let matrix = snapshot.correlation;
```

**Arbitrage Panel (Lines 855-972):**
```rust
// Change from:
if let Some((basis, basis_pct)) = app.arbitrage.spot_perp_basis(symbol) {

// To:
if let Some(basis) = &snapshot.tickers[symbol].basis {
    let basis_usd = basis.basis_usd;
    let basis_pct = basis.basis_pct;
```

**Market Regime Panel:**
**NOTE:** Market regime detection is NOT in shared state.rs yet. Options:
1. Keep local MarketRegimeDetector for now
2. Add to state.rs in Phase 3
3. Remove from TUI temporarily

**Recommended:** Keep local for now, migrate in Phase 3.

**Deliverable:**
- [ ] Liquidation/correlation use snapshot
- [ ] Basis uses real calculation
- [ ] Regime detection kept local (temporary)
- [ ] Compiles and runs

---

### Task 2.4: Add Telemetry Panel to All TUIs

**Add to footer/header of each TUI:**

```rust
fn render_telemetry(f: &mut Frame, snapshot: &AggregatedSnapshot, area: Rect) {
    let rate = snapshot.processing_rate_pct;
    let color = if rate > 95.0 {
        Color::Green
    } else if rate > 80.0 {
        Color::Yellow
    } else {
        Color::Red
    };

    let lines = vec![Line::from(vec![
        Span::styled("Events: ", Style::default().fg(Color::Gray)),
        Span::styled(
            format!("{} rcv / {} proc ",
                snapshot.events_received,
                snapshot.events_processed
            ),
            Style::default().fg(Color::White),
        ),
        Span::styled(
            format!("({:.1}%)", rate),
            Style::default().fg(color),
        ),
    ])];

    let paragraph = Paragraph::new(lines).block(Block::default());
    f.render_widget(paragraph, area);
}
```

**Deliverable:**
- [ ] All 3 TUIs show telemetry
- [ ] Processing rate visible
- [ ] Color-coded health indicator

---

## 📈 PHASE 3: METRICS COMPLETION

**Owner:** Codex
**Estimated Time:** 4-8 hours
**Dependencies:** Phase 2 complete

### Task 3.1: Verify Real Basis Calculation

**Goal:** Ensure basis uses actual spot/perp mids, not simulated

**File:** `barter-trading-tuis/src/shared/state.rs`

**Current Implementation (Lines 827-856):**
```rust
fn basis(&self) -> Option<BasisStats> {
    let spot = self.spot_mid?;  // ✅ Uses real spot mid
    let perp = self.perp_mid?;  // ✅ Uses real perp mid

    if spot <= 0.0 {
        return None;
    }

    let basis_usd = perp - spot;  // ✅ Correct calculation
    let raw_pct = (basis_usd / spot) * 100.0;
    let basis_pct = (raw_pct * 100.0).round() / 100.0; // 2 decimal places
    let neutral_band = 0.05; // 5 bps deadband

    let state = if basis_pct.abs() < neutral_band {
        BasisState::Unknown
    } else if basis_pct > 0.0 {
        BasisState::Contango
    } else {
        BasisState::Backwardation
    };

    let steep = basis_pct.abs() > 0.5;

    Some(BasisStats {
        basis_usd,
        basis_pct,
        state,
        steep,
    })
}
```

**Verify spot_mid and perp_mid are populated:**

**In push_trade() (Lines 356-361):**
```rust
if is_spot {
    self.spot_mid = Some(trade.price);  // ✅
}
if is_perp {
    self.perp_mid = Some(trade.price);  // ✅
}
```

**In push_orderbook() (Lines 440-452):**
```rust
let mid = ob.mid_price().and_then(|m| m.to_f64());

if is_spot {
    self.spot_mid = mid;  // ✅
}
if is_perp {
    self.perp_mid = mid;  // ✅
}
```

**Test Basis Calculation:**
```rust
#[test]
fn test_real_basis_calculation() {
    let mut agg = Aggregator::new();

    // Add spot trade
    agg.process_event(make_trade("BTC", 95000.0, "Spot", Utc::now()));

    // Add perp trade
    agg.process_event(make_trade("BTC", 95050.0, "Perpetual", Utc::now()));

    let snapshot = agg.snapshot();
    let basis = snapshot.tickers["BTC"].basis.unwrap();

    assert_eq!(basis.basis_usd, 50.0);  // 95050 - 95000
    assert!((basis.basis_pct - 0.0526).abs() < 0.001);  // ~0.05%
    assert_eq!(basis.state, BasisState::Contango);
    assert_eq!(basis.steep, false);  // < 0.5%
}
```

**Acceptance:**
- [ ] Basis calculation uses real spot/perp mids
- [ ] Test passes
- [ ] Market Microstructure TUI displays real basis (not simulated estimate)

---

### Task 3.2: Add Market Regime Detection to state.rs

**Goal:** Centralize regime detection (currently in risk_scanner only)

**File:** `barter-trading-tuis/src/shared/state.rs`

**Add to TickerState (after line 290):**
```rust
struct TickerState {
    // ... existing fields ...

    // Market regime tracking
    regime_prices: VecDeque<(DateTime<Utc>, f64)>,  // Last 300 prices for regime detection
}
```

**Add to TickerSnapshot (after line 57):**
```rust
pub struct TickerSnapshot {
    // ... existing fields ...

    // Market regime
    pub regime: MarketRegime,
}

#[derive(Clone, Debug, Default)]
pub struct MarketRegime {
    pub state: String,        // "TRENDING", "VOLATILE", "RANGING", "RANGE-BOUND"
    pub confidence: f64,      // 0-100%
    pub volatility: f64,      // Realized volatility
    pub liquidity: String,    // "THIN", "NORMAL", "THICK"
}
```

**Implement regime detection (add to TickerState impl):**
```rust
fn detect_regime(&self) -> MarketRegime {
    if self.price_history.len() < 20 {
        return MarketRegime::default();
    }

    let volatility = self.calculate_volatility();
    let trend_strength = self.calculate_trend_strength();

    let state = if volatility > 0.02 {
        if trend_strength > 0.6 {
            "TRENDING"
        } else {
            "VOLATILE"
        }
    } else {
        if trend_strength > 0.4 {
            "RANGING"
        } else {
            "RANGE-BOUND"
        }
    };

    let confidence = (self.price_history.len() as f64 / 300.0 * 100.0).min(100.0);
    let liquidity = self.assess_liquidity();

    MarketRegime {
        state: state.to_string(),
        confidence,
        volatility,
        liquidity,
    }
}

fn calculate_volatility(&self) -> f64 {
    let prices: Vec<f64> = self.price_history.iter().map(|(_, p)| *p).collect();

    if prices.len() < 2 {
        return 0.0;
    }

    let mean = prices.iter().sum::<f64>() / prices.len() as f64;
    let variance = prices.iter()
        .map(|p| {
            let diff = p - mean;
            diff * diff
        })
        .sum::<f64>() / (prices.len() - 1) as f64;

    (variance.sqrt() / mean).max(0.0)
}

fn calculate_trend_strength(&self) -> f64 {
    let prices: Vec<f64> = self.price_history.iter().map(|(_, p)| *p).collect();

    if prices.len() < 10 {
        return 0.0;
    }

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

    (slope.abs() / y_mean * 100.0).min(1.0)
}

fn assess_liquidity(&self) -> String {
    let bid_size = self.best_bid.map(|(_, s)| s).unwrap_or(0.0);
    let ask_size = self.best_ask.map(|(_, s)| s).unwrap_or(0.0);
    let total_size = bid_size + ask_size;

    // Thresholds vary by ticker
    if self.ticker.contains("BTC") {
        if total_size > 10.0 { "THICK" }
        else if total_size > 5.0 { "NORMAL" }
        else { "THIN" }
    } else if self.ticker.contains("ETH") {
        if total_size > 50.0 { "THICK" }
        else if total_size > 25.0 { "NORMAL" }
        else { "THIN" }
    } else {
        if total_size > 1000.0 { "THICK" }
        else if total_size > 500.0 { "NORMAL" }
        else { "THIN" }
    }.to_string()
}
```

**Deliverable:**
- [ ] Regime detection in state.rs
- [ ] Risk Scanner TUI uses snapshot.regime
- [ ] Test validates regime detection

---

### Task 3.3: Document Funding Rate Gap

**Goal:** Make it clear funding rates are not yet implemented

**File:** `barter-trading-tuis/src/shared/state.rs`

**Add comment (after line 23):**
```rust
/// Market event message envelope from the server
///
/// # Supported Event Types
/// - `trade`: Public trades from spot and perpetual markets
/// - `liquidation`: Forced liquidations (perpetuals only)
/// - `cumulative_volume_delta`: Exchange-provided CVD (preferred over trade-derived)
/// - `open_interest`: Total outstanding contracts
/// - `order_book_l1`: Top of book (best bid/ask)
///
/// # NOT YET SUPPORTED
/// - `funding_rate`: Perpetual funding rates (server doesn't broadcast yet)
///
/// To add funding rate support:
/// 1. Update barter-data-server to poll funding rates (similar to OI polling)
/// 2. Add FundingData struct to types.rs
/// 3. Add funding field to TickerState
/// 4. Implement funding momentum calculation
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarketEventMessage {
    ...
}
```

**Add to TickerSnapshot:**
```rust
pub struct TickerSnapshot {
    // ... existing fields ...

    /// Funding rate metrics (currently None - awaiting server support)
    pub funding: Option<FundingMetrics>,
}

#[derive(Clone, Debug, Default)]
pub struct FundingMetrics {
    pub current_rate: f64,      // Current funding rate %
    pub rate_8h: f64,           // 8-hour funding rate
    pub momentum: String,       // "↑↑↑"/"↑↑"/"↑"/"→"/"↓"/"↓↓"/"↓↓↓"
    pub payer: String,          // "LONGS PAY" / "SHORTS PAY"
    pub intensity: String,      // "EXTREME" if >0.04%, "HIGH", "NORMAL"
}
```

**Deliverable:**
- [ ] Funding gap documented
- [ ] Placeholder in snapshot
- [ ] TUIs show "N/A" for funding panel

---

## ✅ PHASE 4: TESTING & VALIDATION

**Owner:** Codex (lead), Sonnet (assist)
**Estimated Time:** 4-8 hours
**Dependencies:** Phase 3 complete

### Task 4.1: Unit Tests for Core Metrics

**File:** `barter-trading-tuis/src/shared/state.rs` (add at end)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_trade_event(ticker: &str, price: f64, kind: &str, time: DateTime<Utc>) -> MarketEventMessage {
        MarketEventMessage {
            time_exchange: time,
            time_received: time,
            exchange: "BinanceFuturesUsd".to_string(),
            instrument: InstrumentInfo {
                base: ticker.to_lowercase(),
                quote: "usdt".to_string(),
                kind: kind.to_string(),
            },
            kind: "trade".to_string(),
            data: serde_json::json!({
                "id": "12345",
                "price": price,
                "amount": 1.0,
                "side": "Buy"
            }),
        }
    }

    #[test]
    fn test_orderflow_1m_window() {
        let mut agg = Aggregator::new();
        let now = Utc::now();

        // Buy trade
        agg.process_event(make_trade_event("BTC", 95000.0, "Perpetual", now));

        // Sell trade
        let mut sell_event = make_trade_event("BTC", 95000.0, "Perpetual", now);
        sell_event.data["side"] = serde_json::json!("Sell");
        agg.process_event(sell_event);

        let snapshot = agg.snapshot();
        let btc = &snapshot.tickers["BTC"];

        assert!(btc.orderflow_1m.buy_usd > 0.0);
        assert!(btc.orderflow_1m.sell_usd > 0.0);
        assert!((btc.orderflow_1m.imbalance_pct - 50.0).abs() < 1.0); // ~50% buy
    }

    #[test]
    fn test_basis_calculation() {
        let mut agg = Aggregator::new();
        let now = Utc::now();

        // Spot @ 95000
        agg.process_event(make_trade_event("BTC", 95000.0, "Spot", now));

        // Perp @ 95050 (contango)
        agg.process_event(make_trade_event("BTC", 95050.0, "Perpetual", now));

        let snapshot = agg.snapshot();
        let basis = snapshot.tickers["BTC"].basis.as_ref().unwrap();

        assert_eq!(basis.basis_usd, 50.0);
        assert!((basis.basis_pct - 0.0526).abs() < 0.001);
        assert_eq!(basis.state, BasisState::Contango);
    }

    #[test]
    fn test_liquidation_clustering() {
        let mut agg = Aggregator::new();
        let now = Utc::now();

        // Add liquidations at similar price levels
        for i in 0..10 {
            let mut liq_event = make_trade_event("BTC", 95050.0 + i as f64, "Perpetual", now);
            liq_event.kind = "liquidation".to_string();
            liq_event.data = serde_json::json!({
                "side": "Buy",
                "price": 95050.0 + i as f64,
                "quantity": 1.0,
                "time": now
            });
            agg.process_event(liq_event);
        }

        let snapshot = agg.snapshot();
        let btc = &snapshot.tickers["BTC"];

        assert!(btc.liquidations.len() > 0);
        assert!(btc.cascade_risk >= 0.0);
    }

    #[test]
    fn test_correlation_matrix() {
        let mut agg = Aggregator::new();
        let now = Utc::now();

        // Add correlated price movements for BTC/ETH
        for i in 0..100 {
            let time = now + chrono::Duration::seconds(i);
            agg.process_event(make_trade_event("BTC", 95000.0 + i as f64 * 10.0, "Perpetual", time));
            agg.process_event(make_trade_event("ETH", 3200.0 + i as f64 * 0.5, "Perpetual", time));
        }

        let snapshot = agg.snapshot();
        let corr = snapshot.correlation;

        // BTC-BTC = 1.0
        assert!((corr[0][0] - 1.0).abs() < 0.01);

        // BTC-ETH should be positive (both trending up)
        assert!(corr[0][1] > 0.5);
    }

    #[test]
    fn test_event_timestamp_not_utc_now() {
        let mut agg = Aggregator::new();

        // Event from 30 minutes ago
        let old_time = Utc::now() - chrono::Duration::minutes(30);
        agg.process_event(make_trade_event("BTC", 95000.0, "Perpetual", old_time));

        // Event from now
        let now = Utc::now();
        agg.process_event(make_trade_event("BTC", 96000.0, "Perpetual", now));

        let snapshot = agg.snapshot();
        let btc = &snapshot.tickers["BTC"];

        // Old event should be pruned from 1m window (15min retention)
        // Only recent event should count
        assert!(btc.orderflow_1m.buy_usd > 0.0);
    }

    #[test]
    fn test_whale_tracking() {
        let mut agg = Aggregator::new();
        let now = Utc::now();

        // Small trade (not whale)
        let mut small = make_trade_event("BTC", 95000.0, "Perpetual", now);
        small.data["amount"] = serde_json::json!(0.1);  // $9,500
        agg.process_event(small);

        // Whale trade
        let mut whale = make_trade_event("BTC", 95000.0, "Perpetual", now);
        whale.data["amount"] = serde_json::json!(10.0);  // $950,000
        agg.process_event(whale);

        let snapshot = agg.snapshot();
        let btc = &snapshot.tickers["BTC"];

        assert_eq!(btc.whales.len(), 1);  // Only whale trade tracked
        assert!(btc.whales[0].volume_usd >= 500_000.0);
    }

    #[test]
    fn test_cvd_divergence_signals() {
        let mut agg = Aggregator::new();
        let now = Utc::now();

        // Price down trend
        for i in 0..50 {
            let time = now + chrono::Duration::seconds(i);
            let price = 95000.0 - i as f64 * 10.0;  // Declining

            // But volume is net buying (CVD up)
            let mut buy = make_trade_event("BTC", price, "Perpetual", time);
            buy.data["amount"] = serde_json::json!(2.0);
            agg.process_event(buy);

            let mut sell = make_trade_event("BTC", price, "Perpetual", time);
            sell.data["side"] = serde_json::json!("Sell");
            sell.data["amount"] = serde_json::json!(1.0);
            agg.process_event(sell);
        }

        let snapshot = agg.snapshot();
        let btc = &snapshot.tickers["BTC"];

        // Price down + CVD up = Bullish divergence
        assert_eq!(btc.cvd_divergence, DivergenceSignal::Bullish);
    }
}
```

**Run Tests:**
```bash
cargo test --lib --package barter-trading-tuis
```

**Acceptance:**
- [ ] All 8 tests pass
- [ ] Code coverage > 80% for state.rs

---

### Task 4.2: Integration Test - Side-by-Side Comparison

**Goal:** Run new TUIs alongside legacy TUI to verify data consistency

**File:** Create `test_side_by_side.sh`

```bash
#!/bin/bash

# Start server in background
cargo run --release -p barter-data-server 2>&1 | tee server.log &
SERVER_PID=$!

sleep 5

# Start legacy TUI (for reference)
cargo run --release -p barter-data-tui > legacy_output.log 2>&1 &
LEGACY_PID=$!

# Start new market-microstructure TUI
cargo run --release --bin market-microstructure > micro_output.log 2>&1 &
MICRO_PID=$!

# Start new institutional-flow TUI
cargo run --release --bin institutional-flow > inst_output.log 2>&1 &
INST_PID=$!

# Start new risk-scanner TUI
cargo run --release --bin risk-scanner > risk_output.log 2>&1 &
RISK_PID=$!

echo "Running for 10 minutes..."
sleep 600

# Stop all
kill $LEGACY_PID $MICRO_PID $INST_PID $RISK_PID $SERVER_PID

echo "Analyzing logs..."

# Check for errors
echo "=== Errors in server.log ==="
grep -i "error\|panic\|crash" server.log || echo "No errors"

echo "=== Liquidations received (server) ==="
grep "LIQ EVENT" server.log | wc -l

echo "=== Test complete ==="
```

**Manual Checklist:**
- [ ] Server logs show liquidations arriving
- [ ] All 3 new TUIs render without crashes
- [ ] Liquidation counts match across TUIs
- [ ] No stderr spam from market-microstructure
- [ ] Memory usage stable (no leaks)

---

### Task 4.3: Load Test - Volatile Market Conditions

**Goal:** Verify system handles high event throughput

**Procedure:**
1. Run during US market open (high volatility)
2. Monitor telemetry panel in each TUI
3. Watch for:
   - Processing rate staying > 95%
   - No event drops
   - Liquidation clusters updating in real-time
   - No UI freezes

**Acceptance:**
- [ ] Runs for 4+ hours during volatile period
- [ ] Processing rate > 90% throughout
- [ ] No crashes or memory leaks
- [ ] Liquidations visible and clustering correctly

---

### Task 4.4: Replay Test (Optional but Recommended)

**Goal:** Record event stream, replay to verify deterministic results

**Step 1: Record Stream**
```rust
// Add to barter-data-server/src/main.rs
let mut log_file = std::fs::File::create("event_recording.jsonl")?;

while let Some(event) = combined_stream.next().await {
    // Write to file
    if let Event::Item(Ok(market_event)) = &event {
        let json = serde_json::to_string(&market_event)?;
        writeln!(log_file, "{}", json)?;
    }

    // Broadcast as normal
    ...
}
```

**Step 2: Replay to Aggregator**
```rust
#[test]
fn test_replay_deterministic() {
    let file = std::fs::File::open("event_recording.jsonl").unwrap();
    let reader = std::io::BufReader::new(file);

    let mut agg = Aggregator::new();

    for line in reader.lines() {
        let event: MarketEventMessage = serde_json::from_str(&line.unwrap()).unwrap();
        agg.process_event(event);
    }

    let snapshot = agg.snapshot();

    // Verify expected metrics
    assert!(snapshot.tickers.contains_key("BTC"));
    assert!(snapshot.tickers["BTC"].orderflow_1m.buy_usd > 0.0);
    // ... more assertions
}
```

**Acceptance:**
- [ ] Replay produces consistent results
- [ ] Snapshot matches expected values
- [ ] No panics during replay

---

## 📚 PHASE 5: DOCUMENTATION & OPS

**Owner:** Sonnet
**Estimated Time:** 2-4 hours
**Dependencies:** Phase 4 complete

### Task 5.1: Update README

**File:** `barter-trading-tuis/README.md` (create if missing)

```markdown
# Barter Trading TUIs

Three institutional-grade terminal interfaces for cryptocurrency market analysis.

## Architecture

```
barter-data-server (port 9001)
    ↓
WebSocket JSON Events
    ↓
Shared Aggregator (state.rs)
    ↓
Snapshot (consistent metrics)
   / | \
  /  |  \
TUI1 TUI2 TUI3
```

## TUI Applications

### 1. Market Microstructure (`market-microstructure`)
**Refresh:** 250ms
**Use Case:** Active trading decisions

**Panels:**
- Orderflow Imbalance (1m window)
- Spot vs Perp Basis (real calculation)
- Liquidation Clusters ($100 price buckets)
- Funding Momentum (not yet implemented)
- Whale Detector (>$100K trades)
- CVD Divergence (price vs volume delta)

### 2. Institutional Flow (`institutional-flow`)
**Refresh:** 1 second
**Use Case:** Smart money tracking

**Panels:**
- Net Flow (5m windows)
- Aggressor Ratio (buy vs sell initiated)
- Exchange Dominance (% by exchange)
- Orderbook Depth Imbalance (L1 only)
- Momentum Signals (VWAP deviation, tick direction)

### 3. Risk Scanner (`risk-scanner`)
**Refresh:** 5 seconds
**Use Case:** Risk monitoring

**Panels:**
- Liquidation Cascade Risk (0-100 score)
- Next Cascade Level & Protection
- Market Regime Detection
- Correlation Matrix (BTC/ETH/SOL)

## Usage

```bash
# Start server
cargo run --release -p barter-data-server

# In separate terminals:
cargo run --release --bin market-microstructure
cargo run --release --bin institutional-flow
cargo run --release --bin risk-scanner

# Press 'q' to quit any TUI
```

## Metrics Glossary

**Orderflow Imbalance:** Buy USD / (Buy USD + Sell USD) * 100
**Basis:** (Perp Mid - Spot Mid) / Spot Mid * 100
**Cascade Risk:** (Largest Cluster USD / $50M) * 100, capped at 100%
**CVD Divergence:** Price trend vs cumulative volume delta trend
**Exchange Dominance:** Exchange volume / Total volume * 100 (1m window)

## Known Limitations

- **Funding Rates:** Not yet implemented (awaiting server support)
- **L2 Orderbook:** Only L1 (top of book) available
- **Volatility Metrics:** Not yet implemented
- **ETH/SOL Data:** May be missing if upstream connection fails

## Telemetry

Each TUI displays processing rate in footer:
- **Green (>95%):** Healthy
- **Yellow (80-95%):** Some lag
- **Red (<80%):** Backpressure/issues

## Testing

```bash
# Unit tests
cargo test --lib --package barter-trading-tuis

# Integration test (10 minute run)
./test_side_by_side.sh
```

## Architecture Details

See `TUI_REFACTOR_PLAN.md` for complete refactor documentation.
```

**Deliverable:**
- [ ] README.md created
- [ ] Usage instructions clear
- [ ] Limitations documented

---

### Task 5.2: Add Inline Code Comments

**File:** `barter-trading-tuis/src/shared/state.rs`

**Add doc comments to key methods:**

```rust
/// Process a market event and update internal state.
///
/// # Arguments
/// * `event` - Market event from WebSocket server
///
/// # Supported Events
/// - `trade`: Updates orderflow, VWAP, whale tracking, CVD
/// - `liquidation`: Updates clusters, cascade risk
/// - `cumulative_volume_delta`: Updates CVD (preferred over trade-derived)
/// - `open_interest`: Updates total OI across exchanges
/// - `order_book_l1`: Updates bid/ask, spread, basis
///
/// # Time Handling
/// Uses `event.time_exchange` for all timestamps and pruning windows.
/// This ensures multi-exchange data stays synchronized even with network latency.
///
/// # Example
/// ```
/// let mut agg = Aggregator::new();
/// agg.process_event(market_event);
/// let snapshot = agg.snapshot();
/// ```
pub fn process_event(&mut self, event: MarketEventMessage) {
    ...
}

/// Generate a snapshot of all current metrics.
///
/// This is the primary API for TUIs - call periodically to get latest data.
/// All metrics are pre-computed (no heavy calculations in snapshot()).
///
/// # Performance
/// O(n) where n = number of tickers tracked (typically 3-5).
/// Safe to call at 250ms intervals without performance impact.
///
/// # Returns
/// `AggregatedSnapshot` containing per-ticker metrics and cross-ticker correlation.
pub fn snapshot(&self) -> AggregatedSnapshot {
    ...
}
```

**Deliverable:**
- [ ] All public methods documented
- [ ] Code comments explain why, not what
- [ ] Examples included

---

### Task 5.3: Create Troubleshooting Guide

**File:** `barter-trading-tuis/TROUBLESHOOTING.md`

```markdown
# Troubleshooting Guide

## No Liquidations Showing

**Symptom:** Liquidation cluster panel shows "No liquidations in last 5 minutes"

**Causes:**
1. **Market is calm** - Liquidations are RARE (1-50 per hour)
2. **Wrong time window** - Happened >5 minutes ago
3. **Upstream connection** - Server not receiving liquidation feeds

**Debug:**
```bash
# Check server logs
tail -f server.log | grep "LIQ EVENT"

# Should see:
# LIQ EVENT BinanceFuturesUsd btc/usdt @ 95234.5 qty 0.123 side Buy
```

**Fix:**
- Wait for volatile market period (major news, cascade events)
- Increase retention window in state.rs (line 15: `LIQ_RETENTION_SECS`)
- Verify exchange connections in server logs

---

## ETH/SOL Data Missing

**Symptom:** Only BTC data shows, ETH/SOL panels empty

**Causes:**
1. **Upstream connection failed** - Server not connected to exchange feeds
2. **Subscription issue** - Server config missing ETH/SOL subscriptions

**Debug:**
```bash
grep "eth\|sol" server.log

# Should see:
# Subscribing to ETH/USDT perpetual on BinanceFuturesUsd
```

**Fix:**
- Restart server
- Check `barter-data-server/src/main.rs` subscriptions (lines 290-398)
- Verify exchange API status

---

## Basis Shows "N/A"

**Symptom:** Spot vs Perp Basis panel shows "Data not available"

**Causes:**
1. **No spot data** - Only perp prices arriving
2. **No perp data** - Only spot prices arriving
3. **Different price sources** - Spot from one exchange, perp from another

**Debug:**
```bash
# Check snapshot in TUI telemetry
# Should see both spot_mid and perp_mid populated
```

**Fix:**
- Verify both spot and perp subscriptions in server
- Wait for both markets to trade
- Check if exchanges are operational

---

## High Processing Lag (Rate < 80%)

**Symptom:** Telemetry shows processing rate in yellow/red

**Causes:**
1. **Network latency** - Slow connection to exchanges
2. **CPU overload** - Machine resources exhausted
3. **Event storm** - Extremely high market activity

**Debug:**
```bash
# Check CPU usage
top -p $(pgrep market-microstructure)

# Check network
ping api.binance.com
```

**Fix:**
- Close other applications
- Increase channel buffer sizes
- Run on faster hardware

---

## TUI Crashes on Startup

**Symptom:** TUI exits immediately with error

**Debug:**
```bash
# Run with full error output
RUST_BACKTRACE=full cargo run --release --bin market-microstructure 2> error.log
cat error.log
```

**Common Errors:**
- **WebSocket connection refused** - Server not running
- **Parse error** - Incompatible event format (update server)
- **Panic in render** - Missing data (check snapshot fields)

---

## Memory Leak

**Symptom:** Memory usage grows over time

**Debug:**
```bash
# Monitor memory
watch -n 5 'ps aux | grep market-micro'
```

**Causes:**
- Unbounded VecDeques not being pruned
- Event channel backlog

**Fix:**
- Verify pruning is working (check retention constants)
- Restart TUI daily
- Report issue with memory profile

---

## Inconsistent Numbers Across TUIs

**Symptom:** Market Microstructure shows different liquidation count than Risk Scanner

**This Should Not Happen (Post-Refactor)**

If you see this:
1. Verify all TUIs are using shared Aggregator (not local caches)
2. Check git branch - may be on old pre-refactor code
3. Report as critical bug

---

## Funding Panel Shows "N/A"

**Expected Behavior** - Funding rates not yet implemented.

See `TUI_REFACTOR_PLAN.md` Task 3.3 for implementation plan.

```

**Deliverable:**
- [ ] Troubleshooting guide created
- [ ] Common issues documented
- [ ] Debug procedures clear

---

### Task 5.4: Create Side-by-Side Launcher Script

**File:** `launch_all_tuis.sh`

```bash
#!/bin/bash
# Launch all TUIs in tmux for side-by-side comparison

set -e

echo "=== Barter Trading TUIs Launcher ==="

# Check if tmux is installed
if ! command -v tmux &> /dev/null; then
    echo "Error: tmux not installed. Install with: brew install tmux"
    exit 1
fi

# Check if server is running
if ! lsof -i:9001 > /dev/null; then
    echo "Starting barter-data-server..."
    cargo build --release -p barter-data-server
    cargo run --release -p barter-data-server &> server.log &
    sleep 5
fi

# Build all TUIs
echo "Building TUIs..."
cargo build --release --bin market-microstructure
cargo build --release --bin institutional-flow
cargo build --release --bin risk-scanner

# Create tmux session
SESSION="barter-tuis"
tmux new-session -d -s $SESSION

# Split window into 3 panes
tmux split-window -h -t $SESSION
tmux split-window -h -t $SESSION
tmux select-layout -t $SESSION even-horizontal

# Launch TUI in each pane
tmux send-keys -t $SESSION:0.0 "cargo run --release --bin market-microstructure" C-m
tmux send-keys -t $SESSION:0.1 "cargo run --release --bin institutional-flow" C-m
tmux send-keys -t $SESSION:0.2 "cargo run --release --bin risk-scanner" C-m

# Attach to session
echo "Attaching to tmux session..."
echo "Press 'q' in any TUI to quit"
echo "Detach with: Ctrl+b d"
tmux attach -t $SESSION

echo "Session ended."
```

**Make executable:**
```bash
chmod +x launch_all_tuis.sh
```

**Deliverable:**
- [ ] Launcher script created
- [ ] Works on macOS/Linux
- [ ] Creates side-by-side layout

---

## 🎯 ACCEPTANCE CRITERIA (Final Checklist)

### Phase 1: Aggregation Engine
- [ ] state.rs uses event timestamps (not Utc::now())
- [ ] state.rs exported in mod.rs and lib.rs
- [ ] Telemetry counters added
- [ ] All Opus metrics verified

### Phase 2: TUI Wiring
- [ ] market-microstructure uses shared aggregator
- [ ] institutional-flow uses shared aggregator
- [ ] risk-scanner uses shared aggregator
- [ ] All per-TUI caches removed
- [ ] Logging spam removed
- [ ] All TUIs render from snapshot

### Phase 3: Metrics Completion
- [ ] Basis uses real spot/perp mids
- [ ] Market regime in shared state
- [ ] Funding gap documented

### Phase 4: Testing
- [ ] Unit tests pass (8/8)
- [ ] Side-by-side comparison successful
- [ ] 4-hour load test passes
- [ ] Replay test passes (if implemented)

### Phase 5: Documentation
- [ ] README.md complete
- [ ] Code comments added
- [ ] Troubleshooting guide created
- [ ] Launcher script works

### Final Validation
- [ ] All 3 TUIs run simultaneously without crashes
- [ ] Liquidation counts match across TUIs
- [ ] Basis calculation correct
- [ ] No memory leaks (24-hour test)
- [ ] Processing rate > 95% under normal load
- [ ] Telemetry visible in all TUIs

---

## 🚨 RISK MITIGATION

### Rollback Plan
If refactor fails:
1. Git branch: `feature/tui-enhancements-and-fixes` (current code)
2. New branch: `refactor/shared-aggregator` (refactor work)
3. Can revert to old branch at any time

### Incremental Validation
- Test each phase before proceeding
- Keep legacy TUI running for comparison
- Don't delete old code until new code validated

### Known Risks

| Risk | Probability | Impact | Mitigation |
|------|------------|--------|------------|
| Timestamp bug causes data loss | Medium | High | Extensive testing in Phase 4 |
| Performance degradation | Low | Medium | Load testing, profiling |
| Regression in liquidation capture | Low | High | Side-by-side comparison |
| Memory leak | Low | Medium | 24-hour soak test |

---

## 📊 ESTIMATED TIMELINE

| Phase | Tasks | Hours | Lead |
|-------|-------|-------|------|
| 1. Aggregation Engine | 4 tasks | 8-12 | Codex |
| 2. TUI Wiring | 4 tasks | 8-12 | Codex |
| 3. Metrics Completion | 3 tasks | 4-8 | Codex |
| 4. Testing | 4 tasks | 4-8 | Codex + Sonnet |
| 5. Documentation | 4 tasks | 2-4 | Sonnet |
| **Total** | **19 tasks** | **26-44 hours** | **3-5 days** |

**With buffer:** 4-6 days for production-ready refactor

---

## 🤝 OWNER RESPONSIBILITIES

### Codex (Lead)
- Execute Phases 1-4
- Make all code changes
- Run all tests
- Validate correctness
- Ensure zero regressions

### Sonnet (Assistant)
- Create this specification document ✅
- Execute Phase 5 (documentation)
- Assist with testing
- Write test cases
- Review Codex's work

---

## ✅ SUCCESS DEFINITION

**This refactor is successful when:**

1. **Consistency:** All 3 TUIs show identical liquidation counts, basis values, and dominance percentages
2. **Correctness:** Event timestamps used (not Utc::now()), basis calculated from real spot/perp
3. **Stability:** 24-hour uptime test passes with no crashes or memory leaks
4. **Performance:** Processing rate > 95% under normal load
5. **Code Quality:** Dead code removed, logic not duplicated, tests passing
6. **Documentation:** README complete, troubleshooting guide exists, code commented

**Delivery:** Production-ready TUIs with single source of truth for all metrics.

---

**END OF COMPREHENSIVE REFACTOR PLAN**
