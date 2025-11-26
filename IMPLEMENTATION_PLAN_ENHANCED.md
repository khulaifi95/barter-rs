# Enhanced Implementation Plan - Trading TUIs v2.0

## Executive Summary

This plan supersedes the original `IMPLEMENTATION_PLAN.md` based on real-world testing and trading requirements. Key changes:

1. **Unified TUI Architecture** - Single process with view modes (not 5 separate binaries)
2. **Two Trading Modes** - Scalper (5s-30s) vs Intraday (1m-5m)
3. **Quick Wins First** - Improve existing TUI before building new features
4. **Actionable Intelligence** - Less text, more signals

### Global UX/Display Priority
- Make UI readability and actionability the top priority: keep blocks concise, use abbreviated labels, and at most 2–3 lines per ticker where possible.
- Use color sparingly to draw attention to important state/direction (arrows, alerts), not for decoration. Default to short forms (EXCH-KIND tags, compact numbers) and avoid wall-of-text.

---

## Current State Assessment

### What's Working ✅
- Binance/OKX/Bybit data flowing (after timeout fix)
- Shared state architecture (`barter-trading-tuis/src/shared/`)
- Basic panels rendering correctly
- Whale detection working
- Liquidation clusters displaying

### What's Weak ❌
| Panel | Issue | Impact |
|-------|-------|--------|
| **CVD Divergence** | Always shows NEUTRAL (45-55% band too wide) | No actionable signals |
| **Market Stats** | Too much text, hard to scan quickly | Slow decision making |
| **Spot vs Perp Basis** | Missing velocity/trend, static display | No alpha generation |
| **Missing** | No OI velocity, no delta momentum | Blind to institutional flow |

---

## Phase 1: Quick Wins (Current TUI1 Improvements)

**Priority: HIGH | Effort: LOW | Impact: HIGH**
**Timeline: Can be done incrementally**

### 1.1 CVD Divergence Panel Fixes

**Current:**
```
CVD DIVERGENCE
BTC: 5m +111.36M | Price ↑ CVD ↑  NEUTRAL  ← Always neutral!
```

**Enhanced:**
```
┌─ CVD MOMENTUM ─────────────────────────────────────────────────┐
│ BTC:                                                           │
│   30s: +$4.2M ↑↑  │ 1m: +$28M ↑  │ 5m: +$111M →               │
│   Signal: ACCUMULATION (price flat, CVD rising)                │
│                                                                │
│ ETH:                                                           │
│   30s: -$1.1M ↓   │ 1m: -$5M ↓   │ 5m: -$3M →                 │
│   Signal: DISTRIBUTION (price up, CVD falling)                 │
└────────────────────────────────────────────────────────────────┘
```

**Code Changes:**
```rust
// In state.rs - Add 30s window
pub cvd_30s: f64,
pub cvd_velocity_30s: f64,  // Rate of change

// Tighten neutral band from 45-55% to 48-52%
const NEUTRAL_BAND_LOW: f64 = 0.48;
const NEUTRAL_BAND_HIGH: f64 = 0.52;

// Add signal detection
enum FlowSignal {
    Accumulation,  // Price flat + CVD rising
    Distribution,  // Price up + CVD falling
    Exhaustion,    // Price moving + CVD flat
    Confirmation,  // Price and CVD aligned
    Neutral,
}
```

### 1.2 Market Stats Panel - Compact Format

**Current (Too verbose):**
```
MARKET STATS (5m)
BTC:
24050 t/5m  $696.3M  $29K/trade
OKX-PERP 97% | BNC-PERP 1% | BBT-PERP 1%
Speed: 115.3 t/s (HIGH)  Spread: 0.00%
```

**Enhanced (Scannable):**
```
┌─ MARKET PULSE ─────────────────────────────────────────────────┐
│      PRICE      VOL/5m   SPEED   DOMINANT    OI Δ5m           │
│ BTC  $87.8K    $696M    115/s   OKX 97%    +$12M ↑            │
│ ETH  $2.9K     $89M     153/s   OKX 82%    -$3M ↓             │
│ SOL  $137      $6.9M    57/s    OKX 49%    +$0.5M →           │
└────────────────────────────────────────────────────────────────┘
```

**Key Changes:**
- Single line per asset (not 3-4 lines)
- Add OI delta with direction
- Remove redundant "t/5m" and "$K/trade"
- Bold/color the dominant exchange

