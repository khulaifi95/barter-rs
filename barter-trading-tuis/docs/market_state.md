# Market State Engine Specification

> **Goal**: Build a deterministic state machine that synthesizes market signals into actionable WAIT/READY/CAUTION states with full auditability.

> **Philosophy**: Physics-First, Signal over Noise. Only high-quality signals that affect trade outcomes. No enterprise bloat.

---

## 0. Consolidated Requirements (Gemini/Codex Review)

### REJECTED / OUT OF SCOPE
| Item | Reason |
|------|--------|
| ❌ Microservices / gRPC | Single binary only. Lock-free primitives (ArcSwap, tokio::sync::watch) |
| ❌ Full Order Book Reconstruction | Track only BBO, CVD, Aggregated Whale Flow |
| ❌ Complex Liquidation Heatmap | Defer to future. Use simple OI-based "Warning" only |
| ❌ On-chain data | Too slow for microstructure |
| ❌ Historical liquidation levels | Over-engineered before proving edge |
| ❌ Z-Score only (no percentile) | Use **Dual-Vol**: 7-day percentile + 1m z-score |

### ELEVATED / CONFIRMED P0
| Item | Reason |
|------|--------|
| ✅ Options Integration (Phase 2) | Gamma Flip is structural physics, not optional |
| ✅ Dynamic Gamma Flip | Must account for IV shifts, not static OI |
| ✅ Dual-Vol Regime | 7-day percentile + 1m z-score shock (both required) |
| ✅ Non-blocking Audit | mpsc channel, never block hot path |

### AUDIT LAYER CLARIFICATION
- **Log only state FLIPS** (WAIT→READY, READY→CAUTION, etc.)
- **NOT every tick** - exchanges have that data
- **Bounded channel** (10,000 entries) - drop logs before blocking trades
- **Log rotation** - new file every 24h or 500MB
- **Expected volume** - ~400MB/day worst case (10 flips/sec × 500 bytes)

### DUAL-VOL REQUIREMENT (Gemini Feedback)
Use **BOTH** Percentile AND Sigma for L1 Regime:
- **Percentile (Context)**: Is this volatility normal for the 7-day regime?
- **Sigma (Shock)**: Is there a flash crash / black swan happening RIGHT NOW?

```
L1 PASS requires BOTH:
  ✓ Vol Percentile < 95th (not in structural chaos)
  ✓ 1-minute Z-Score < 3.5σ (no flash crash in progress)
```

