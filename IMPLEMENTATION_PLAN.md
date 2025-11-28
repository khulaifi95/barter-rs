# Barter-RS TUI Enhancement Implementation Plan

## 📋 Overview

This document outlines the complete implementation plan for enhancing the barter-rs terminal user interfaces (TUIs) with institutional-grade trading metrics and fixing critical WebSocket stability issues.

**Branch:** `feature/tui-enhancements-and-fixes`
**Base Branch:** `dev`
**Target Merge:** `dev` → `main`

---

## 🎯 Project Goals

1. **Fix Critical Bugs**: WebSocket connections disconnecting after 5-6 minutes
2. **Add Missing Data**: Implement aggregations and calculations currently absent
3. **Build Professional TUIs**: Three specialized institutional-grade trading terminals

---

## 🐛 Critical Bugs to Fix

### Bug 1: WebSocket Broadcast Buffer Too Small
**File:** `barter-data-server/src/main.rs:81`

**Problem:**
```rust
// Current (TOO SMALL):
let (tx, _rx) = broadcast::channel::<MarketEventMessage>(1000);
```

With 3 exchanges × multiple data types (trades, liquidations, CVD, OI, L1) generating 100+ events/sec:
- Buffer fills in ~10 seconds
- Slow clients lag → `RecvError::Lagged` → disconnect

**Fix:**
```rust
// Increase 10x:
let (tx, _rx) = broadcast::channel::<MarketEventMessage>(10_000);
```

---

### Bug 2: Client Doesn't Handle Lag Errors
**File:** `barter-data-tui/src/main.rs:394-422`

**Problem:**
```rust
while let Some(msg) = read.next().await {
    match msg {
        Ok(Message::Text(text)) => { /* process */ }
        Ok(Message::Close(_)) => { break; }  // ← Exits
        Err(_) => { break; }                 // ← Exits on ANY error!
        _ => {}
    }
}
```

On `RecvError::Lagged`, client breaks instead of catching up.

**Fix:** Handle lag gracefully, add error differentiation, continue on recoverable errors.

---

### Bug 3: No Heartbeat/Ping-Pong
**Problem:** WebSocket connections need periodic pings to stay alive.

**Fix:** Add 30-second ping interval in client.

---

## 📊 Missing Data & Calculations

### Currently NOT Tracked:

#### 1. Volume Metrics (COMPLETELY MISSING)
```rust
struct VolumeMetrics {
    // Per time window (1m, 5m, 15m)
    total_volume_usd: f64,
    buy_volume_usd: f64,
    sell_volume_usd: f64,
    trade_count: u64,
    vwap: f64,                        // Volume-weighted average price
    volume_rate: f64,                 // $/sec

    // By exchange
    exchange_volumes: HashMap<String, f64>,
    exchange_dominance: HashMap<String, f64>,  // %

    // Large trades (tiered)
    large_trades: Vec<Trade>,         // >$100K (for aggregation)
    whale_trades: Vec<Trade>,         // >$500K (for TUI display)
    mega_whale_trades: Vec<Trade>,    // >$5M (⚠️ flagged)
    avg_trade_size: f64,
    trade_size_trend: f64,            // Is size increasing?
    whale_count_5m: u64,              // Count in last 5 minutes
}
```

#### 2. Open Interest - Missing Aggregations
```rust
// Currently have: OI per exchange ✅
// Missing:
total_oi_all_exchanges: f64,         // Sum across BNC+OKX+BBT
oi_delta_rate: f64,                  // contracts/sec
oi_momentum: f64,                    // acceleration
exchange_oi_share: f64,              // % of total
oi_vs_volume_ratio: f64,
```

#### 3. CVD - Missing Intelligence
```rust
// Currently have: CVD per exchange ✅
// Missing:
total_cvd_all_exchanges: f64,        // Net across all
cvd_velocity: f64,                   // rate of change
cvd_divergence: bool,                // vs price direction
exchange_cvd_correlation: f64,
net_flow_1m: f64,
net_flow_5m: f64,
```

#### 4. Liquidations - Missing Aggregations
```rust
// Currently: Just store events ✅
// Missing:
total_liq_volume_1m: f64,
total_liq_volume_5m: f64,
long_liq_volume: f64,
short_liq_volume: f64,
liq_clusters: HashMap<i64, Vec<Liq>>,  // Grouped by price level
cascade_risk_score: f64,             // 0-100
avg_liq_size: f64,
liq_rate: f64,                       // events/min
```