**UX Requirements (Market Stats):**
- Max 2 lines per asset (target 1 line) to preserve scanability.
- Show: trades/5m (abbrev), vol/5m (abbrev), avg $/trade, top-3 dominance with EXCH-KIND tags, OI Δ5m + arrow; Speed tag (HIGH/MED/LOW). Drop spread unless widened (>0.02%).
- Abbreviate numbers (e.g., $570M, 32k t/5m). Use short tags (OKX-PERP, BNC-SPOT, BBT-PERP). Limit text to avoid wrapping.
- Minimal color: color OI arrow only; dominance text stays plain to reduce noise. Speed can be colored but keep label.

### 1.3 Spot vs Perp Basis - Add Intelligence

**Current (Static):**
```
SPOT vs PERP BASIS
BTC  $+0.00 (+0.00%) NEUTRAL
ETH  $-1.31 (-0.04%) NEUTRAL
SOL  $-0.07 (-0.05%) BACKWRD
```

**Enhanced (Actionable):**
```
┌─ BASIS MOMENTUM ───────────────────────────────────────────────┐
│      BASIS     Δ1m    Δ5m    STATE      SIGNAL                │
│ BTC  +$38     +$12↑  +$45↑  CONTANGO   WIDENING → Long risk   │
│ ETH  -$1.31   -$0.2↓ -$0.8↓ BACKWRD    NARROWING → Squeeze?   │
│ SOL  -$0.07   +$0.01→ -$0.02→ BACKWRD  STABLE                  │
└────────────────────────────────────────────────────────────────┘
```

**UX Requirements (Basis):**
- Keep 1–2 lines per asset. Show: current basis ($, %), 1m/5m deltas with arrows, and a simple momentum tag (widening/narrowing/steady). Avoid prose.
- Abbreviate values; use arrows for direction; optional color on arrows only.

**Alpha Signals:**
- `WIDENING CONTANGO` = Perp longs paying up → liquidation risk
- `NARROWING BACKWRD` = Shorts covering → potential squeeze
- `RAPID CHANGE` = Perp-driven flow → fade or momentum entry

### 1.4 Add OI Intelligence (New Sub-Panel)

**Add to existing layout:**
```
┌─ OI VELOCITY ──────────────────────────────────────────────────┐
│      OI TOTAL    Δ5m      VELOCITY    SIGNAL                  │
│ BTC  $12.4B     +$45M↑   +$150K/s    LONGS BUILDING           │
│ ETH  $4.2B      -$12M↓   -$40K/s     SHORTS PRESSING          │
│ SOL  $890M      +$2M→    +$7K/s      BALANCED                 │
└────────────────────────────────────────────────────────────────┘
```

---

## Phase 2: Architecture Unification

**Priority: MEDIUM | Effort: MEDIUM | Impact: HIGH**

### Verification & Requirements (before coding)
- Health/observability: expose minimal metrics/logs (events/sec per exchange, active client count, reconnect/timeout counters) with acceptable thresholds.
- Resource targets: cap sockets per exchange; monitor CPU/RSS for the unified process; define acceptable event-to-render lag.
- Watchdog: on stalled feeds (timeout/idle), auto-reconnect and surface status in UI.
- Backward compatibility: existing TUIs/binaries remain usable; unified TUI is additive; no protocol breakage.
- Rollout: phased—build aggregator + new unified view, keep old TUIs intact; switch only after validation.
- L2 Depth Plan (book imbalance): add after current stability work.
  - Availability: Binance `depth20@100ms`, OKX `books5@100ms` (or `books@100ms` for deeper), Bybit `orderbook.50@20ms` (or `orderbook.200@200ms` slower).
  - Rationale: for scalping/intraday, speed > extreme depth; top 20–50 levels capture actionable liquidity.
  - Metrics: book imbalance within top N levels (per ticker; optional per venue), book flips, feed freshness.
  - Display:
    - TUI1 (Market Pulse): compact “Book: 63% BID” (or small bar) per ticker.
    - Scalper: small panel with book imbalance/flips; optionally per venue.
    - Risk Scanner: optional.
  - Defaults (tunable via env): Binance depth20@100ms; OKX books5@100ms (books@100ms if deeper needed); Bybit orderbook.50@20ms.
  - Effort: ~4–5h (server subs 2–3h; aggregation 1h; UI 30–60m; testing 30m).
  - Open choices: level counts per exchange (fast vs deep), update rates, aggregated vs per-venue view, configurable level counts.
  - Current limitation: Only Binance/Bybit L2 are wired; OKX L2 requires upstream barter-data work (channels + transformer + subscription validation). BOOK will show `--` for OKX until that’s added.

