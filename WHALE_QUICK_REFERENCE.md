# Whale Detection Quick Reference

## The Problem
Trades from Binance and Bybit don't appear in the whale detector panel, even though OI and Liquidations work fine.

## Root Cause (In Order of Impact)

### 1. FIFO Buffer Overflow (PRIMARY)
- **Buffer size**: 500 whales max per ticker (configurable: `MAX_WHALES`)
- **Eviction**: Oldest whales dropped when buffer full
- **OKX dominance**: If OKX generates >480 whales in 15 min, Binance/Bybit evicted
- **Evidence**: `state.rs:453-455` `while self.whales.len() > max_whales() { self.whales.pop_back() }`

### 2. Size Threshold (SECONDARY)
- **Threshold**: $500,000 USD notional (configurable: `WHALE_THRESHOLD`)
- **Binance avg**: $100K-$300K per trade
- **Result**: Most Binance trades below threshold, never enter buffer
- **Evidence**: `state.rs:438` `if usd >= whale_threshold()`

### 3. Display Limit (TERTIARY)
- **Display limit**: Only newest 20 whales shown
- **Storage**: Up to 500 kept in buffer
- **Result**: Even if Binance in positions 21-500, not visible
- **Evidence**: `state.rs:635` `.take(20)`

## Configuration

### Environment Variables
```bash
WHALE_THRESHOLD=500000          # Min USD size to trigger whale detection
MAX_WHALES=500                  # Buffer size per ticker
LIQ_DANGER_THRESHOLD=1000000    # Liq cascade risk threshold
MEGA_WHALE_THRESHOLD=5000000    # Highlight threshold (display only)
```

### Quick Fixes

#### Fix #1: Lower Threshold (For Dev/Testing)
```bash
export WHALE_THRESHOLD=100000    # Catch $100K+ trades instead of $500K+
```

#### Fix #2: Increase Buffer
```bash
export MAX_WHALES=2000          # Allow 2000 whales instead of 500
```

#### Fix #3: Shorter Retention (Code Change Required)
In `state.rs:16`:
```rust
const TRADE_RETENTION_SECS: i64 = 5 * 60;  // 5 min instead of 15 min
```

## Detection Steps

### 1. Enable Debug Output
```bash
# Just run the TUI, console automatically prints every 30 seconds
cargo run --bin market_microstructure
```

### 2. Watch for `[whale-debug]` Output
```
[whale-debug] last 30s whales: Okx:534 (spot 200 / perp 334 / other 0), BinanceFuturesUsd:12 (spot 0 / perp 12 / other 0), BybitPerpetualsUsd:8 (spot 0 / perp 8 / other 0)
```

### 3. Interpret Results
- If `Okx: >480` = buffer overflow (eviction happening)
- If `Okx: >>BinanceFuturesUsd` = OKX dominance confirmed
- If all counts low = threshold too high

## Code Evidence Summary

| Finding | File | Line | Code |
|---------|------|------|------|
| **Where filtering?** | state.rs | 438 | `if usd >= whale_threshold()` |
| **Default threshold?** | state.rs | 28 | `.unwrap_or(500_000.0)` |
| **Per-exchange?** | state.rs | 438 | NO - single `whale_threshold()` |
| **Max buffer size?** | state.rs | 39 | `.unwrap_or(500)` |
| **Buffer type?** | state.rs | 367 | `VecDeque<WhaleRecord>` |
| **Eviction method?** | state.rs | 454 | `self.whales.pop_back()` |
| **New entry?** | state.rs | 439 | `self.whales.push_front()` |
| **Display rows?** | state.rs | 635 | `.take(20)` |
| **Debug interval?** | state.rs | 268 | `>= 30 seconds` |
| **Debug output?** | state.rs | 280 | `println!("[whale-debug] ...")` |

## Data Flow

```
All Trades From WebSocket
    ↓
process_event() [state.rs:199]
    ↓
push_trade() [state.rs:409]
    ↓
USD = price * amount
    ↓
if usd >= $500,000 [state.rs:438] ← SIZE FILTER
    ↓
    push_front(WhaleRecord) [state.rs:439]
    ↓
    while len() > 500 [state.rs:453] ← FIFO EVICTION
        pop_back()
    ↓
to_snapshot() [state.rs:629]
    ↓
.take(20) [state.rs:635] ← DISPLAY LIMIT
    ↓
Render whale_panel() [market_microstructure.rs:399]
```

## Per-Ticker Isolation

Each ticker has its OWN whale buffer:
- BTC: up to 500 whales
- ETH: up to 500 whales
- SOL: up to 500 whales

So if BTC is saturated with OKX, ETH/SOL not affected.

## No Per-Exchange Filtering

Single global threshold = all exchanges treated equally.

Example:
- OKX $100K trade = whale (passes)
- Binance $100K trade = whale (passes)
- But if buffer full, one gets evicted

No exchange-specific thresholds exist.

## Retention Windows

```rust
const TRADE_RETENTION_SECS: i64 = 15 * 60;      // 900 sec = 15 min
const LIQ_RETENTION_SECS: i64 = 10 * 60;        // 600 sec = 10 min
const CVD_RETENTION_SECS: i64 = 5 * 60;         // 300 sec = 5 min
const PRICE_RETENTION_SECS: i64 = 15 * 60;      // 900 sec = 15 min
```

Whales outside 15-min window are pruned (plus FIFO eviction).

## OI & Liquidations Work Fine Because

1. **Liquidations**: No buffer limit, no size filter - all retained 10 min
2. **OI**: No buffer limit - all retained permanently per exchange
3. **Whales**: Only whales have buffer + size filtering

That's why whale panel empty but liq/OI panels populated.