#### 5. OrderBook L1 - Missing Intelligence
```rust
// Currently: bid/ask/spread ✅
// Missing:
depth_imbalance: f64,                // bid_qty / ask_qty
bid_pressure: f64,                   // %
ask_pressure: f64,                   // %
size_ratio: f64,
micro_structure_signal: String,      // "BUY"/"SELL"/"NEUTRAL"
```

#### 6. Orderflow Analysis (COMPLETELY MISSING)
```rust
struct OrderflowMetrics {
    // 1-minute window
    buy_initiated_volume: f64,       // Aggressor = buyer
    sell_initiated_volume: f64,      // Aggressor = seller
    imbalance: f64,                  // -100 to +100

    // Tick direction
    upticks: u64,                    // Consecutive buys
    downticks: u64,                  // Consecutive sells
    tick_ratio: f64,

    // Size analysis
    avg_buy_size: f64,
    avg_sell_size: f64,
    size_imbalance: f64,

    // Aggressor ratio
    aggressor_ratio: f64,            // buy trades / sell trades
}
```

#### 7. Cross-Market Metrics (COMPLETELY MISSING)
```rust
struct CrossMarketMetrics {
    // Spot vs Perp (if we add spot data)
    basis: f64,                      // perp - spot
    basis_pct: f64,
    basis_trend: String,             // "↑"/"↓"/"—"

    // Exchange arbitrage
    price_spreads: HashMap<String, f64>,
    volume_migration: String,        // Which exchange gaining volume

    // Correlation
    exchange_correlation: f64,
}
```

#### 8. Funding Rate Metrics (COMPLETELY MISSING)
```rust
struct FundingMetrics {
    current_rate: f64,               // Current funding rate %
    rate_8h: f64,                    // 8-hour funding rate
    momentum: String,                // "↑↑↑"/"↑↑"/"↑"/"→"/"↓"/"↓↓"/"↓↓↓"
    delta: f64,                      // d(funding)/dt

    // Who pays
    payer: String,                   // "LONGS PAY" / "SHORTS PAY"
    intensity: String,               // "EXTREME" if >0.04%

    // Cross-exchange
    exchange_rates: HashMap<String, f64>,
    arbitrage_opportunity: bool,     // Rate differential >0.01%
}
```

#### 9. Spot-Perp Basis Metrics (COMPLETELY MISSING)
```rust
struct BasisMetrics {
    basis_usd: f64,                  // perp_price - spot_price
    basis_pct: f64,                  // (basis / spot) × 100
    state: String,                   // "CONTANGO" / "BACKWARDATION"
    intensity: String,               // "STEEP" if >0.5%

    // Trend
    trend: String,                   // "↑" / "→" / "↓"

    // Arbitrage
    arb_opportunity: bool,           // If >0.03%
}
```

#### 10. Tick Direction Metrics (COMPLETELY MISSING)
```rust
struct TickMetrics {
    upticks: u64,                    // Consecutive price increases
    downticks: u64,                  // Consecutive price decreases
    tick_ratio: f64,                 // upticks / (upticks + downticks)
    tick_percentage_up: f64,         // % of upticks

    // Momentum
    consecutive_direction: String,   // Current streak direction
    streak_length: u64,              // Current streak count
}
```

#### 11. Risk Metrics (COMPLETELY MISSING)
```rust
struct RiskMetrics {
    // Liquidation cascade
    cascade_risk: f64,               // 0-100 score
    next_cascade_level: f64,         // Price level
    next_cascade_volume: f64,        // $ at risk
    protection_level: f64,           // Opposite side liquidation level
    protection_volume: f64,          // $ at protection level

    // Market regime
    regime: String,                  // "TRENDING"/"RANGING"/"VOLATILE"/"RANGE-BOUND"
    regime_confidence: f64,          // 0-100%
    trend_direction: String,         // "BULLISH"/"BEARISH"/"NEUTRAL"

    // Volatility
    realized_vol: f64,
    implied_vol: f64,                // If we have options data
    vol_state: String,               // "INCREASING"/"DECREASING"/"STABLE"

    // Liquidity
    liquidity: String,               // "THIN"/"NORMAL"/"THICK"
    liquidity_score: f64,            // 0-100
}
```

#### 12. Correlation Metrics (COMPLETELY MISSING)
```rust
struct CorrelationMetrics {
    // Asset correlations
    btc_eth_corr: f64,               // -1.0 to 1.0
    btc_sol_corr: f64,
    eth_sol_corr: f64,

    // Exchange correlations
    exchange_corr: HashMap<String, f64>,

    // Leading/lagging
    leader: String,                  // Which asset leads
    lag_seconds: f64,                // Time lag
}
```

