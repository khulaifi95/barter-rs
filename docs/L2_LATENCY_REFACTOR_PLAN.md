# L2 Latency Refactor Plan

## Problem Statement

The barter-data-server currently experiences **12-18 second latency** on trade events due to high-frequency L2 orderbook data creating backpressure in a single broadcast channel.

### Root Cause

1. **Single broadcast channel** carries all data types (L2, trades, liquidations, OI)
2. **L2 volume**: ~200 messages/sec across 3 exchanges × 3 symbols
3. **Hot-path trade logging** (`info!`) blocks the event loop
4. **Unbounded internal queues** allow memory growth without backpressure signals

### Evidence

Latency measurements from TUI client showed:
```
[LATENCY] BTC Binance trades: avg=12832ms min=10981ms max=17491ms
[LATENCY] BTC Binance trades: avg=18738ms min=17975ms max=19727ms
```

After initial fixes (debug logging + buffer increase):
```
[LATENCY] BTC Binance trades: avg=4ms min=-49ms max=274ms   # Good
[LATENCY] BTC Binance trades: avg=2025ms min=509ms max=3025ms  # L2 bursts
```

---

## Architecture Overview

### Current Data Flow

```
Exchange WebSockets (Binance/Bybit/OKX)
         │
         ▼
DynamicStreams (unbounded mpsc channels per type)
         │
         ▼
Main Event Loop (single-threaded, lines 133-285)
    │
    ├── JSON serialization (per event)
    ├── Type detection (5+ pattern matches)
    ├── Logging (info! for trades - FIXED to debug!)
    │
    ▼
Broadcast Channel (100K buffer - INCREASED from 10K)
         │
         ▼
Per-Client Tasks (JSON serialize again, WebSocket send)
```

### Exchange L2 Specifications

| Exchange | Update Freq | Depth Levels | Traffic/Symbol |
|----------|-------------|--------------|----------------|
| OKX | 100ms | 400 | ~10 KB/sec |
| Binance | 100ms | 1000 (delta) | ~8 KB/sec |
| Bybit | 200ms | 200 | ~4 KB/sec |

**Total L2 for 9 streams**: ~66 KB/sec = ~200 messages/sec

### What TUI Actually Needs

The TUI extracts **one value** from each L2 update:
```rust
bid_imbalance_pct: f64  // 0-100%, where 50% = balanced
```

Client-side throttles display to **1500ms** anyway (`L2_INTERVAL_MS`).

---

## Refactor Phases

### Phase 1: Server-Side L2 Throttling (RECOMMENDED FIRST)

**Effort**: 2-3 hours | **Risk**: Low | **Impact**: 80% latency reduction

#### Changes Required

**File: `barter-data-server/src/main.rs`**

1. **Add L2 throttling state** (after line 104):

```rust
use std::sync::Mutex;
use once_cell::sync::Lazy;

// L2 throttling: only broadcast every 100ms per instrument
static L2_LAST_BROADCAST: Lazy<Mutex<HashMap<String, Instant>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
const L2_THROTTLE_MS: u64 = 100;
```

2. **Add throttle check in main loop** (around line 192, before L2 broadcast):

```rust
if is_orderbook_l2 {
    let key = format!("{}:{}:{}",
        message.exchange,
        message.instrument.base,
        message.instrument.quote);

    let mut last = L2_LAST_BROADCAST.lock().unwrap();
    let now = Instant::now();

    if let Some(prev) = last.get(&key) {
        if now.duration_since(*prev) < Duration::from_millis(L2_THROTTLE_MS) {
            continue; // Skip this L2 update, keep latest state
        }
    }
    last.insert(key, now);
    // Fall through to broadcast
}
```

3. **Add latency telemetry** (in main loop, for trades):

```rust
if is_trade {
    let exchange_time = market_event.time_exchange;
    let receive_time = market_event.time_received;
    let send_time = Utc::now();

    let exchange_to_receive_ms = (receive_time - exchange_time).num_milliseconds();
    let receive_to_send_ms = (send_time - receive_time).num_milliseconds();

    debug!(
        "LATENCY {} {}: exch→recv={}ms recv→send={}ms",
        message.exchange,
        message.instrument.base,
        exchange_to_receive_ms,
        receive_to_send_ms
    );
}
```

#### Expected Outcome

- L2 broadcasts reduced from ~200/sec to ~90/sec (10 per instrument per second)
- Trade latency: <100ms p50, <500ms p99
- No TUI changes required

---

### Phase 2: Separate L2 Channel (IF NEEDED)

**Effort**: 4-6 hours | **Risk**: Medium | **Impact**: Complete isolation

Only proceed if Phase 1 doesn't achieve <100ms trade latency.

#### Changes Required

**File: `barter-data-server/src/main.rs`**

1. **Create two broadcast channels**:

```rust
// Events channel (trades, liquidations, OI, L1) - guaranteed delivery
let (tx_events, _) = broadcast::channel::<MarketEventMessage>(50_000);

// L2 channel (orderbooks) - best effort, can drop
let (tx_l2, _) = broadcast::channel::<MarketEventMessage>(10_000);
```