### 2.1 Single Binary with View Modes

**Replace 3 binaries with 1:**
```
barter-trading-tuis/src/bin/
├── trading_terminal.rs     # NEW: Unified TUI
├── market_microstructure.rs # Keep for backward compat
├── institutional_flow.rs    # Keep for backward compat
└── risk_scanner.rs          # Keep for backward compat
```

**Hotkey Navigation:**
```
[1] Market Pulse  - Current TUI1 (enhanced)
[2] Scalper Mode  - Fast execution (5s-30s windows)
[3] Flow Analysis - Institutional tracking
[4] Risk Scanner  - Liquidation/regime
[5] Dashboard     - Condensed all-in-one
[S] Split View    - Show 2 panels side-by-side
[B/E/O] Asset Focus - BTC/ETH/SOL only mode
```

### 2.2 Shared State Enhancement

```rust
// Enhanced shared state structure
pub struct UnifiedState {
    // Fast metrics (50ms update for scalper)
    pub fast: FastState,

    // Standard metrics (250ms update)
    pub standard: StandardState,

    // Slow metrics (1s update for risk)
    pub slow: SlowState,
}

pub struct FastState {
    pub delta_5s: HashMap<String, f64>,
    pub delta_15s: HashMap<String, f64>,
    pub delta_30s: HashMap<String, f64>,
    pub velocity_5s: HashMap<String, f64>,  // Rate of change
    pub imbalance_5s: HashMap<String, f64>, // Buy/sell ratio
    pub tape_speed: HashMap<String, f64>,   // Trades/sec
}
```

---

## Phase 3: Scalper Mode (New View)

**Priority: HIGH | Effort: MEDIUM | Impact: VERY HIGH**

### 3.1 Scalper Layout

```
┌─────────────────────────────────────────────────────────────────┐
│                      SCALPER MODE - BTC                         │
│                   Last: $87,842  Δ: +$12 (0.01%)               │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│   DELTA VELOCITY           IMBALANCE            TAPE            │
│   ══════════════           ═════════            ════            │
│                                                                 │
│      +$4.2M/s                 67%              142/s            │
│        ↑↑↑                  BUYERS              HIGH            │
│    ACCELERATING                                                 │
│                                                                 │
├─────────────────────────────────────────────────────────────────┤
│   5s: +$21M    15s: +$48M    30s: +$89M                        │
│   ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━                       │
│   SIGNAL: STRONG BUY MOMENTUM - Accelerating into resistance   │
├─────────────────────────────────────────────────────────────────┤
│   WHALE TAPE (last 30s):                                        │
│   → $2.1M BUY  @87,842  BNC  ← 0.3s                            │
│   → $1.8M SELL @87,840  OKX  ← 1.2s                            │
│   → $3.4M BUY  @87,838  BNC  ← 2.1s                            │
│   → $0.9M BUY  @87,835  BBT  ← 4.8s                            │
└─────────────────────────────────────────────────────────────────┘
│ [B]TC  [E]TH  [S]OL │  Refresh: 50ms  │  Latency: 12ms         │
```

### 3.2 Scalper-Specific Metrics

```rust
pub struct ScalperMetrics {
    // Delta velocity (most important)
    pub delta_velocity_5s: f64,   // $/second rate of change
    pub delta_acceleration: f64,  // Is velocity increasing?

    // Imbalance
    pub buy_volume_5s: f64,
    pub sell_volume_5s: f64,
    pub imbalance_pct: f64,       // -100 to +100

    // Tape metrics
    pub trades_per_second: f64,
    pub avg_trade_size_5s: f64,
    pub large_trade_ratio: f64,   // % of volume from >$100K trades

    // Whale tape
    pub recent_whales: VecDeque<(Instant, WhaleTrade)>,

    // Signal
    pub signal: ScalperSignal,
}

pub enum ScalperSignal {
    StrongBuy,      // Delta accel + imbalance >70% + whale buys
    Buy,            // Delta up + imbalance >55%
    WeakBuy,        // Imbalance >55% but delta flat
    Neutral,
    WeakSell,
    Sell,
    StrongSell,
}
```

---

## Phase 4: Enhanced Panels (Medium-Term)

### 4.1 Orderflow Panel Enhancement

**Current:**
```
ORDERFLOW IMBALANCE
BTC: 1m [ ████░░░░ ] 42% Δ -36.4M/min
     5m [ ██████░░ ] 58% Δ +21.9M/min  FADING
```