---

## 🏗️ Architecture

### Current System
```
┌──────────────────────────────────────┐
│ EXCHANGES (Binance, Bybit, OKX)      │
└────────────┬─────────────────────────┘
             │ WebSocket streams
             ▼
┌──────────────────────────────────────┐
│ barter-data-server                   │
│ • Receives raw events                │
│ • NO aggregation                     │
│ • NO statistics                      │
│ • Pure relay via WebSocket           │
│ • Broadcast channel (1000 → 10000)   │
└────────────┬─────────────────────────┘
             │ ws://0.0.0.0:9001
             ▼
┌──────────────────────────────────────┐
│ barter-data-tui (current)            │
│ • Simple event display               │
│ • Basic sparklines                   │
│ • Minimal aggregation                │
└──────────────────────────────────────┘
```

### New Architecture
```
┌──────────────────────────────────────┐
│ EXCHANGES (Binance, Bybit, OKX)      │
└────────────┬─────────────────────────┘
             │ WebSocket streams
             ▼
┌──────────────────────────────────────┐
│ barter-data-server (ENHANCED)        │
│ • Broadcast buffer: 10,000 events    │
│ • Lag error handling                 │
│ • Ping/pong heartbeat                │
└────────────┬─────────────────────────┘
             │ ws://0.0.0.0:9001
             ▼
┌──────────────────────────────────────┐
│ Shared Data Aggregation Layer        │
│ • Volume windows (1m, 5m, 15m)       │
│ • Total OI/CVD across exchanges      │
│ • Orderflow metrics                  │
│ • Liquidation clusters               │
│ • Exchange dominance                 │
└────┬────┬────┬──────────────────────┘
     │    │    │
     ▼    ▼    ▼
   ┌───┐┌───┐┌───┐
   │TUI││TUI││TUI│
   │ 1 ││ 2 ││ 3 │
   └───┘└───┘└───┘
```

---

## 🖥️ Three TUI Design

### TUI 1: Market Microstructure Dashboard
**Binary:** `market-microstructure`
**Purpose:** Real-time orderflow and market activity
**Refresh Rate:** 250ms
**Primary Use:** Active trading decisions

**Panels:**
1. **Orderflow Imbalance** (1m window)
   - Buy volume vs sell volume
   - Imbalance percentage: -100% (all sell) to +100% (all buy)
   - Net flow: $ per minute
   - Trend indicator: ↑↑ / ↑ / → / ↓ / ↓↓

2. **Spot vs Perp Basis**
   - Basis: perp_price - spot_price
   - Basis %: (basis / spot) × 100
   - CONTANGO (positive) vs BACKWARDATION (negative)
   - STEEP if >0.5%

3. **Liquidation Clusters**
   - Group liquidations by price level ($100 buckets)
   - Show concentration: which price has most $ at risk
   - DANGER ZONE: RED for high cascade risk
   - Include: long/short split count

4. **Funding Momentum**
   - Current funding rate with trend arrows
   - ↑↑↑ EXTREME if >0.04%
   - Longs pay vs Shorts pay indication

5. **Whale Detector** (Trades >$500K)
   - Real-time feed of large trades
   - Show: time, ticker, side, $ value, price, exchange
   - ⚠️ flag for mega trades >$5M

6. **CVD Divergence**
   - Compare: Price direction vs CVD direction
   - BULLISH: Price ↓ but CVD ↑ (accumulation)
   - BEARISH: Price ↑ but CVD ↓ (distribution)
   - ALIGNED: Price and CVD same direction

