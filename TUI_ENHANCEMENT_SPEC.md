# TUI Enhancement Specification

## Current Iteration - TUI1 Intraday Refactor

### Panel 1: EXCHANGE INTELLIGENCE (Replaces BASIS MOMENTUM)

```
┌─ EXCHANGE INTELLIGENCE ──────────────┐
│ OI (5m/15m Δ)              HEALTH   │
│  BNC 42%: +2.1M/+8.5M ↑    ● 0.2s  │
│  OKX 38%: +1.8M/+6.2M ↑    ● 0.1s  │
│  BBT 20%: -0.3M/-1.1M ↓    ● 0.3s  │
│                                      │
│ CVD (5m/15m)                         │
│  BNC: -1.5M/-12M ↓                  │
│  OKX: +45M/+180M ↑  ← LEADER        │
│  BBT: -2M/-8M ↓                     │
│                                      │
│ ⚡ OKX leading: +180M vs BNC -12M   │
└──────────────────────────────────────┘
```

**Features:**
- Per-exchange OI share percentage
- 5m AND 15m OI delta per exchange
- 5m AND 15m CVD per exchange
- Health/latency indicator (data freshness)
- Leader insight line (which exchange is driving flow)

**Colors:**
- Green: Positive delta / expanding
- Red: Negative delta / contracting
- Gray: Neutral / flat
- Health dot: Green (< 1s), Yellow (1-5s), Red (> 5s stale)

---

### Panel 2: MARKET PULSE (Enhanced)

```
┌─ MARKET PULSE ───────────────────────┐
│ BTC $88.0K   95t/s   ● ALL LIVE     │
│  Vol: 101M(30s) 295M(1m) 2B(5m)     │
│  OI:  +5K (BNC+3K OKX+1.5K BBT+0.5K)│
│  Vel: +18/s EXPANDING               │
│                                      │
│ ETH $2.9K    98t/s                   │
│  Vol: 12M(30s) 23M(1m) 152M(5m)     │
│  OI:  +3K (BNC+2K OKX+0.8K BBT+0.2K)│
│  Vel: +12/s EXPANDING               │
│                                      │
│ SOL $137.78  41t/s                   │
│  Vol: 388K(30s) 760K(1m) 5M(5m)     │
│  OI:  +2K (BNC+1K OKX+0.5K BBT+0.5K)│
│  Vel: flat NEUTRAL                   │
└──────────────────────────────────────┘
```

**Changes from current:**
- Remove `Exch:OKX 98%` (moved to Exchange Intelligence)
- Add per-exchange OI breakdown inline
- Add health indicator `● ALL LIVE`
- Add OI velocity label (EXPANDING/CONTRACTING/NEUTRAL)

---

### Panel 3: CVD DIVERGENCE (Enhanced)

```
┌─ CVD DIVERGENCE ─────────────────────┐
│ BTC:                                 │
│  1m:  -243M  ↓  ACCEL │ DISTRIBUTION │
│  5m:  -311M  ↓  STEADY│ BEARISH      │
│  15m: -890M  ↓        │ Vel: -59M/min│
│                                      │
│ ETH:                                 │
│  1m:  +8.3M  ↑  DECEL │ ⚠️ TURNING   │
│  5m:  -11.6M ↓  STEADY│ NEUTRAL      │
│  15m: -42M   ↓        │ Vel: -2.8M/min│
│                                      │
│ SOL:                                 │
│  1m:  +1.9M  ↑  STEADY│ ACCUMULATION │
│  5m:  -2.8M  ↓  STEADY│ NEUTRAL      │
│  15m: +5.2M  ↑        │ Vel: +347K/min│
└──────────────────────────────────────┘
```

**Changes from current:**
- Add 15m timeframe (was only 1m/5m)
- Add velocity labels (ACCEL/DECEL/STEADY)
- Add flow signals (ACCUMULATION/DISTRIBUTION/EXHAUSTION/TURNING)
- Use 30s data internally for early signal detection (not displayed)
- Remove Binance (5m) section (moved to Exchange Intelligence)

**Signal Logic:**
| Signal | Condition | Color |
|--------|-----------|-------|
| ACCUMULATION | Price flat, CVD rising | Green |
| DISTRIBUTION | Price up, CVD falling | Red |
| EXHAUSTION | Price moving, CVD flat | Yellow |
| TURNING | 30s opposing 1m direction | Yellow |
| BULLISH | Price up, CVD up | Green |
| BEARISH | Price down, CVD down | Red |
| NEUTRAL | No clear signal | Gray |

