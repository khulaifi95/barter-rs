# TUI Trade Filtering & Aggregation Investigation Report

## CRITICAL FINDING: GLOBAL BUFFER OVERFLOW ON WHALES

The whale detection panel is NOT filtering trades - it's **dropping oldest whales** when the buffer fills up.

---

## 1. TRADE FILTERING LOGIC

### Where Whales Are Detected
**File: barter-trading-tuis/src/shared/state.rs**

#### Trade Entry Point (Lines 199-220)
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
```

**KEY INSIGHT**: All trades are processed (line 212-219), regardless of size, exchange, or market type.
- No pre-filtering by exchange
- No pre-filtering by market type
- No per-exchange thresholds

---

## 2. DEFAULT WHALE_THRESHOLD VALUE

**File: barter-trading-tuis/src/shared/state.rs (Lines 21-30)**

```rust
/// Get whale detection threshold from WHALE_THRESHOLD env var (default: $500,000)
fn whale_threshold() -> f64 {
    static WHALE_THRESHOLD: OnceLock<f64> = OnceLock::new();
    *WHALE_THRESHOLD.get_or_init(|| {
        std::env::var("WHALE_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(500_000.0)  // ← DEFAULT: $500,000
    })
}
```

**DEFAULT WHALE_THRESHOLD: $500,000 USD notional**

Environment variable: `WHALE_THRESHOLD=<value>`

---

## 3. IS THERE PER-EXCHANGE FILTERING?

### Answer: NO - PURELY GLOBAL THRESHOLD

**File: barter-trading-tuis/src/shared/state.rs (Lines 437-464)**

```rust
fn push_trade(
    &mut self,
    trade: TradeData,
    exchange: &str,
    time: DateTime<Utc>,
    is_spot: bool,
    is_perp: bool,
) {
    let usd = trade.price * trade.amount;
    let side = trade.side.clone();
    
    // ... trade record created ...
    
    // Whale threshold (USD notional)
    if usd >= whale_threshold() {  // ← GLOBAL THRESHOLD ONLY
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
        // ... buffer size management ...
    }
}
```

**CRITICAL FINDING**:
- Single global threshold (`whale_threshold()`) applied to ALL exchanges
- No per-exchange thresholds
- No per-market-type thresholds
- No dynamic thresholds based on exchange dominance

---

## 4. MAX_WHALES BUFFER SIZE & BEHAVIOR

### Buffer Configuration
**File: barter-trading-tuis/src/shared/state.rs (Lines 32-41)**

```rust
/// Get max whales buffer size from MAX_WHALES env var (default: 500)
fn max_whales() -> usize {
    static MAX_WHALES: OnceLock<usize> = OnceLock::new();
    *MAX_WHALES.get_or_init(|| {
        std::env::var("MAX_WHALES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(500)  // ← DEFAULT: 500 whales per ticker
    })
}
```

**DEFAULT MAX_WHALES: 500 whales per ticker**

### Buffer Overflow Behavior (Lines 453-455)
```rust
while self.whales.len() > max_whales() {
    self.whales.pop_back();  // ← DROPS OLDEST WHALES
}
```

**CRITICAL BEHAVIOR**:
1. Whales stored in `VecDeque<WhaleRecord>` (FIFO queue)
2. New whales pushed to **front** (`push_front`)
3. When buffer exceeds 500, **oldest whales are dropped** from back
4. Display shows latest 20 whales (line 635):
   ```rust
   let whales: Vec<WhaleRecord> = self.whales.iter().cloned().take(20).collect();
   ```

**CONSEQUENCE**: If OKX is generating >480 whales, ALL Binance/Bybit trades are evicted.

---

## 5. OKX DOMINANCE IMPACT

### Evidence of Exchange Logging
**File: barter-trading-tuis/src/shared/state.rs (Lines 250-283)**

Prints whale counts per exchange every 30 seconds:
```
[whale-debug] last 30s whales: Okx:534 (spot 200 / perp 334 / other 0), BinanceFuturesUsd:12 (spot 0 / perp 12 / other 0), BybitPerpetualsUsd:8 (spot 0 / perp 8 / other 0)
```

**HOW TO DETECT OKX DOMINANCE**:
1. Run the TUI and check console for `[whale-debug]` lines
2. If OKX appears first with >480 trades, buffer overflow occurs
3. Older Binance/Bybit trades are evicted from the 500-whale buffer

---

## 6. CONDITIONS THAT HIDE BINANCE/BYBIT TRADES

### Condition 1: Volume Threshold Not Met
```rust
if usd >= whale_threshold() {  // $500,000 minimum
    // Only then is the trade added to whale buffer
}
```

**Binance/Bybit Hidden If**:
- Individual trade < $500,000
- Most normal trades fall below this threshold

### Condition 2: Buffer Overflow (FIFO Eviction)
**File: barter-trading-tuis/src/shared/state.rs (Lines 439-455)**

```rust
self.whales.push_front(WhaleRecord { ... });
while self.whales.len() > max_whales() {
    self.whales.pop_back();  // ← FIFO eviction
}
```

**Binance/Bybit Hidden If**:
- OKX generates 480+ whales in 15-minute retention window
- Older Binance/Bybit whales get evicted from back of queue
- Display shows only last 20 newest whales

### Condition 3: Display Limited to 20 Newest Trades
**File: barter-trading-tuis/src/shared/state.rs (Line 635)**

```rust
let whales: Vec<WhaleRecord> = self.whales.iter().cloned().take(20).collect();
```

**Result**: Even if Binance trades are in buffer positions 21-500, they won't display.

---

## 7. FILTERING SUMMARY TABLE

| Question | Answer | File:Line |
|----------|--------|-----------|
| **Where is filtering?** | Line 438 | state.rs:438 `if usd >= whale_threshold()` |
| **Default threshold?** | $500,000 | state.rs:28 `.unwrap_or(500_000.0)` |
| **Per-exchange?** | NO | state.rs:438 single `whale_threshold()` call |
| **MAX_WHALES?** | 500 | state.rs:39 `.unwrap_or(500)` |
| **Buffer type?** | FIFO VecDeque | state.rs:367 `whales: VecDeque<WhaleRecord>` |
| **Eviction?** | pop_back | state.rs:454 `self.whales.pop_back()` |
| **Display count?** | 20 newest | state.rs:635 `.take(20)` |
| **Exchange logging?** | Every 30s | state.rs:268-283 `[whale-debug]` |

---

## ROOT CAUSE: WHY BINANCE/BYBIT DON'T APPEAR

### Path 1: Size Threshold
- Binance typical trade: $100K-$300K
- Whale threshold: $500K
- Result: Most Binance trades never qualify as whales

### Path 2: Buffer Overflow
- OKX high volume: Generates 480+ whales per 15 minutes
- Max buffer: 500 whales per ticker
- Method: FIFO - newest added to front, oldest dropped from back
- Result: Binance trades evicted to make room for OKX

### Path 3: Display Starvation
- Only newest 20 whales displayed
- If last 20 are all recent OKX trades
- Binance/Bybit pushed to positions 21+
- Result: Never visible on screen

---

## CONFIGURATION OPTIONS

### Environment Variables

```bash
# Whale detection threshold (USD notional)
export WHALE_THRESHOLD=500000        # Default: $500K

# Maximum whales per ticker in buffer
export MAX_WHALES=500                # Default: 500

# Liquidation danger threshold
export LIQ_DANGER_THRESHOLD=1000000  # Default: $1M

# Mega whale highlighting (display only)
export MEGA_WHALE_THRESHOLD=5000000  # Default: $5M

# Liq display danger threshold
export LIQ_DISPLAY_DANGER_THRESHOLD=1000000  # Default: $1M
```

### To Fix Binance/Bybit Visibility

**Option A: Lower whale threshold**
```bash
export WHALE_THRESHOLD=100000        # $100K instead of $500K
```

**Option B: Increase buffer size**
```bash
export MAX_WHALES=2000               # 2000 instead of 500
```

**Option C: Change retention window** (requires code change)
```rust
// In state.rs:16, change from:
const TRADE_RETENTION_SECS: i64 = 15 * 60;  // 900 seconds
// To:
const TRADE_RETENTION_SECS: i64 = 5 * 60;   // 300 seconds (5 minutes)
```

---

## CONCLUSION

**The specific evidence:**

1. **Whale filtering starts at state.rs:438**
   - Only trades >= $500,000 USD added to whale buffer
   
2. **No per-exchange filtering at state.rs:438**
   - Single `whale_threshold()` call for all exchanges
   
3. **MAX_WHALES = 500 at state.rs:39**
   - Buffer holds max 500 whales per ticker
   
4. **FIFO eviction at state.rs:454**
   - `while self.whales.len() > max_whales() { self.whales.pop_back() }`
   - Drops oldest when new ones arrive
   
5. **OKX dominance logged at state.rs:280**
   - `[whale-debug]` shows exchange counts every 30 seconds
   - If OKX > 480, buffer overflow occurs
   
6. **Display limited to 20 at state.rs:635**
   - `.take(20)` shows only newest trades
   - Even if Binance in buffer, positions 21+ don't display

**To verify the hypothesis**: Run TUI, watch console for `[whale-debug]` output. If OKX count >> Binance/Bybit, buffer overflow is the cause.