**Layout:**
```
┌─ ORDERFLOW IMBALANCE ──────────────────────────┬─ SPOT vs PERP BASIS ─────────┐
│ BTC  [████████░░] 73% BUY   Δ +$2.3M/min ↑   │ BTC  +$38 (0.04%) CONTANGO   │
│ ETH  [███░░░░░░░] 31% BUY   Δ -$1.1M/min ↓   │ ETH  -$12 (-0.32%) BACKWRD   │
│ SOL  [██████████] 92% BUY   Δ +$0.8M/min ↑↑  │ SOL  +$0.8 (0.52%) STEEP     │
├─ LIQUIDATION CLUSTERS ─────────────────────────┼─ FUNDING MOMENTUM ───────────┤
│ BTC: $94.5K █████ (127 longs)  DANGER ZONE    │ BTC: 0.012% ↑↑ LONGS PAY     │
│      $96.2K ███ (82 shorts)                   │ ETH: -0.008% ↓ SHORTS PAY    │
│      $93.8K ██ (45 longs)                     │ SOL: 0.045% ↑↑↑ EXTREME      │
├─ WHALE DETECTOR (>$500K) ──────────────────────┼─ CVD DIVERGENCE ─────────────┤
│ 10:32:15 BTC SELL $2.3M @$95.8K [BINANCE]    │ BTC: Price ↑ CVD ↓ BEARISH   │
│ 10:31:44 ETH BUY  $1.8M @$3.2K  [OKX]        │ ETH: Price ↓ CVD ↑ BULLISH   │
│ 10:30:22 BTC BUY  $5.1M @$95.7K [BYBIT] ⚠️   │ SOL: Price ↑ CVD ↑ ALIGNED   │
└────────────────────────────────────────────────┴──────────────────────────────┘
```

---

### TUI 2: Institutional Flow Monitor
**Binary:** `institutional-flow`
**Purpose:** Understanding smart money positioning
**Refresh Rate:** 1 second
**Primary Use:** Position sizing, trend confirmation

**Panels:**
1. **Smart Money Tracker**
   - Net flow (5min windows)
   - Aggressor ratio (buy/sell initiated)
   - Cumulative volume delta
   - Per exchange breakdown

2. **Exchange Dominance**
   - Volume share by exchange
   - OI share by exchange
   - Visual bars showing dominance
   - Migration tracking (which exchange gaining)

3. **Orderbook Depth Imbalance**
   - Bid quantity vs Ask quantity at multiple depth levels (1%, 2%, 5%)
   - Ratio: bid_qty / ask_qty
   - Pressure indicators
   - Interpretation: >2.0 = BUYERS DOMINANT, <0.5 = STRONG ASK

4. **Momentum Signals**
   - VWAP deviation: price vs VWAP
   - Tick direction: upticks vs downticks (percentage)
   - Trade size trend: is avg size increasing?
   - Time & Sales speed: trades/sec with intensity (HIGH/MEDIUM/LOW)