**30s Hidden Signal Logic:**
- If 30s CVD opposes 1m direction AND magnitude > threshold → Show ⚠️ TURNING
- If 30s CVD confirms 1m direction AND magnitude > threshold → Strengthen color
- Otherwise → No early indicator

---

## Future Enhancements (Lower Priority)

### Volatility & ATR (Can calculate from existing data)

```
┌─ VOLATILITY ─────────────────────────┐
│ BTC: 0.8% (1m) 2.1% (5m) LOW        │
│      Range: $87,800 - $88,200       │
│      ATR(14): $320                   │
│                                      │
│ ETH: 1.2% (1m) 3.4% (5m) NORMAL     │
│      Range: $2,880 - $2,940         │
│      ATR(14): $45                    │
└──────────────────────────────────────┘
```

**Implementation:** Calculate from `price_history` - no new subscription needed.

**Use case:** Position sizing, stop distance calculation.

---

### VWAP Deviation (Already have data)

```
┌─ VWAP ───────────────────────────────┐
│ BTC: $88,050 (1m)  Price: $88,120   │
│      Status: +0.08% ABOVE VWAP      │
│                                      │
│ ETH: $2,915 (1m)   Price: $2,908    │
│      Status: -0.24% BELOW VWAP      │
└──────────────────────────────────────┘
```

**Implementation:** Already calculated (`vwap_1m`, `vwap_5m`), just need display.

**Use case:** Mean reversion entries, fair value reference.

---

### Order Book Depth L2 (Would need new subscription)

```
┌─ BOOK DEPTH ─────────────────────────┐
│ BTC:                                 │
│  Bid Wall: $87,900 ($5M depth)      │
│  Ask Wall: $88,500 ($3M depth)      │
│  Imbalance: 62% BID                 │
│                                      │
│ Visible Liquidity (±0.5%):          │
│  Bids: $12M  │  Asks: $8M           │
└──────────────────────────────────────┘
```

**Implementation:** Requires L2/L3 orderbook subscription (not currently subscribed).

**Use case:** Support/resistance levels, liquidity analysis.

**Priority:** LOW - significant implementation effort.

---

### Funding Rate (Would need new subscription)

```
┌─ FUNDING ────────────────────────────┐
│ BTC: +0.0100% (8h)  Longs pay       │
│ ETH: +0.0085% (8h)  Longs pay       │
│ SOL: -0.0050% (8h)  Shorts pay      │
│                                      │
│ Next funding in: 2h 34m             │
└──────────────────────────────────────┘
```

**Implementation:** Requires funding rate subscription.

**Use case:** Sentiment indicator, position cost calculation.

**Priority:** LOW for intraday/scalping (don't hold through funding).

---

## Scalper TUI Revamp (Next Iteration)

### Current Issues Identified:
1. Only OKX whales showing (Binance/Bybit trades too small individually)
2. Missing velocity acceleration indicators
3. No VWAP deviation display
4. No volatility/range indicator
5. Empty space in layout could show more data

### Proposed Enhancements:
1. Add trade aggregation for Binance/Bybit (combine fills within same ms/price)
2. Add VWAP deviation panel
3. Add volatility indicator
4. Enhance delta velocity with acceleration curves
5. Consider lowering whale threshold for scalper context

### Timeframe Alignment:
- Scalper: 5s / 15s / 30s (sub-minute focus)
- TUI1 Intraday: 1m / 5m / 15m (minute+ focus)

---

## Data Availability Summary

| Data | Have? | Source | Notes |
|------|-------|--------|-------|
| Trades | ✅ | WebSocket | All exchanges |
| OrderBook L1 | ✅ | WebSocket | Best bid/ask only |
| OrderBook L2/L3 | ❌ | - | Not subscribed |
| Liquidations | ✅ | WebSocket | All exchanges |
| Open Interest | ✅ | WebSocket + REST | Perps only |
| Funding Rate | ❌ | - | Not subscribed |
| VWAP | ✅ | Calculated | From trades |
| Volatility | ⚠️ | Calculate | Need to add |
| ATR | ⚠️ | Calculate | Need to add |
| CVD | ✅ | Calculated | From trades |

---

## Implementation Order

### Phase 1 (Current)
1. ✅ Document specifications
2. Add 15m CVD calculation to shared state
3. Add per-exchange OI tracking to shared state
4. Implement EXCHANGE INTELLIGENCE panel
5. Implement enhanced MARKET PULSE panel
6. Implement enhanced CVD DIVERGENCE panel

### Phase 2 (Future)
1. Add volatility/ATR calculations
2. Add VWAP deviation display
3. Revamp Scalper TUI
4. Consider trade aggregation for Binance/Bybit whale detection

### Phase 3 (If needed)
1. L2 orderbook subscription
2. Funding rate subscription