### CODEX FEEDBACK: ACCEPTED
| Issue | Resolution |
|-------|------------|
| Signal time-alignment | Add **Freshness Gate** - max staleness per signal |
| No-Trad fallback | If ES/NQ stale, bypass macro filter (don't freeze) |
| Event ordering | Use **event time** (exchange timestamp), not arrival time |
| Gamma optional | Gamma Flip is **modifier**, not blocker. NO-GAMMA state allowed |

### CODEX FEEDBACK: REJECTED
| Issue | Reason for Rejection |
|-------|---------------------|
| Audit sampling | Already naturally sampled (only state flips logged) |
| Multi-binary | Single binary simpler; use Rust type system for coupling |

---

## 1. Executive Summary

### The Problem
Current TUIs show 50+ data points but force the trader to mentally synthesize them into a decision. This leads to:
- Analysis paralysis during fast moves
- Inconsistent decision-making
- No way to verify if signals were correct post-trade

### The Solution
A **State Engine** that applies hierarchical "kill logic":

```
L1: REGIME FILTER  →  "Can I trade at all?"
         ↓
L2: BIAS FILTER    →  "Which direction makes sense?"
         ↓
L3: TRIGGER + FUEL →  "Is now the right time and is the move real?"
         ↓
    STATE OUTPUT   →  WAIT / READY / CAUTION
```

If L1 fails, don't even evaluate L2. This prevents false positives.

---

## 2. Requirements

### 2.1 Functional Requirements

| ID | Requirement | Priority |
|----|-------------|----------|
| F1 | Calculate volatility regime (percentile vs 7-day history) | P0 |
| F2 | Integrate Gamma Flip price from Deribit options | P0 |
| F3 | Track funding rate velocity (not just rate) | P0 |
| F4 | Produce single MarketState output (WAIT/READY/CAUTION) | P0 |
| F5 | Log every state transition to audit file (JSONL) | P0 |
| F6 | Show confidence breakdown (why this state?) | P0 |
| F7 | L3 Fuel quality gate (RVOL + OI delta + liquidation rate) | P0 |
| F8 | Detect liquidation clusters from OI distribution | P1 |
| F9 | Session context (Asia/EU/US weighting) | P2 |

### 2.2 Non-Functional Requirements

| ID | Requirement | Target |
|----|-------------|--------|
| NF1 | State calculation latency | < 10ms |
| NF2 | Memory footprint | < 100MB |
| NF3 | Audit log write | Non-blocking, async |
| NF4 | State update frequency | Every 100ms |

### 2.3 Explicitly Out of Scope

- ❌ On-chain data (too slow for microstructure)
- ❌ Social sentiment (noise)
- ❌ News feeds (noise)
- ❌ Auto-execution (signals only)

---

## 2.4 Implementation Status (Current)

### Implemented (in code)
- **L3 Fuel gate**: RVOL (5m vs 1h), OI delta (5m, USD), liquidation rate (USD/min) integrated into state machine.
- **Funding velocity**: 15m rate change wired from rolling funding history (per-exchange average).
- **L3 UI split**: Trigger (consensus/absorption/funding) + Fuel (RVOL/OI/Liq) with icon status.
- **L2 imbalance + walls**: smoothed BID/ASK ratio, top 2 bid/ask walls within band.
- **Options context**: GEX/DEX/VEX, gamma flip, max pain, put/call walls, bucket summaries.
- **Liquidation thresholds**: calibrated to real-world rates (LOW < 150K/min, EXHT > 2M/min).
- **Config-driven thresholds**: all fuel thresholds live in `config/thresholds.toml`.
- **Centralized feeds**: Deribit options + IBKR trad ticks ingested by `barter-data-server`, TUIs no longer fetch directly.
- **Server snapshots**: `market_snapshot` events power L1/L3 fuel inputs (RVOL/OI/Funding/Liq) for consistent state decisions.
- **State reason**: human-readable reason displayed in state banner (READY/CAUTION/WAIT rationale).
- **Warnings strip**: renders Warning types + freshness ages + compact TradFi summary line.
- **Fuel warm-up**: L3 Fuel shows WARM (not 0.00) until sufficient history is available.
- **Price display**: prefer Binance perp last; fallback to snapshot price if stale.
- **IBKR logging**: tick throughput + silent disconnect detection in data server.

### Pending / Gaps
- **True 7-day percentile**: VolRegimeEngine exists but not yet wired into runtime; server snapshot still reports 0/unknown percentile.
- **Per-exchange drift**: Flow tables still use per-exchange rolling windows (kept for execution views).

### Trade-offs (Intentional)
- **L2 imbalance / walls** are context only (not part of state gating).
- **NO-OPTS / NO-T** mode: bias and state proceed from flow-only inputs when options/trad data unavailable.
- **Vol percentile fallback**: use trend-derived percentile when server does not provide history-backed percentile.

### Planned (Next)
- Replace trend-based percentile with **VolRegimeEngine** (168 hourly samples + 60 1m returns).
- Add IBKR health endpoint / liveness reporting (optional monitoring).

### Review Checkpoints (Thresholds)
- **Pre-centralization:** Re-validate RVOL/OI/liquidation/funding thresholds against live data in each TUI to ensure no drift.
- **Post-centralization:** Recalibrate using unified snapshots (single source of truth) and lock thresholds for production runs.

## 3. Architecture

### 3.1 Hierarchical Kill Logic

```
┌─────────────────────────────────────────────────────────────┐
│                    MARKET STATE ENGINE                      │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  L1: REGIME FILTER (Safety Gate)                           │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ Volatility Regime = ?                                │   │
│  │                                                      │   │
│  │ EXTREME (>95th percentile) → WAIT (full stop)       │   │
│  │ HIGH (80-95th)             → CAUTION (reduced size) │   │
│  │ NORMAL (20-80th)           → PROCEED to L2          │   │
│  │ LOW (<20th)                → PROCEED to L2          │   │
│  └─────────────────────────────────────────────────────┘   │
│                          ↓                                  │
│  L2: BIAS FILTER (Direction Gate)                          │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ Where is price vs Gamma Flip?                        │   │
│  │                                                      │   │
│  │ ABOVE Gamma Flip → BIAS = MEAN_REVERSION            │   │
│  │   • Fade breakouts, expect pullbacks                │   │
│  │   • Look for SHORT setups                           │   │
│  │                                                      │   │
│  │ BELOW Gamma Flip → BIAS = MOMENTUM                  │   │
│  │   • Follow breaks, ride trends                      │   │
│  │   • Look for continuation setups                    │   │
│  └─────────────────────────────────────────────────────┘   │
│                          ↓                                  │
│  L3: TRIGGER (Consensus Gate)                             │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ Does flow confirm the bias?                          │   │
│  │                                                      │   │
│  │ Check:                                               │   │
│  │  • CVD consensus (2/3+ venues agree)                │   │
│  │  • Funding velocity (not spiking against us)        │   │
│  │  • No absorption detected                           │   │
│  └─────────────────────────────────────────────────────┘   │
│                          ↓                                  │
│  L3: FUEL (Quality Gate)                                   │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ Is the move real (not just a squeeze)?               │   │
│  │                                                      │   │
│  │ Check:                                               │   │
│  │  • RVOL (5m vs 1h baseline)                          │   │
│  │  • OI delta (bias-aware)                             │   │
│  │  • Liquidation rate (exhaustion filter)              │   │
│  └─────────────────────────────────────────────────────┘   │
│                          ↓                                  │
│  L3 RESULT:                                                │
│   • READY if Trigger + Fuel both pass                    │
│   • CAUTION if either is partial                         │
│   • WAIT if either fails                                 │
│                          ↓                                  │
│  OUTPUT: MarketState { state, bias, confidence, reasons }  │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### 3.2 Data Flow

```
┌──────────────────────────────────────────────────────────────────┐
│                         DATA SOURCES                              │
├──────────────┬──────────────┬──────────────┬──────────────┬──────────────┤
│   Binance    │    Bybit     │     OKX      │    Deribit   │    IBKR      │
│   (spot+perp)│   (perp)     │    (perp)    │   (options)  │   (ES/NQ)    │
└──────┬───────┴──────┬───────┴──────┬───────┴──────┬───────┴──────┬───────┘
       │              │              │              │              │
       ▼              ▼              ▼              ▼              ▼
┌──────────────────────────────────────────────────────────────────┐
│                   barter-data-server (hub)                        │
│  • Aggregates raw feeds                                           │
│  • Emits raw events + market_snapshot                             │
└──────────────────────────┬───────────────────────────────────────┘
                           │
                           ▼
┌──────────────────────────────────────────────────────────────────┐
│                      MARKET SNAPSHOT                              │
│  • Per-venue: price, L2, CVD, flow imbalance, OI                  │
│  • Aggregated: consensus, whale flow, spread                      │
│  • Options: GEX/DEX/VEX, gamma flip, max pain, walls              │
│  • Volatility: ATR, RV, percentile, 1m z-score                    │
│  • Funding: current rate + 15m velocity                           │
│  • Fuel: RVOL (5m vs 1h), OI delta (USD), liq rate (USD/min)       │
└──────────────────────────┬───────────────────────────────────────┘
                           │
                           ▼
┌──────────────────────────────────────────────────────────────────┐
│                      STATE ENGINE                                 │
│                  (Hierarchical Kill Logic)                        │
└──────────────────────────┬───────────────────────────────────────┘
                           │
              ┌────────────┴────────────┐
              ▼                         ▼
┌─────────────────────────┐  ┌─────────────────────────┐
│      TUI VIEWS          │  │     AUDIT LOG           │
│  (render state)         │  │  (audit.jsonl)          │
└─────────────────────────┘  └─────────────────────────┘
```

### 3.3 Centralized Data Hub (Implemented)

**Now**: barter-data-server is the single aggregation hub for:
- Crypto perps (Binance/Bybit/OKX)
- Options (Deribit: gamma/GEX/DEX/VEX, walls, max pain)
- TradFi (IBKR ES/NQ)

**TUIs**: consume raw events + `market_snapshot` from the server (no direct Deribit/IBKR fetches).

**Note**: per-exchange flow/L2 windows remain local for execution detail; state inputs use the server snapshot.

### 3.4 TUI Roles (Harmonized)

- **trading-terminal** = **Macro cockpit** (state machine + gamma + fuel). Primary go/no-go decision screen.
- **scalper_v2** = **Execution cockpit** (tape speed, L2, trad markets, micro timing).
- **market_microstructure** = **Deep dive / debug** (whales, divergences, diagnostics).

---

## 4. Data Structures

### 4.1 Core Types

```rust
/// Primary output of the state engine
#[derive(Debug, Clone, Serialize)]
pub struct MarketState {
    /// Current state
    pub state: State,
    /// Directional bias (if state != Wait)
    pub bias: Option<TradingBias>,
    /// Confidence 0-100
    pub confidence: u8,
    /// Full breakdown for auditability
    pub components: StateComponents,
    /// Why this state? Human-readable
    pub reason: String,
    /// Unique ID for audit correlation
    pub audit_id: u64,
    /// Timestamp
    pub timestamp: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum State {
    /// Do not trade - conditions not met
    Wait,
    /// Conditions aligned - can execute
    Ready,
    /// Can trade but with elevated risk
    Caution,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum TradingBias {
    /// Price above gamma flip - fade moves
    MeanReversion,
    /// Price below gamma flip - follow moves
    Momentum,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum Direction {
    Long,
    Short,
    Neutral,
}
```

### 4.2 Component Scores (Auditable Breakdown)

```rust
/// Full breakdown of how state was determined
#[derive(Debug, Clone, Serialize)]
pub struct StateComponents {
    // L1: Regime
    pub vol_regime: VolRegimeScore,

    // L2: Bias
    pub gamma_context: GammaScore,

    // L3: Triggers
    pub flow_consensus: FlowScore,
    pub funding_context: FundingScore,
    pub fuel_context: FuelScore,

    // Options context (if available)
    pub options_context: Option<OptionsSummary>,

    // Warnings (don't block, but flag)
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VolRegimeScore {
    pub current_rv: f64,
    pub percentile: f64,        // 0-100, vs 7-day history
    pub regime: VolRegime,
    pub zscore_1m: f64,         // 1m return z-score
    pub is_shock: bool,
    pub passed: bool,           // Did it pass the filter?
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum VolRegime {
    Low,      // <20th percentile - quiet, range-bound
    Normal,   // 20-80th - standard conditions
    High,     // 80-95th - elevated, wider stops needed
    Extreme,  // >95th - crisis/euphoria, sit out
}

#[derive(Debug, Clone, Serialize)]
pub struct GammaScore {
    pub gamma_flip_price: f64,
    pub current_price: f64,
    pub distance_pct: f64,
    pub position: GammaPosition,
    pub bias: TradingBias,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum GammaPosition {
    AboveFlip,   // In positive gamma zone
    BelowFlip,   // In negative gamma zone
    AtFlip,      // Within 0.5% of flip (uncertain)
}

#[derive(Debug, Clone, Serialize)]
pub struct FlowScore {
    pub venues_agreeing: u8,
    pub venues_total: u8,
    pub consensus_direction: Direction,
    pub cvd_net: f64,
    pub absorption_detected: bool,
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FundingScore {
    pub current_rate: f64,
    pub rate_15m_ago: f64,
    pub velocity: f64,          // Change per 15m
    pub is_extreme: bool,       // >0.05% or <-0.02%
    pub is_spiking: bool,       // velocity > 0.02% in 15m
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FuelInput {
    pub rvol_5m: f64,            // vol_5m / (vol_1h / 12)
    pub oi_delta_usd_5m: f64,    // OI delta (USD)
    pub liq_rate_usd_per_min: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FuelScore {
    pub rvol: f64,
    pub rvol_status: RvolStatus,
    pub oi_delta_usd_5m: f64,
    pub oi_trend: OiTrend,
    pub liq_rate_usd_per_min: f64,
    pub liq_state: LiqState,
    pub quality: FuelQuality,
    pub passed: bool,
}

pub enum FuelQuality { High, Medium, Low, Fail }
pub enum RvolStatus { Strong, Normal, Thin, Fail }
pub enum OiTrend { NewMoney, Squeeze, Flat }
pub enum LiqState { Low, Moderate, High, Exhaustion }

#[derive(Debug, Clone, Serialize)]
pub enum Warning {
    ApproachingGammaFlip { distance_pct: f64 },
    FundingElevated { rate: f64 },
    LiquidationClusterNearby { price: f64, size_usd: f64 },
    SingleVenueDisagreeing { venue: String },
    LowLiquiditySession,
    StaleData { signal: Signal, age_ms: u64 },
    NoGammaData,
    NoTradMarketsData,
    ExpiryBucketConflict,
}
```

### 4.3 Options Context

```rust
/// Options-derived market context (from Deribit)
#[derive(Debug, Clone, Serialize)]
pub struct OptionsContext {
    /// Net gamma exposure in USD
    pub gexp: f64,
    /// Net delta exposure in USD
    pub dexp: f64,
    /// Net vega exposure in USD
    pub vexp: f64,
    /// The critical level: where gamma flips sign
    pub gamma_flip_price: f64,
    /// Max pain strike (price magnet near expiry)
    pub max_pain: f64,
    /// Nearest significant put wall (support)
    pub put_wall: Option<OptionsWall>,
    /// Nearest significant call wall (resistance)
    pub call_wall: Option<OptionsWall>,
    /// Put/Call OI ratio
    pub put_call_oi_ratio: f64,
    /// Expiry buckets (0-7d / 7-30d / 30d+)
    pub buckets: Vec<BucketSummary>,
    /// Hours until next major expiry
    pub hours_to_expiry: f64,
    /// Data freshness
    pub last_update: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct OptionsWall {
    pub strike: f64,
    pub notional_usd: f64,
    pub distance_pct: f64,
}

pub struct BucketSummary {
    pub label: String,          // "0-7d", "7-30d", "30d+"
    pub gamma_flip: f64,
    pub max_pain: f64,
    pub put_wall: Option<OptionsWall>,
    pub call_wall: Option<OptionsWall>,
    pub gex: f64,
    pub dex: f64,
    pub put_call_ratio: f64,
    pub hours_to_expiry: f64,
    pub is_front: bool,
}
```

---

## 5. Wireframes

### 5.1 Global Radar View (Primary)

**Note:** Current implementation splits L3 into **TRIGGER** (consensus/timing) and **FUEL** (RVOL/OI/Liq quality). Wireframes below should be read with that split in mind.

```
┌─ TRADING TERMINAL ──────────────────────────────────────────────────────────┐
│ BTC $92,552 │ DATA: BNC● BBT● OKX● DERB●  │ [1]Radar [2]Exec [3]Debug [Q]uit│
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│                    ████████  READY: SHORT  ████████                         │
│                         Confidence: 82%                                     │
│                                                                             │
├─ WHY THIS STATE ────────────────────────────────────────────────────────────┤
│                                                                             │
│  L1 REGIME     │  L2 BIAS           │  L3 TRIGGER                          │
│  ─────────────────────────────────────────────────────────────────────────  │
│  Vol: NORMAL   │  Gamma: ABOVE FLIP │  CVD: 3/3 SELL ✓                     │
│  Pctl: 45th ✓  │  → MEAN REVERSION  │  Funding: OK ✓                       │
│                │  Bias: SHORT       │  Absorption: None ✓                  │
│                │                    │                                       │
│  [PASS]        │  [PASS]            │  [PASS]                              │
│                                                                             │
├─ GAMMA CONTEXT ─────────────────────────────────────────────────────────────┤
│                                                                             │
│  GAMMA FLIP: $91,500                                                       │
│  YOU ARE:    $92,552 (+1.1% ABOVE)                                         │
│                                                                             │
│  │ NEGATIVE γ (momentum) │   FLIP   │ POSITIVE γ (mean revert) │           │
│  │◄━━━━━━━━━━━━━━━━━━━━━━│━━━━━━━━━━│━━━━━━━●━━━━━━━━━━━━━━━━━►│           │
│  │ $88K              $91.5K         $92.5K                $96K │           │
│                                      ↑ YOU                                  │
│                                                                             │
│  IMPLICATION: Fade breakouts. Expect reversion toward VWAP/Flip.           │
│                                                                             │
├─ FLOW CONSENSUS ────────────────────────────────────────────────────────────┤
│                                                                             │
│  VENUE     │  CVD 5m   │  FLOW 1m  │  WHALE 5m  │  VERDICT                 │
│  ──────────────────────────────────────────────────────────────            │
│  Binance   │  -$857K   │  44% SELL │  NEUTRAL   │  → SELL                  │
│  Bybit     │  -$281K   │  37% SELL │  SELL      │  → SELL                  │
│  OKX       │  -$164K   │  41% SELL │  NEUTRAL   │  → SELL                  │
│  ──────────────────────────────────────────────────────────────            │
│  CONSENSUS │  3/3 SELL (100%)                                              │
│                                                                             │
├─ VOLATILITY & FUNDING ──────────────────────────────────────────────────────┤
│                                                                             │
│  VOLATILITY                         │  FUNDING                              │
│  RV(1h): 0.04%  Pctl: 45th NORMAL  │  Rate: 0.010%                         │
│  ATR:    $38    BVOL: 1.50         │  15m Δ: +0.002% (stable)              │
│  Trend:  STABLE (RV30≈RV1h)        │  Status: NEUTRAL                      │
│                                                                             │
├─ WARNINGS ──────────────────────────────────────────────────────────────────┤
│  (none)                                                                     │
├─────────────────────────────────────────────────────────────────────────────┤
│  Last audit: #4829 @ 13:45:02.451  │  States today: 142 WAIT, 28 READY     │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 5.2 Compact Mode (Minimal)

```
┌─ BTC $92,552 ───────────────────────────────────────────────────────────────┐
│                                                                             │
│  ████ READY: SHORT 82% ████  │  Vol:NORMAL │ γ:ABOVE │ Flow:3/3 SELL       │
│                                                                             │
│  Gamma Flip: $91.5K (+1.1%)  │  Funding: 0.01% (stable)  │  RV: 45th pctl  │
│                                                                             │
│  Reason: Above gamma flip + unanimous sell flow → fade longs               │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 5.3 Execution View (When READY)

```
┌─ EXECUTION COCKPIT ─────────────────────────────────────────────────────────┐
│ DIRECTION: SHORT │ CONFIDENCE: 82% │ BIAS: MEAN REVERSION                   │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  VENUE RANKING (Best to Worst)                                             │
│  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━              │
│  #1 BYBIT    │ Spread: 0.00% │ L2: 71% BID │ Score: 91 ◀ RECOMMENDED       │
│  #2 BINANCE  │ Spread: 0.01% │ L2: 65% BID │ Score: 78                     │
│  #3 OKX      │ Spread: 0.02% │ L2: 58% BID │ Score: 65                     │
│                                                                             │
├─ BYBIT DETAIL ──────────────────────────────────────────────────────────────┤
│                                                                             │
│  L2 BOOK                           │  RECENT WHALES                        │
│  $92,600 [████████░░] 180 ASK      │  $616K SELL @92562 27s                │
│  $92,580 [██████░░░░] 120 ASK      │  $607K SELL @92562 27s                │
│  $92,560 ══ MID ══════════════     │                                       │
│  $92,540 [████████░░] 165 BID      │  Net 5m: -$1.8M SELL                  │
│  $92,520 [██████████] 220 BID ◀    │                                       │
│                                                                             │
│  Spread: $0 (0.00%)  │  Depth: 71% BID  │  Trades: 85/s                    │
│                                                                             │
├─ OPTIONS LEVELS ────────────────────────────────────────────────────────────┤
│                                                                             │
│  PUT WALL:  $90,000 ($85M notional) - 2.8% below                           │
│  CALL WALL: $95,000 ($120M notional) - 2.6% above                          │
│  MAX PAIN:  $91,000 - 1.7% below                                           │
│                                                                             │
│  → Expect price to stay in $90K-$95K range (options pinning)               │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│  [G] Back to Global Radar                                                   │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 6. Implementation Phases (Revised per Gemini/Codex)

### Phase 1: The Core Brain (P0) ⬅ START HERE
**Duration: 3-4 days**

| Task | File | Description |
|------|------|-------------|
| 1.1 | `src/shared/market_state.rs` | Core types: MarketState, State, TradingBias, Direction |
| 1.2 | `src/shared/market_state.rs` | Component types: VolRegimeScore, GammaScore, FlowScore, FundingScore, FuelScore |
| 1.3 | `src/shared/market_state.rs` | `calculate_state()` with hierarchical KILL logic |
| 1.4 | `src/shared/vol_regime.rs` | **Percentile-based** regime (7-day rolling rank, NOT z-score) |
| 1.5 | `src/shared/audit.rs` | Non-blocking JSONL logger (mpsc channel, bounded 10K) |
| 1.6 | Tests | Unit tests for state transitions |

**Kill Logic Implementation:**
```rust
// L1: If Vol > 95th percentile → WAIT (full stop, don't evaluate L2/L3)
// L2: Price vs Gamma Flip → determines BIAS (MeanReversion vs Momentum)
// L3: CVD consensus + funding velocity + fuel gate (RVOL/OI/Liq) → READY/CAUTION
```

**Deliverable:** `MarketState::calculate()` produces WAIT/READY/CAUTION with auditable reasons.

### Phase 2: The Structural Physics (P0)
**Duration: 2-3 days**

| Task | File | Description |
|------|------|-------------|
| 2.1 | `src/shared/options_state.rs` | OptionsContext struct |
| 2.2 | `src/shared/deribit.rs` | Deribit REST client (fetch every 60s) |
| 2.3 | `src/shared/gamma.rs` | **Dynamic** Gamma Flip (accounts for IV shifts) |
| 2.4 | `src/shared/gamma.rs` | Put/Call wall identification (nearest significant strikes) |
| 2.5 | Integration | Wire options into state engine L2 filter |

**Key Requirement:** Gamma Flip must be DYNAMIC, not static OI snapshot.

**Deliverable:** Gamma flip price integrated, updates with IV changes.

### Phase 3: Micro-Flow (P0)
**Duration: 1-2 days**

| Task | File | Description |
|------|------|-------------|
| 3.1 | `src/shared/funding.rs` | FundingTracker with 15m ring buffer |
| 3.2 | `src/shared/funding.rs` | Velocity calc: `Δ funding > 0.02%/15m` = spike |
| 3.3 | Integration | RVOL calculation (5m vs 1h) |
| 3.4 | Integration | OI delta (USD, 5m) + trend classification |
| 3.5 | Integration | Liquidation rate (USD/min) + state classification |
| 3.6 | Integration | L3 Trigger (CVD + funding + absorption) + L3 Fuel gate |
| 3.7 | Integration | CVD consensus: require 2/3 venues to agree |

**Deliverable:** L3 Trigger + Fuel gates drive READY/CAUTION/WAIT using consensus + funding + RVOL/OI/Liq.

### Phase 4: Unified TUI (P0)
**Duration: 3-4 days**

| Task | File | Description |
|------|------|-------------|
| 4.1 | `src/bin/trading_terminal.rs` | Single binary entry point |
| 4.2 | `src/views/mod.rs` | View trait + state sharing via `Arc<ArcSwap<MarketState>>` |
| 4.3 | `src/views/global_radar.rs` | Global Radar view (primary) |
| 4.4 | `src/views/execution.rs` | Execution Cockpit view |
| 4.5 | `src/views/debug.rs` | Microstructure debug view (port existing) |
| 4.6 | Integration | Tab switching [1][2][3], single WebSocket set |

**Key Requirement:** Use `ArcSwap` or `tokio::sync::watch` for lock-free state sharing. No mutex on hot path.

**Deliverable:** Single binary with instant view switching.

### Phase 5: Simple Warnings (P1) - DEFERRED
**Duration: 1 day (when needed)**

| Task | File | Description |
|------|------|-------------|
| 5.1 | `src/shared/warnings.rs` | Simple OI concentration check |
| 5.2 | Integration | Add as Warning, NOT complex heatmap |

**Scope:** Just flag "High OI concentration at $91K" - no historical reconstruction.

**Deliverable:** Simple warning text, not visualization.

---

## 7. Testing Strategy

### 7.1 Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extreme_vol_forces_wait() {
        let snapshot = MockSnapshot::with_vol_percentile(98.0);
        let state = MarketState::calculate(&snapshot, None);

        assert_eq!(state.state, State::Wait);
        assert!(state.reason.contains("EXTREME volatility"));
    }

    #[test]
    fn test_above_gamma_flip_sets_mean_reversion() {
        let snapshot = MockSnapshot::default();
        let options = OptionsContext {
            gamma_flip_price: 90_000.0,
            ..Default::default()
        };
        snapshot.price = 92_000.0; // Above flip

        let state = MarketState::calculate(&snapshot, Some(&options));

        assert_eq!(state.bias, Some(TradingBias::MeanReversion));
    }

    #[test]
    fn test_funding_spike_triggers_caution() {
        let snapshot = MockSnapshot::default();
        snapshot.funding.velocity = 0.03; // 3% in 15m = spike

        let state = MarketState::calculate(&snapshot, None);

        assert_eq!(state.state, State::Caution);
        assert!(state.components.warnings.iter()
            .any(|w| matches!(w, Warning::FundingElevated { .. })));
    }

    #[test]
    fn test_unanimous_consensus_with_good_regime_is_ready() {
        let snapshot = MockSnapshot::builder()
            .vol_percentile(50.0)      // Normal
            .venue_consensus(3, 3)      // Unanimous
            .cvd_direction(Direction::Short)
            .build();

        let options = OptionsContext {
            gamma_flip_price: 90_000.0, // Price above = mean reversion
            ..Default::default()
        };
        snapshot.price = 92_000.0;

        let state = MarketState::calculate(&snapshot, Some(&options));

        assert_eq!(state.state, State::Ready);
        assert_eq!(state.confidence, 82); // High confidence
    }
}
```

### 7.2 Integration Tests

```rust
#[tokio::test]
async fn test_state_engine_with_live_feeds() {
    // Connect to test WebSocket feeds
    // Verify state transitions occur correctly
    // Check audit log entries
}

#[tokio::test]
async fn test_audit_log_write() {
    let state = MarketState { /* ... */ };
    let logger = AuditLogger::new("test_audit.jsonl");

    logger.log(&state).await;

    let content = std::fs::read_to_string("test_audit.jsonl").unwrap();
    let entry: AuditEntry = serde_json::from_str(&content).unwrap();

    assert_eq!(entry.audit_id, state.audit_id);
}
```

### 7.3 Verification Criteria

| Criterion | Method |
|-----------|--------|
| State accuracy | Compare state output vs historical outcomes |
| Latency | Measure calculate_state() execution time |
| Audit completeness | Every state change has JSONL entry |
| Regime correctness | Backtest vol percentile vs known events |
| Gamma flip accuracy | Compare to Laevitas/Amberdata values |

---

## 8. Audit Log Format (Non-Blocking Implementation)

### 8.1 Architecture: Never Block the Hot Path

**EXPLICIT REQUIREMENTS:**
- ✅ **Buffered:** Bounded mpsc channel (10,000 entries)
- ✅ **Non-blocking:** `try_send()` - never wait, drop if full
- ✅ **Kill-switch:** If channel >90% full, log warning and drop aggressively
- ✅ **Rate-limited:** Max 100 logs/second (state flips are rare, but protect against bugs)

```
┌─────────────────────────────────────────────────────────────────┐
│                      HOT PATH (Trading Logic)                   │
│                                                                 │
│  MarketState::calculate() → state_changed? → audit_tx.send()   │
│                                              ↓                  │
│                                    ~100-200ns (non-blocking)    │
│                                                                 │
│  GUARANTEE: This path NEVER waits for I/O                      │
└─────────────────────────────────────────────────────────────────┘
                                              │
                                              ↓
                               ┌──────────────────────────────┐
                               │  mpsc::channel (bounded 10K) │
                               │                              │
                               │  KILL-SWITCH:                │
                               │  >9000 entries = DROP new    │
                               │  logs + emit warning         │
                               └──────────────────────────────┘
                                              │
                                              ↓
┌─────────────────────────────────────────────────────────────────┐
│                      COLD PATH (Background Task)                │
│                                                                 │
│  loop { audit_rx.recv() → serde_json → write to file }         │
│                                                                 │
│  • Runs in dedicated tokio task                                │
│  • Slow I/O happens here, not on hot path                      │
│  • Rate limit: process max 100/sec (batch if needed)           │
│  • If channel full, DROP logs (never block trades)             │
└─────────────────────────────────────────────────────────────────┘
```

**Latency Impact:** ZERO on hot path. Audit logging is fully decoupled.

### 8.2 Rust Implementation

```rust
use tokio::sync::mpsc;
use std::fs::{File, OpenOptions};
use std::io::Write;

const AUDIT_CHANNEL_SIZE: usize = 10_000;

/// The audit entry - only logged on STATE FLIPS
#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    pub audit_id: u64,
    pub timestamp: i64,
    pub ticker: String,
    pub price: f64,

    // State transition
    pub prev_state: State,
    pub new_state: State,

    // The "Physics" that caused this
    pub bias: Option<TradingBias>,
    pub confidence: u8,

    // L1: Volatility
    pub vol_percentile: f64,
    pub vol_regime: VolRegime,

    // L2: Gamma
    pub gamma_flip_price: Option<f64>,
    pub price_vs_flip_pct: Option<f64>,

    // L3: Flow
    pub cvd_consensus: String,  // "3/3 SELL"
    pub funding_rate: f64,
    pub funding_velocity: f64,

    // Human readable
    pub reason: String,
}

pub struct AuditLogger {
    tx: mpsc::Sender<AuditEntry>,
}

impl AuditLogger {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(AUDIT_CHANNEL_SIZE);

        // Spawn background writer task
        tokio::spawn(Self::writer_task(rx));

        Self { tx }
    }

    /// Non-blocking send - returns immediately
    /// Uses try_send to never block; drops if channel full
    pub fn log(&self, entry: AuditEntry) {
        // try_send is non-blocking - if channel full, log is dropped
        let _ = self.tx.try_send(entry);
    }

    async fn writer_task(mut rx: mpsc::Receiver<AuditEntry>) {
        let mut current_file = Self::open_log_file();
        let mut current_date = chrono::Utc::now().date_naive();

        while let Some(entry) = rx.recv().await {
            // Check for date rollover
            let today = chrono::Utc::now().date_naive();
            if today != current_date {
                current_file = Self::open_log_file();
                current_date = today;
            }

            // Serialize and write
            if let Ok(json) = serde_json::to_string(&entry) {
                let _ = writeln!(current_file, "{}", json);
            }
        }
    }

    fn open_log_file() -> File {
        let date = chrono::Utc::now().format("%Y-%m-%d");
        let path = format!("audit_{}.jsonl", date);
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("Failed to open audit log")
    }
}
```

### 8.3 What Gets Logged (and What Doesn't)

| Event | Logged? | Reason |
|-------|---------|--------|
| State flip (WAIT→READY) | ✅ YES | Core audit trail |
| State flip (READY→CAUTION) | ✅ YES | Core audit trail |
| Confidence change (82→85) | ❌ NO | Too noisy, same state |
| Price tick | ❌ NO | Exchange has this |
| L2 book update | ❌ NO | Too frequent |
| Vol percentile crosses threshold | ✅ YES | Regime change |
| Gamma flip price update | ✅ YES | Physics changed |

### 8.4 JSONL Schema (Minimal)

```json
{
  "audit_id": 4829,
  "timestamp": 1736088302451,
  "ticker": "BTC",
  "price": 92552.50,
  "prev_state": "WAIT",
  "new_state": "READY",
  "bias": "MEAN_REVERSION",
  "confidence": 82,
  "vol_percentile": 45,
  "vol_regime": "NORMAL",
  "gamma_flip_price": 91500,
  "price_vs_flip_pct": 1.15,
  "cvd_consensus": "3/3 SELL",
  "funding_rate": 0.0001,
  "funding_velocity": 0.00002,
  "reason": "Above gamma flip + unanimous sell flow"
}
```

### 8.5 Log Rotation

- **New file each day:** `audit_2026-01-05.jsonl`
- **Max size:** 500MB per file (rotate mid-day if exceeded)
- **Retention:** 30 days
- **Compression:** gzip files older than 7 days

---

## 9. Freshness Gate & Data Availability (Codex Requirement)

### 9.1 Freshness Gate Table

Every signal has a **maximum allowed staleness**. If exceeded, the signal is marked `STALE` and handled per fallback rules.

| Signal | Max Staleness | Fallback if Stale |
|--------|---------------|-------------------|
| Price (BBO) | 1 second | **WAIT** - Cannot trade without price |
| CVD / Order Flow | 2 seconds | **WAIT** - Core signal unavailable |
| L2 Book Imbalance | 2 seconds | Use last known, flag as degraded |
| Whale Trades | 10 seconds | Ignore whale filter, proceed |
| Funding Rate | 5 minutes | Use last known value |
| Gamma Flip (Deribit) | 10 minutes | **NO-GAMMA mode** - proceed without L2 bias |
| ES/NQ (Trad Markets) | 2 minutes | **NO-TRAD mode** - bypass macro filter |
| Volatility History | 1 hour | Fallback to trend-based percentile until VolRegimeEngine wired |

### 9.2 NO-TRAD Fallback Mode

If traditional market data (ES/NQ) is stale or unavailable:

```rust
pub enum TradMarketStatus {
    Live,           // Data fresh, use macro divergence filter
    Stale,          // Data old, bypass macro filter
    Unavailable,    // No connection, bypass macro filter
}

// In calculate_state():
if trad_status != TradMarketStatus::Live {
    // Skip L2 macro divergence check
    // Log: "NO-TRAD mode: macro filter bypassed"
    // Still apply L1 (vol) and L3 (flow) filters
}
```

### 9.3 NO-GAMMA Fallback Mode

If Deribit options data is stale or unavailable:

```rust
pub enum GammaStatus {
    Live { flip_price: f64 },  // Use gamma for bias
    Stale,                      // Use last known flip
    Unavailable,                // No gamma data - use flow-only bias
}

// In calculate_state():
if gamma_status == GammaStatus::Unavailable {
    // Determine bias from CVD/flow direction only
    // Do NOT block READY state
    // Log: "NO-GAMMA mode: bias from flow only"
}
```

### 9.4 Data Availability Matrix

| Scenario | Price | CVD | Gamma | TradMkts | Result |
|----------|-------|-----|-------|----------|--------|
| All Live | ✅ | ✅ | ✅ | ✅ | Full state calculation |
| No Gamma | ✅ | ✅ | ❌ | ✅ | NO-GAMMA mode, flow-based bias |
| No TradMkts | ✅ | ✅ | ✅ | ❌ | NO-TRAD mode, skip macro filter |
| No CVD | ✅ | ❌ | ✅ | ✅ | **WAIT** - cannot determine consensus |
| No Price | ❌ | * | * | * | **WAIT** - cannot trade |

### 9.5 Event Time Ordering

**Critical:** Use exchange timestamp (`event_time`), NOT arrival time.

```rust
// WRONG: Race condition prone
let cvd = self.cvd_at_arrival_time();

// CORRECT: Deterministic
let cvd = self.cvd_at_event_time(event_ts);
```

This prevents late-arriving messages from causing spurious state flips.

---

## 10. Evaluation Metrics (Codex Requirement)

### 10.1 Success Metric Definition

The ">65% accuracy" target is defined as:

**Metric:** Precision of READY signals
**Definition:** Of all READY states emitted, what percentage led to a profitable trade zone?

**Label Definition (EXPLICIT):**
- **Horizon:** 5 minutes post-signal
- **Profitable:** Price moved ≥0.1% in the bias direction
- **Unprofitable:** Price moved ≥0.1% AGAINST the bias direction
- **Neutral:** Price moved <0.1% either way (excluded from precision calc)

```
Precision = (READY signals → price moved ≥0.1% in bias direction within 5 min)
            / (READY signals → price moved ≥0.1% in either direction within 5 min)

Target: Precision >= 65%
```

**Alternative Horizons for Validation:**
| Horizon | Use Case | Expected Precision |
|---------|----------|-------------------|
| 1 minute | Scalping validation | ≥55% |
| 5 minutes | **Primary metric** | ≥65% |
| 15 minutes | Swing validation | ≥60% |

### 10.2 Evaluation Methodology

| Metric | Definition | Target |
|--------|------------|--------|
| **Precision** | READY → price moves ≥0.1% in bias direction within 5 min | ≥65% |
| **Recall** | Of all 5-min windows with ≥0.3% move, how many had READY? | ≥40% |
| **False Positive Rate** | READY → price moves ≥0.1% AGAINST bias within 5 min | ≤35% |
| **State Flip Frequency** | Average flips per hour | 5-20 |

### 10.3 Baseline

Compare against:
- **Random baseline**: 50% precision (coin flip)
- **Always-READY baseline**: ~45% precision (most moves are noise)
- **Vol-only filter**: ~55% precision

Target of 65% represents meaningful edge over baselines.

---

## 11. Configuration Management

### 11.1 Single Source of Truth

**Location:** `barter-trading-tuis/config/thresholds.toml`

All thresholds in ONE file. No thresholds hardcoded in Rust source.

```toml
[volatility]
percentile_extreme = 95    # L1 kill threshold
percentile_high = 80       # Caution threshold
zscore_shock = 3.5         # Flash crash threshold

[consensus]
min_agreement_pct = 66     # 2/3 venues must agree

[funding]
spike_threshold = 0.0002   # 0.02% velocity = spike
extreme_long = 0.0005      # >0.05% = extreme
extreme_short = -0.0001    # <-0.01% = extreme

[fuel]
rvol_strong_min = 2.0      # >= 2.0x = strong conviction
rvol_normal_min = 0.8      # >= 0.8x = normal activity
rvol_thin_min = 0.5        # >= 0.5x = thin volume
oi_momentum_fail_usd = -15000000   # <= -$15M = squeeze risk (momentum)
oi_momentum_caution_usd = -5000000 # <= -$5M = caution (momentum)
oi_flat_usd = 3000000              # <= $3M = flat/neutral
oi_new_money_min_usd = 5000000     # >= $5M = new money
liq_caution_usd_per_min = 150000   # >= $150K/min = caution (elevated)
liq_fail_usd_per_min = 2000000     # >= $2M/min = exhaustion

[freshness_ms]
price = 1000
cvd = 2000
l2_book = 2000
whale = 10000
funding = 300000           # 5 minutes
gamma = 600000             # 10 minutes
trad_markets = 120000      # 2 minutes

[gamma]
flip_buffer_pct = 0.5      # Within 0.5% of flip = "AT_FLIP"

[audit]
channel_size = 10000       # Bounded mpsc buffer
log_dir = "logs/audit"     # Audit log directory
rotation_mb = 500          # Rotate at 500MB
retention_days = 30        # Keep 30 days
```

### 11.2 Per-Ticker Overrides

**Precedence:** Ticker-specific > Default

```toml
[ticker.BTC]
# BTC uses defaults (no overrides needed)

[ticker.ETH]
# ETH tends to have higher funding extremes
funding.extreme_long = 0.0008

[ticker.SOL]
# SOL is more volatile, lower threshold
volatility.percentile_extreme = 90
```

### 11.3 Config Loading

```rust
/// Load config with precedence: ticker-specific > default
pub fn load_thresholds(ticker: &str) -> Thresholds {
    let config: Config = toml::from_str(&std::fs::read_to_string(
        "config/thresholds.toml"
    ).expect("Missing config file")).unwrap();

    // Start with defaults
    let mut thresholds = config.defaults.clone();

    // Apply ticker-specific overrides
    if let Some(ticker_config) = config.ticker.get(ticker) {
        thresholds.merge(ticker_config);
    }

    thresholds
}
```

### 11.4 Hot Reload (Optional)

Config can be reloaded without restart via `SIGHUP` or file watcher:

```rust
// Watch for config changes (optional, Phase 4+)
notify::recommended_watcher(|res| {
    if let Ok(_) = res {
        CONFIG.store(Arc::new(load_thresholds(TICKER)));
        info!("Config reloaded");
    }
});
```

---

## 12. Risk Considerations

| Risk | Mitigation |
|------|------------|
| Deribit API rate limits | Cache options data, refresh every 60s |
| Stale options data | Show data age, warn if >5 minutes old |
| False READY signals | Require ALL L3 triggers to pass |
| Network latency | Show data health indicators per feed |
| Gamma flip calculation errors | Cross-validate with external sources |

---

## 10. Success Metrics

| Metric | Target | Measurement |
|--------|--------|-------------|
| READY signal accuracy | >65% profitable | Backtest + forward test |
| State calculation latency | <10ms p99 | Instrumentation |
| Audit coverage | 100% of state changes | Log analysis |
| False positive rate | <20% | READY → losing trade |
| User cognitive load | "Know what to do in <2 seconds" | Qualitative |

---

## 11. Dependencies

### External APIs

| Service | Purpose | Fallback |
|---------|---------|----------|
| Deribit | Options data (GEXP, gamma flip) | Laevitas, Amberdata |
| Binance | Spot + perp data | Primary, no fallback |
| Bybit | Perp data | Secondary venue |
| OKX | Perp data | Secondary venue |

### Rust Crates

| Crate | Purpose |
|-------|---------|
| `tokio` | Async runtime |
| `ratatui` | TUI rendering |
| `crossterm` | Terminal backend |
| `serde` / `serde_json` | Serialization |
| `reqwest` | HTTP client (Deribit REST) |
| `tokio-tungstenite` | WebSocket client |
| `tracing` | Logging |

---

## 12. Open Questions

1. **Gamma flip source**: Calculate from raw options chain or use pre-calculated from Laevitas/Amberdata?
2. **Historical vol storage**: In-memory ring buffer or persist to SQLite?
3. **Multi-ticker**: Start with BTC only or include ETH from day 1?
4. **Session weighting**: Include in Phase 1 or defer to Phase 5?

---

## Appendix A: Dual-Vol Filter (Percentile + Sigma)

**Why BOTH Percentile AND Sigma?**

Crypto volatility requires a **compound filter**:
- **Percentile** = Context: "Is this volatility normal for the 7-day regime?"
- **Sigma/Z-Score** = Shock: "Is there a black swan happening RIGHT NOW?"

```
┌─────────────────────────────────────────────────────────────────┐
│                    L1: DUAL-VOL FILTER                          │
│                                                                 │
│   ┌─────────────────────┐    ┌─────────────────────┐           │
│   │ PERCENTILE (7-day)  │    │ SIGMA (1-minute)    │           │
│   │                     │    │                     │           │
│   │ < 95th? ✓           │ AND│ < 3.5σ? ✓           │ → PASS    │
│   │ ≥ 95th? ✗           │    │ ≥ 3.5σ? ✗           │ → WAIT    │
│   └─────────────────────┘    └─────────────────────┘           │
│                                                                 │
│   BOTH must pass. Either failing = WAIT.                       │
└─────────────────────────────────────────────────────────────────┘
```

### A.1 Percentile: Regime Detection

```rust
/// Rolling 7-day volatility history (168 hourly samples)
pub struct VolatilityHistory {
    hourly_rv: VecDeque<f64>,  // Last 168 hours
}

impl VolatilityHistory {
    const HISTORY_SIZE: usize = 168;  // 7 days × 24 hours

    pub fn push(&mut self, rv: f64) {
        if self.hourly_rv.len() >= Self::HISTORY_SIZE {
            self.hourly_rv.pop_front();
        }
        self.hourly_rv.push_back(rv);
    }

    /// Calculate percentile rank (0-100) of current RV vs history
    pub fn percentile(&self, current_rv: f64) -> f64 {
        if self.hourly_rv.is_empty() {
            return 50.0;  // Default to middle if no history
        }

        let count_below = self.hourly_rv.iter()
            .filter(|&&x| x < current_rv)
            .count();

        (count_below as f64 / self.hourly_rv.len() as f64) * 100.0
    }

    /// Determine regime from percentile
    pub fn regime(&self, percentile: f64) -> VolRegime {
        match percentile {
            p if p < 20.0 => VolRegime::Low,
            p if p < 80.0 => VolRegime::Normal,
            p if p < 95.0 => VolRegime::High,
            _ => VolRegime::Extreme,
        }
    }
}
```

### A.2 Sigma: Shock Detection

```rust
/// 1-minute return Z-Score for flash crash detection
pub struct ShockDetector {
    /// Rolling 1-hour of 1-minute returns for mean/std calculation
    returns_1m: VecDeque<f64>,
}

impl ShockDetector {
    const LOOKBACK: usize = 60;  // 60 × 1-minute samples = 1 hour

    pub fn push(&mut self, return_1m: f64) {
        if self.returns_1m.len() >= Self::LOOKBACK {
            self.returns_1m.pop_front();
        }
        self.returns_1m.push_back(return_1m);
    }

    /// Calculate Z-Score of latest 1-minute return
    pub fn zscore(&self) -> f64 {
        if self.returns_1m.len() < 10 {
            return 0.0;  // Not enough data
        }

        let mean = self.returns_1m.iter().sum::<f64>() / self.returns_1m.len() as f64;
        let variance = self.returns_1m.iter()
            .map(|x| (x - mean).powi(2))
            .sum::<f64>() / self.returns_1m.len() as f64;
        let std_dev = variance.sqrt();

        if std_dev < 0.0001 {
            return 0.0;  // Avoid division by near-zero
        }

        let latest = self.returns_1m.back().unwrap_or(&0.0);
        (latest - mean) / std_dev
    }

    /// Is there a shock happening? (>3.5σ move)
    pub fn is_shock(&self) -> bool {
        self.zscore().abs() > 3.5
    }
}
```

### A.3 Combined L1 Check

```rust
/// L1 Volatility Filter - BOTH must pass
pub fn l1_vol_check(
    percentile: f64,
    zscore_1m: f64,
) -> L1Result {
    let percentile_ok = percentile < 95.0;
    let shock_ok = zscore_1m.abs() < 3.5;

    match (percentile_ok, shock_ok) {
        (true, true) => L1Result::Pass,
        (false, _) => L1Result::Fail {
            reason: format!("Vol at {}th percentile (extreme regime)", percentile)
        },
        (_, false) => L1Result::Fail {
            reason: format!("{}σ shock detected (flash crash)", zscore_1m)
        },
    }
}
```

### A.4 Thresholds Summary

| Metric | Threshold | Meaning | Action |
|--------|-----------|---------|--------|
| Percentile < 20 | LOW | Quiet market | Trade, tight stops OK |
| Percentile 20-80 | NORMAL | Standard | Trade normally |
| Percentile 80-95 | HIGH | Elevated | CAUTION, widen stops |
| Percentile ≥ 95 | EXTREME | Chaos | **WAIT** |
| Z-Score < 3.5σ | Normal | No shock | Trade normally |
| Z-Score ≥ 3.5σ | SHOCK | Flash crash | **WAIT** |

---

## Appendix B: Gamma Flip Calculation (Dynamic)

The gamma flip price is where net market maker gamma exposure crosses zero.

**Key Requirement:** Must account for IV (Implied Volatility) shifts, not just static OI.

```rust
/// Calculate gamma flip from options chain
/// Must be re-calculated when IV changes significantly
pub fn calculate_gamma_flip(
    spot: f64,
    options_chain: &[OptionContract],
    current_iv: f64,
) -> f64 {
    // Simplified: Find price where net gamma = 0
    // In practice, use Black-Scholes gamma for each strike

    let mut best_flip = spot;
    let mut min_net_gamma = f64::MAX;

    // Search price range around spot
    for test_price in price_range(spot * 0.9, spot * 1.1, 100.0) {
        let net_gamma = options_chain.iter()
            .map(|opt| opt.dealer_gamma_at_price(test_price, current_iv))
            .sum::<f64>();

        if net_gamma.abs() < min_net_gamma {
            min_net_gamma = net_gamma.abs();
            best_flip = test_price;
        }
    }

    best_flip
}

/// Simplified: Use highest Put OI strike as approximate flip
/// (Use this if full calculation is too slow)
pub fn approximate_gamma_flip(options_chain: &[OptionContract]) -> f64 {
    options_chain.iter()
        .filter(|opt| opt.is_put)
        .max_by(|a, b| a.open_interest.partial_cmp(&b.open_interest).unwrap())
        .map(|opt| opt.strike)
        .unwrap_or(0.0)
}
```

**Data Source:** Deribit REST API, refresh every 60 seconds.

---

## Appendix C: Funding Velocity Formula

```rust
/// Track funding rate history for velocity calculation
pub struct FundingTracker {
    history: VecDeque<(i64, f64)>,  // (timestamp, rate)
}

impl FundingTracker {
    const LOOKBACK_MS: i64 = 15 * 60 * 1000;  // 15 minutes

    pub fn push(&mut self, ts: i64, rate: f64) {
        // Remove old entries
        while let Some(&(old_ts, _)) = self.history.front() {
            if ts - old_ts > Self::LOOKBACK_MS {
                self.history.pop_front();
            } else {
                break;
            }
        }
        self.history.push_back((ts, rate));
    }

    /// Calculate velocity: change in funding rate over 15 minutes
    pub fn velocity(&self) -> f64 {
        if self.history.len() < 2 {
            return 0.0;
        }
        let oldest = self.history.front().map(|&(_, r)| r).unwrap_or(0.0);
        let newest = self.history.back().map(|&(_, r)| r).unwrap_or(0.0);
        newest - oldest
    }

    /// Detect if funding is spiking (retail FOMO indicator)
    pub fn is_spiking(&self) -> bool {
        self.velocity().abs() > 0.0002  // 0.02% change in 15m
    }

    /// Detect extreme funding levels
    pub fn is_extreme(&self, current_rate: f64) -> bool {
        current_rate > 0.0005 || current_rate < -0.0002  // >0.05% or <-0.02%
    }
}
```

**Interpretation:**
| Velocity | Meaning | Action |
|----------|---------|--------|
| > +0.02%/15m | Retail FOMOing long | CAUTION: Long squeeze likely |
| < -0.02%/15m | Retail FOMOing short | CAUTION: Short squeeze likely |
| Within ±0.02% | Stable | Normal conditions |

---

## Appendix D: CVD Consensus Logic

```rust
/// Calculate cross-venue CVD consensus
pub fn cvd_consensus(venues: &[VenueFlow]) -> ConsensusResult {
    let mut buy_count = 0;
    let mut sell_count = 0;
    let mut total = 0;

    for venue in venues {
        if venue.is_stale() {
            continue;  // Skip stale data
        }
        total += 1;
        if venue.cvd_5m > 0.0 {
            buy_count += 1;
        } else if venue.cvd_5m < 0.0 {
            sell_count += 1;
        }
    }

    let direction = if buy_count > sell_count {
        Direction::Long
    } else if sell_count > buy_count {
        Direction::Short
    } else {
        Direction::Neutral
    };

    let agreeing = buy_count.max(sell_count);
    let consensus_pct = if total > 0 {
        (agreeing as f64 / total as f64) * 100.0
    } else {
        0.0
    };

    ConsensusResult {
        direction,
        agreeing,
        total,
        consensus_pct,
        // Require 2/3 (66%) agreement for READY
        passed: consensus_pct >= 66.0,
    }
}
```

**Threshold:** Require at least 2/3 venues (66%) to agree on direction before READY state.