**Layout:**
```
┌─ SMART MONEY TRACKER ──────────────────────────────────────────────────────┐
│ ┌─ NET FLOW (5min) ─┐ ┌─ AGGRESSOR ──┐ ┌─ EXCHANGE DOMINANCE ────────────┐  │
│ │ BTC: +$12.3M  ↑↑  │ │ BUY:  68%    │ │ BINANCE: 45% ████████         │  │
│ │ ETH: -$3.2M   ↓   │ │ SELL: 32%    │ │ OKX:     28% █████            │  │
│ │ SOL: +$0.8M   →   │ │ Ratio: 2.1:1 │ │ BYBIT:   27% █████            │  │
│ └────────────────────┘ └──────────────┘ └─────────────────────────────────┘  │
├─ ORDERBOOK DEPTH IMBALANCE ────────────────────────────────────────────────┤
│ BTC  BID            ASK                                                        │
│ 1%   $4.2M ████    $1.8M ██       BUYERS 2.3x                                │
│ 2%   $8.1M ████████ $3.2M ███     STRONG BID                                 │
│ 5%   $15M  ███████████████ $12M   BALANCED                                  │
├─ MOMENTUM SIGNALS ──────────────────────────────────────────────────────────┤
│ • VWAP DEVIATION: BTC +0.3% above | ETH -0.1% below                           │
│ • TICK DIRECTION: ↑712 ↓423 (62% upticks)                                     │
│ • TRADE SIZE TREND: Increasing (avg $45K → $67K)                              │
│ • TIME&SALES SPEED: 182 trades/sec (HIGH)                                     │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

### TUI 3: Risk & Arbitrage Scanner
**Binary:** `risk-scanner`
**Purpose:** Risk management and opportunity detection
**Refresh Rate:** 5 seconds
**Primary Use:** Position monitoring, risk alerts

**Panels:**
1. **Liquidation Cascade Risk**
   - Next cascade level (price with % from current)
   - Volume at risk at that level ($ value)
   - Risk score: 0-100 with visual bar
   - Protection level (opposite side liquidations)

2. **Market Regime Detection**
   - State: TRENDING / RANGING / VOLATILE / RANGE-BOUND
   - Confidence: 0-100%
   - Volatility: realized vs implied comparison
   - Liquidity assessment (THIN/NORMAL/THICK)
   - Trend: direction detection

3. **Arbitrage Opportunities**
   - Spot-perp basis ($ and % with ⚠️ if >0.03%)
   - Exchange spreads ($ spread between exchanges)
   - Funding rate differentials

4. **Correlation Matrix**
   - BTC/ETH/SOL correlation matrix (0.0 to 1.0)
   - Cross-exchange correlation
   - Leading/lagging indicators

**Layout:**
```
┌─ RISK METRICS ─────────────────────────────┬─ ARBITRAGE OPPORTUNITIES ──────────┐
│ LIQUIDATION CASCADE RISK: ████░░ HIGH     │ SPOT-PERP: BTC +$38 (0.04%) ⚠️    │
│ Next Level: $94,200 (-1.2%) = $45M longs  │ EXCHANGE: OKX-BINANCE $12 spread  │
│ Protection: $96,800 (+1.5%) = $23M shorts │ FUNDING: SOL 0.045% vs 0.012% ARB │
├─ MARKET REGIME ────────────────────────────┼─ CORRELATION MATRIX ───────────────┤
│ State: RANGE-BOUND (82% confidence)        │     BTC  ETH  SOL                  │
│ Vol: DECREASING (realized < implied)       │ BTC  1.0  0.82 0.71                │
│ Trend: NEUTRAL (no clear direction)        │ ETH  0.82 1.0  0.78                │
│ Liquidity: THIN (holiday mode)             │ SOL  0.71 0.78 1.0                 │
└────────────────────────────────────────────┴─────────────────────────────────────┘
```

---

## 🔑 Key Improvements to Implement

### 1. Smart Aggregation
Instead of showing every trade, aggregate into meaningful windows:

```rust
struct AggregatedFlow {
    period: Duration,           // 1min, 5min
    net_buy_volume: f64,
    net_sell_volume: f64,
    large_trades: Vec<Trade>,   // Only >$100K
    vwap: f64,
    aggressor_ratio: f64,       // Buy vs sell initiated
}
```

### 2. Orderbook Reconstruction (Limited)
Estimate L2 from L1 + trades:

```rust
struct OrderbookEstimate {
    bid_pressure: f64,   // From trade flow direction
    ask_pressure: f64,
    imbalance: f64,      // (bid - ask) / (bid + ask)
    depth_ratio: f64,    // Estimated from liquidation levels
}
```

### 3. Critical Signals Enum

```rust
enum CriticalSignals {
    // Microstructure
    OrderFlowImbalance(f64),     // Buy vs sell pressure
    LiquidationHeatmap,           // Cluster detection
    VolumeProfile,                // Where volume occurs

    // Cross-Market
    SpotPerpBasis(f64),          // Arbitrage opportunity
    FundingMomentum,              // Direction change
    ExchangeNetFlow,              // Where smart money goes