**Enhanced:**
```
┌─ ORDERFLOW ────────────────────────────────────────────────────┐
│ BTC:                                                           │
│   1m  [████░░░░░░] 42%  -$36M/min  ↓↓  SELLERS                │
│   5m  [██████░░░░] 58%  +$22M/min  ↑   FADING                 │
│   Δ:  1m→5m DIVERGING (short-term sell, medium-term buy)      │
│                                                                │
│ ETH:                                                           │
│   1m  [████░░░░░░] 40%  -$5M/min   ↓   SELLERS                │
│   5m  [█████░░░░░] 49%  -$0.5M/min →   BALANCED               │
│   Δ:  ALIGNED (consistent selling pressure)                   │
└────────────────────────────────────────────────────────────────┘
```

### 4.2 Liquidation Panel Enhancement

**Add cascade prediction:**
```
┌─ LIQUIDATION RISK ─────────────────────────────────────────────┐
│ BTC:                                                           │
│   Rate: 0.2/min │ Bucket: $100 │ Window: 10m                  │
│                                                                │
│   LONG RISK:  $94,200 (-0.8%) = $45M at risk  ████████░░      │
│   SHORT RISK: $96,800 (+1.5%) = $23M at risk  ████░░░░░░      │
│                                                                │
│   ⚠️ CASCADE ALERT: 3 more $1M+ liquidations trigger $94K     │
└────────────────────────────────────────────────────────────────┘
```

---

## Implementation Priority Matrix

| Item | Priority | Effort | Impact | Dependencies |
|------|----------|--------|--------|--------------|
| **1.1 CVD 30s + tighter band** | P0 | Low | High | None |
| **1.2 Market Stats compact** | P0 | Low | High | None |
| **1.3 Basis momentum** | P1 | Low | Medium | None |
| **1.4 OI velocity** | P1 | Medium | High | OI data flowing |
| **2.1 Unified binary** | P2 | Medium | High | Phase 1 |
| **3.1 Scalper mode** | P2 | High | Very High | Phase 2 |
| **4.x Enhanced panels** | P3 | Medium | Medium | Phase 2 |

---

## Code Change Summary

### Files to Modify

```
barter-trading-tuis/src/
├── shared/
│   └── state.rs           # Add 30s windows, velocity metrics
├── bin/
│   ├── market_microstructure.rs  # Update render functions
│   └── trading_terminal.rs       # NEW: Unified TUI
```

### Key Functions to Add/Modify

```rust
// state.rs additions
impl Aggregator {
    fn calculate_30s_metrics(&mut self) { ... }
    fn calculate_velocity(&self, window: Duration) -> f64 { ... }
    fn detect_flow_signal(&self, ticker: &str) -> FlowSignal { ... }
    fn calculate_basis_momentum(&self, ticker: &str) -> BasisMomentum { ... }
}

// New types
pub struct BasisMomentum {
    pub delta_1m: f64,
    pub delta_5m: f64,
    pub state: BasisState,      // Contango/Backwardation
    pub trend: BasisTrend,      // Widening/Narrowing/Stable
    pub signal: Option<String>, // "Long risk", "Squeeze setup", etc.
}

pub enum FlowSignal {
    Accumulation,
    Distribution,
    Exhaustion,
    Confirmation,
    Neutral,
}
```

---

## Success Metrics

After implementation, verify:

1. **CVD Panel**: Shows non-NEUTRAL signals at least 20% of the time
2. **Market Stats**: Each asset fits on ONE line (scannable in <1s)
3. **Basis Panel**: Shows directional arrows and actionable signals
4. **Scalper Mode**: Updates every 50ms with delta velocity
5. **Resource Usage**: Single binary uses <50% of 3 binaries combined

---

## Next Steps

1. **Immediate**: Implement Phase 1 quick wins (CVD band, Market Stats format)
2. **This Week**: Add OI velocity and basis momentum
3. **Next Week**: Create unified binary with view modes
4. **Following Week**: Build scalper mode with 5s-30s windows

---

## Appendix: Original vs Enhanced Comparison

### Original Plan (Opus 4.1)
- 3 separate TUI binaries
- Focus on many panels with lots of text
- 1m/5m windows only
- No velocity metrics

### Enhanced Plan (Opus 4.5)
- 1 unified binary with view modes
- Compact, scannable layouts
- 5s/15s/30s windows for scalping
- Delta velocity as primary metric
- Absorption/exhaustion detection
- Single-line per asset where possible

---

*Last Updated: 2025-11-25*
*Authors: Opus 4.5 + User feedback*