2. **Route events by type in main loop**:

```rust
let target_tx = if is_orderbook_l2 {
    &tx_l2
} else {
    &tx_events
};

match target_tx.send(message) { ... }
```

3. **Update client handler to subscribe to both**:

```rust
let mut rx_events = tx_events.subscribe();
let mut rx_l2 = tx_l2.subscribe();

// Use tokio::select! to read from both
loop {
    tokio::select! {
        Ok(event) = rx_events.recv() => {
            // Send immediately (critical path)
            ws_sender.send(event).await?;
        }
        Ok(l2) = rx_l2.recv() => {
            // Can be throttled or dropped
            ws_sender.send(l2).await?;
        }
    }
}
```

**File: `barter-trading-tuis/src/shared/websocket.rs`** (optional)

If using separate ports, add second WebSocket connection.

---

### Phase 3: Pre-Aggregated L2 (OPTIONAL)

**Effort**: 3-4 hours | **Impact**: 90% bandwidth reduction

Send pre-calculated imbalance instead of full orderbook.

#### Server Changes

1. **Maintain orderbook state on server**:

```rust
struct L2ServerState {
    books: HashMap<(String, String), OrderBook>,  // (exchange, instrument)
}

impl L2ServerState {
    fn update_and_get_imbalance(&mut self, event: OrderBookEvent) -> f64 {
        // Update internal book
        // Calculate and return bid_imbalance_pct
    }
}
```

2. **Send minimal message**:

```rust
#[derive(Serialize)]
struct L2ImbalanceMessage {
    exchange: String,
    instrument: String,
    bid_imbalance_pct: f64,
    timestamp_ms: i64,
}
```

#### Client Changes

**File: `barter-trading-tuis/src/shared/state.rs`**

Add handler for new message type:

```rust
"l2_imbalance" => {
    if let Ok(imb) = serde_json::from_value::<L2ImbalanceMessage>(event.data) {
        state.store_book_imbalance_direct(&imb.exchange, imb.bid_imbalance_pct);
    }
}
```

---

## Files to Modify

| Phase | File | Changes |
|-------|------|---------|
| 1 | `barter-data-server/src/main.rs` | Add L2 throttling + telemetry |
| 2 | `barter-data-server/src/main.rs` | Split broadcast channels |
| 2 | `barter-trading-tuis/src/shared/websocket.rs` | (Optional) dual connection |
| 3 | `barter-data-server/src/main.rs` | L2 state + imbalance calc |
| 3 | `barter-trading-tuis/src/shared/state.rs` | Handle imbalance message |
| 3 | `barter-trading-tuis/src/shared/types.rs` | Add L2ImbalanceMessage type |

---

## Testing & Validation

### Latency Measurement

The TUI already has latency logging (added during investigation):

**File: `barter-trading-tuis/src/bin/scalper_v2.rs`** (lines 349-374)

```rust
// Logs every 5 seconds:
// [LATENCY] BTC Binance trades: avg=XXms min=XXms max=XXms samples=XXX
```

### Success Criteria

| Metric | Current | Target (Phase 1) | Target (Phase 2) |
|--------|---------|------------------|------------------|
| Trade latency p50 | 12-18s | <100ms | <50ms |
| Trade latency p99 | 20s+ | <500ms | <200ms |
| L2 latency | 12-18s | <300ms | <200ms |

### Test Commands

```bash
# Start server with warn logging (suppresses debug)
RUST_LOG=warn ./target/release/barter-data-server

# Monitor TUI latency
./target/release/scalper-v2 2>&1 | grep LATENCY

# Compare to ground truth
watch -n 1 'curl -s "https://fapi.binance.com/fapi/v1/ticker/price?symbol=BTCUSDT" | jq -r .price'
```

---

## Already Completed Fixes

These changes have been applied and should NOT be reverted:

1. **Trade logging changed from `info!` to `debug!`**
   - File: `barter-data-server/src/main.rs` line 216
   - Removes synchronous I/O from hot path

2. **Buffer size increased from 10K to 100K**
   - File: `barter-data-server/src/main.rs` line 101
   - Reduces broadcast channel overflow

---

## Rollback Plan

If issues arise:

1. **Phase 1**: Remove L2 throttling code, restore original main loop
2. **Phase 2**: Revert to single broadcast channel
3. **Phase 3**: Keep full orderbook messages, remove imbalance shortcut

All changes are additive and can be feature-flagged via environment variables.

---

## References

- Current server code: `barter-data-server/src/main.rs`
- TUI state processing: `barter-trading-tuis/src/shared/state.rs`
- WebSocket client: `barter-trading-tuis/src/shared/websocket.rs`
- L2 display throttling: `barter-trading-tuis/src/bin/scalper_v2.rs` (L2_INTERVAL_MS = 1500)

---

## Handoff Checklist

- [ ] Read this document fully
- [ ] Review current `barter-data-server/src/main.rs`
- [ ] Implement Phase 1 changes
- [ ] Test with `RUST_LOG=warn`
- [ ] Verify latency via TUI logs
- [ ] If <100ms achieved, stop; otherwise proceed to Phase 2