    // Risk
    CascadeRisk(f64),            // Liquidation cascade probability
    MarketRegime,                 // Trending vs ranging
    LiquidityDepth,               // Thin vs thick markets
}
```

### 4. Visual Optimizations

**Compact Notation System:**
- `↑↑↑` = Strong bullish (>2 std dev)
- `↑↑`  = Bullish (>1 std dev)
- `↑`   = Mild bullish
- `→`   = Neutral
- `↓`   = Mild bearish
- `↓↓`  = Bearish (>1 std dev)
- `↓↓↓` = Strong bearish (>2 std dev)

**Color Coding:**
- `RED`    = Danger/Sell pressure
- `GREEN`  = Safe/Buy pressure
- `YELLOW` = Caution/Neutral
- `PURPLE` = Whale activity
- `CYAN`   = Arbitrage opportunity

**Progress Bars for Quick Scanning:**
- `[████████░░] 80%` = 80% buy pressure
- `[███░░░░░░░] 30%` = 30% sell pressure

### 5. Intelligent Filtering

```rust
struct NoiseFilter {
    min_trade_size: f64,         // Ignore < $10K
    aggregate_period: Duration,   // 1-5 min windows
    outlier_detection: bool,      // Flag unusual activity
    exchange_weight: HashMap,      // Weight by volume
}
```

### 6. Missing Data We Should Calculate

**From existing data streams:**
- **Trade Flow Imbalance** = (buy_volume - sell_volume) / total_volume
- **Funding Rate Momentum** = d(funding)/dt
- **Liquidation Clusters** = group by price levels ($100 buckets)
- **VWAP** = Σ(price * volume) / Σ(volume)
- **Tick Direction** = consecutive up vs down moves
- **Large Order Detection** = trades > 3σ from mean
- **Exchange Dominance** = volume per exchange / total
- **Spot-Perp Basis** = perp_price - spot_price
- **CVD Divergence** = price direction vs CVD direction
- **Aggressor Ratio** = buy_initiated_trades / sell_initiated_trades
- **Orderbook Depth Imbalance** = bid_qty / ask_qty
- **Volume Rate** = total_volume / time_window ($/sec)
- **Liquidation Rate** = liquidation_events / time_window (events/min)

---

## 📁 File Structure

```
barter-rs/
├── barter-data-server/
│   └── src/
│       └── main.rs                    # Fix: increase buffer, add lag handling
├── barter-data-tui/                   # Current TUI (will be deprecated or kept as simple view)
│   └── src/
│       └── main.rs                    # Fix: client lag handling, ping/pong
├── barter-market-microstructure/      # NEW: TUI 1
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs                    # Entry point
│       ├── aggregation.rs             # Shared data calculations
│       ├── orderflow.rs               # Orderflow metrics
│       ├── liquidations.rs            # Cluster detection
│       ├── whale.rs                   # Large trade detection
│       └── ui.rs                      # Ratatui rendering
├── barter-institutional-flow/         # NEW: TUI 2
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── smart_money.rs
│       ├── exchange_analytics.rs
│       └── ui.rs
├── barter-risk-scanner/               # NEW: TUI 3
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── cascade_risk.rs
│       ├── regime_detection.rs
│       └── ui.rs
└── barter-data-aggregation/           # NEW: Shared library
    ├── Cargo.toml
    └── src/
        ├── lib.rs
        ├── volume.rs                  # Volume calculations
        ├── windows.rs                 # Time window management
        ├── exchange_metrics.rs        # Per-exchange analytics
        └── types.rs                   # Shared data structures
```

---

## 🔧 Implementation Phases

**CRITICAL APPROACH:**
> "Less data, more intelligence. Every pixel should convey actionable information!" - Opus Design Philosophy

**Implementation Strategy:**
1. **FIRST:** Fix critical WebSocket bugs (Phase 1) - Nothing works if connections fail
2. **SECOND:** Build aggregation foundation (Phase 2) - All TUIs depend on this
3. **THEN:** Implement TUIs iteratively following Opus's priority:
   - Replace trade noise with aggregated intelligence (90% noise reduction)
   - Add orderflow imbalance and liquidation clusters
   - Implement spot-perp basis and funding tracking
   - Add whale detection and smart money tracking
   - Build risk metrics and regime detection

**Surgical Changes Only:**
- ✅ Analyze ALL dependencies before ANY code changes
- ✅ Make targeted, minimal edits to existing files
- ✅ Test after each phase before proceeding
- ❌ No breaking changes to existing functionality
- ❌ No premature optimization

---

### Phase 1: Critical Bug Fixes (PRIORITY)
**Est. Time:** 30-60 minutes
**Parallel:** No - must be sequential

**Tasks:**
1. ✅ Create feature branch: `feature/tui-enhancements-and-fixes`
2. Fix `barter-data-server/src/main.rs`:
   - Line 81: Change buffer from 1000 to 10,000
   - Lines 175-183: Add `RecvError::Lagged` handling
   - Send lag notification to client
3. Fix `barter-data-tui/src/main.rs`:
   - Lines 394-422: Handle errors gracefully
   - Add ping task (30s interval)
   - Differentiate connection errors from data errors
4. Test: Run TUI for 30+ minutes without disconnection
5. Commit: "fix: resolve WebSocket connection stability issues"

**Acceptance Criteria:**
- [ ] TUI runs for 30+ minutes without disconnecting
- [ ] Server logs show lag warnings instead of disconnections
- [ ] Client reconnects automatically if server restarts

---

### Phase 2: Shared Aggregation Library
**Est. Time:** 2-3 hours
**Parallel:** No - needed by all TUIs

**Tasks:**
1. Create `barter-data-aggregation` crate
2. Implement data structures:
   - `VolumeWindow` (1m, 5m, 15m)
   - `OrderflowMetrics`
   - `LiquidationCluster`
   - `ExchangeMetrics`
   - `AggregatedTotals`
3. Implement calculations:
   - VWAP calculation
   - Orderflow imbalance
   - Exchange dominance %
   - Volume rate ($/sec)
   - Aggressor ratio
4. Time