# L2 Separation Implementation Plan

## Objective
Achieve **10-20ms end-to-end latency** for trades by completely isolating L2 orderbook processing from the trade hot path.

---

## Current Problem

### Architecture (Single Event Loop)
```
┌─────────────────────────────────────────────────────────────┐
│                    SINGLE EVENT LOOP                        │
│  while let Some(event) = combined_stream.next().await       │
│                                                             │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐         │
│  │ L2 Events   │  │   Trades    │  │ Liqs/OI/CVD │         │
│  │ (900/sec)   │  │  (100/sec)  │  │  (10/sec)   │         │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘         │
│         │                │                │                 │
│         ▼                ▼                ▼                 │
│  ┌──────────────────────────────────────────────┐          │
│  │     L2 THROTTLE MUTEX (async lock)           │ ◄── BOTTLENECK
│  │     l2_last_broadcast.lock().await           │          │
│  └──────────────────────────────────────────────┘          │
│                          │                                  │
│                          ▼                                  │
│  ┌──────────────────────────────────────────────┐          │
│  │    SINGLE BROADCAST CHANNEL (100k buffer)    │          │
│  │    tx.send(message) for ALL event types      │          │
│  └──────────────────────────────────────────────┘          │
└─────────────────────────────────────────────────────────────┘
```

### Problem
Even with `tokio::sync::Mutex`, the single event loop processes events sequentially. A burst of L2 events delays trade processing because:
1. L2 mutex lock is acquired on every L2 event (contention point)
2. All event types compete for the same broadcast channel buffer
3. Trades must wait behind L2 bursts in the event loop

### Current Latency Evidence
- Processing latency (recv→send): **0.4ms p50** (good)
- But under L2 bursts: **p95 can spike to 60-150ms**
- Target: **p99 < 20ms**

---

## Proposed Architecture

### Split Event Streams
```
┌─────────────────────────────────────────────────────────────┐
│                    SPLIT EVENT STREAMS                      │
│                                                             │
│  ┌──────────────────────┐    ┌──────────────────────┐      │
│  │  L2 TASK (spawned)   │    │  MAIN LOOP (trades)  │      │
│  │  tokio::spawn        │    │  while let Some...   │      │
│  │                      │    │                      │      │
│  │  L2 throttle logic   │    │  NO L2 processing    │      │
│  │  Per-exchange rates  │    │  NO mutex contention │      │
│  │  tx_l2.send()        │    │  tx_trades.send()    │      │
│  └──────────┬───────────┘    └──────────┬───────────┘      │
│             │                           │                   │
│             ▼                           ▼                   │
│  ┌────────────────────┐    ┌────────────────────┐          │
│  │ L2 CHANNEL (50k)   │    │ TRADES CHANNEL(10k)│          │
│  │ Lower priority     │    │ High priority      │          │
│  └────────────────────┘    └────────────────────┘          │
│                                                             │
│             └──────────────┬───────────────┘               │
│                            ▼                                │
│  ┌──────────────────────────────────────────────┐          │
│  │  CLIENT HANDLER (tokio::select!)             │          │
│  │  Prioritize trades, merge L2 when available  │          │
│  │  Same port: ws://0.0.0.0:9001                │          │
│  └──────────────────────────────────────────────┘          │
└─────────────────────────────────────────────────────────────┘
```

### Key Design Decisions
1. **Single port (9001)** - No client/TUI changes required
2. **Two internal broadcast channels** - Trades vs L2
3. **Per-exchange L2 throttle** - OKX 150-200ms, Binance/Bybit 100ms
4. **Trades prioritized** in client handler via `tokio::select!` with `biased`

---

## Files to Modify

### Server (1 file)
| File | Lines | Changes |
|------|-------|---------|
| `barter-data-server/src/main.rs` | 759 | ~80 lines modified/added |

### Clients (0 files)
No TUI changes required - they receive all events on same port, filter by `kind` field.

---

## Detailed Implementation Steps

### Step 1: Update Constants (Line ~27-31)

**Current:**
```rust
const L2_THROTTLE_MS: u64 = 100;
```

**Change to:**
```rust
// L2 throttling per exchange (OKX is noisier, needs higher throttle)
const L2_THROTTLE_BINANCE_MS: u64 = 100;
const L2_THROTTLE_BYBIT_MS: u64 = 100;
const L2_THROTTLE_OKX_MS: u64 = 150; // OKX sends more L2 data
```

### Step 2: Add Helper Function (After constants, ~Line 32)

**Add:**
```rust
/// Get L2 throttle interval for a given exchange
fn get_l2_throttle_ms(exchange: &str) -> u64 {
    if exchange.contains("Okx") {
        std::env::var("L2_THROTTLE_OKX_MS")
            .ok().and_then(|s| s.parse().ok())
            .unwrap_or(L2_THROTTLE_OKX_MS)
    } else if exchange.contains("Bybit") {
        std::env::var("L2_THROTTLE_BYBIT_MS")
            .ok().and_then(|s| s.parse().ok())
            .unwrap_or(L2_THROTTLE_BYBIT_MS)
    } else {
        std::env::var("L2_THROTTLE_BINANCE_MS")
            .ok().and_then(|s| s.parse().ok())
            .unwrap_or(L2_THROTTLE_BINANCE_MS)
    }
}
```

### Step 3: Create Two Broadcast Channels (Replace lines ~100-110)

**Current:**
```rust
let buffer_size = std::env::var("WS_BUFFER_SIZE")
    .ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(100_000);

info!("WebSocket broadcast buffer size: {}", buffer_size);
let (tx, _rx) = broadcast::channel::<MarketEventMessage>(buffer_size);
let tx = Arc::new(tx);
```

**Change to:**
```rust
// Separate channels for trades (hot path) and L2 (high volume, lower priority)
let trades_buffer = std::env::var("WS_TRADES_BUFFER")
    .ok().and_then(|s| s.parse().ok())
    .unwrap_or(10_000);
let l2_buffer = std::env::var("WS_L2_BUFFER")
    .ok().and_then(|s| s.parse().ok())
    .unwrap_or(50_000);

info!("Trade channel buffer: {}, L2 channel buffer: {}", trades_buffer, l2_buffer);

// Trades channel: trades, liquidations, OI, CVD, L1 (hot path - NO L2)
let (tx_trades, _) = broadcast::channel::<MarketEventMessage>(trades_buffer);
let tx_trades = Arc::new(tx_trades);

// L2 channel: orderbook L2 only (high volume, can lag without affecting trades)
let (tx_l2, _) = broadcast::channel::<MarketEventMessage>(l2_buffer);
let tx_l2 = Arc::new(tx_l2);
```

### Step 4: Update WebSocket Server Spawn (Replace lines ~118-121)

**Current:**
```rust
let tx_clone = tx.clone();
tokio::spawn(async move {
    start_websocket_server(server_addr, tx_clone).await;
});
```

**Change to:**
```rust
let tx_trades_clone = tx_trades.clone();
let tx_l2_clone = tx_l2.clone();
tokio::spawn(async move {
    start_websocket_server(server_addr, tx_trades_clone, tx_l2_clone).await;
});
```

### Step 5: Spawn Dedicated L2 Processor Task (After line ~135, before main loop)

**Add NEW section:**
```rust
// Spawn dedicated L2 processing task (isolates L2 from trade hot path)
let tx_l2_processor = tx_l2.clone();
tokio::spawn(async move {
    // L2 throttling state (per-instrument, per-exchange)
    let mut l2_last_broadcast: HashMap<String, Instant> = HashMap::new();
    let mut rx_l2_internal = tx_l2_processor.subscribe();

    loop {
        match rx_l2_internal.recv().await {
            Ok(event) => {
                // Apply per-exchange throttling
                let key = format!("{}:{}:{}",
                    event.exchange,
                    event.instrument.base,
                    event.instrument.quote
                );
                let throttle_ms = get_l2_throttle_ms(&event.exchange);
                let now = Instant::now();

                if let Some(prev) = l2_last_broadcast.get(&key) {
                    if now.duration_since(*prev) < Duration::from_millis(throttle_ms) {
                        continue; // Skip throttled L2
                    }
                }
                l2_last_broadcast.insert(key, now);

                // L2 is already in the channel, throttled events are just skipped
                // The channel itself handles delivery to clients
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                debug!("L2 processor lagged {} messages (expected under load)", n);
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
});
info!("L2 processor task spawned (isolated from trade path)");
```

**Wait - actually simpler approach:** Don't use internal subscription. Instead, throttle BEFORE sending to L2 channel. Let me revise:

### Step 5 (REVISED): Remove L2 from Main Loop, Send to Separate Channel

**Current main loop L2 handling (lines ~228-265):**
```rust
// Log L2 orderbook events at debug level (very high frequency)
if is_orderbook_l2 {
    debug!(...)
    // Throttle L2 broadcasts to max 1 per 100ms per instrument
    let key = format!(...);
    let now = Instant::now();
    let should_skip = {
        let mut last = l2_last_broadcast.lock().await;  // <-- MUTEX HERE
        ...
    };
    if should_skip {
        continue;
    }
}
```

**Change to (remove mutex from hot path, route L2 to separate channel):**
```rust
// L2 orderbook events go to dedicated channel (NOT the trade hot path)
if is_orderbook_l2 {
    debug!(
        "L2_BOOK {} {}/{}",
        market_event.exchange,
        market_event.instrument.base,
        market_event.instrument.quote
    );

    // Apply per-exchange throttling
    let key = format!(
        "{}:{}:{}",
        market_event.exchange,
        market_event.instrument.base,
        market_event.instrument.quote
    );
    let throttle_ms = get_l2_throttle_ms(&format!("{:?}", market_event.exchange));

    let now = Instant::now();
    let should_skip = {
        let mut last = l2_last_broadcast.lock().await;
        if let Some(prev) = last.get(&key) {
            if now.duration_since(*prev) < Duration::from_millis(throttle_ms) {
                true
            } else {
                last.insert(key, now);
                false
            }
        } else {
            last.insert(key, now);
            false
        }
    };

    if should_skip {
        continue; // Skip throttled L2
    }

    // Send to L2 channel (separate from trades)
    let message = MarketEventMessage::from(market_event);
    let _ = tx_l2.send(message); // Ignore errors if no receivers
    continue; // Don't fall through to trade channel
}
```

### Step 6: Update Broadcast Section (Lines ~304-328)

**Current:**
```rust
match tx.send(message) {
```

**Change to:**
```rust
match tx_trades.send(message) {
```

### Step 7: Update `start_websocket_server` Signature (Line ~344-346)

**Current:**
```rust
async fn start_websocket_server(addr: SocketAddr, tx: Arc<broadcast::Sender<MarketEventMessage>>) {
```

**Change to:**
```rust
async fn start_websocket_server(
    addr: SocketAddr,
    tx_trades: Arc<broadcast::Sender<MarketEventMessage>>,
    tx_l2: Arc<broadcast::Sender<MarketEventMessage>>,
) {
```

### Step 8: Update Client Spawn in `start_websocket_server` (Line ~354-355)

**Current:**
```rust
let tx = tx.clone();
tokio::spawn(handle_client(stream, peer_addr, tx));
```

**Change to:**
```rust
let tx_trades = tx_trades.clone();
let tx_l2 = tx_l2.clone();
tokio::spawn(handle_client(stream, peer_addr, tx_trades, tx_l2));
```

### Step 9: Update `handle_client` Signature (Lines ~359-364)

**Current:**
```rust
async fn handle_client(
    stream: TcpStream,
    peer_addr: SocketAddr,
    tx: Arc<broadcast::Sender<MarketEventMessage>>,
) {
```

**Change to:**
```rust
async fn handle_client(
    stream: TcpStream,
    peer_addr: SocketAddr,
    tx_trades: Arc<broadcast::Sender<MarketEventMessage>>,
    tx_l2: Arc<broadcast::Sender<MarketEventMessage>>,
) {
```

### Step 10: Update Client Send Task (Lines ~376, ~388-412)

**Current:**
```rust
let mut rx = tx.subscribe();

let mut send_task = tokio::spawn(async move {
    loop {
        match rx.recv().await {
            Ok(event) => {
                if let Ok(json) = serde_json::to_string(&event) {
                    if ws_sender.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
            }
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                warn!("Client {} lagged, skipped {} messages", peer_addr, skipped);
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => {
                info!("Broadcast channel closed for {}", peer_addr);
                break;
            }
        }
    }
});
```

**Change to:**
```rust
let mut rx_trades = tx_trades.subscribe();
let mut rx_l2 = tx_l2.subscribe();

let mut send_task = tokio::spawn(async move {
    loop {
        // Use biased select to prioritize trades over L2
        tokio::select! {
            biased;

            // PRIORITY 1: Trades (hot path)
            result = rx_trades.recv() => {
                match result {
                    Ok(event) => {
                        if let Ok(json) = serde_json::to_string(&event) {
                            if ws_sender.send(Message::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!("Client {} trade channel lagged, skipped {} messages", peer_addr, skipped);
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("Trade channel closed for {}", peer_addr);
                        break;
                    }
                }
            }

            // PRIORITY 2: L2 (lower priority, can lag)
            result = rx_l2.recv() => {
                match result {
                    Ok(event) => {
                        if let Ok(json) = serde_json::to_string(&event) {
                            if ws_sender.send(Message::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        // L2 lag is OK - just log at debug level
                        debug!("Client {} L2 channel lagged, skipped {} messages", peer_addr, skipped);
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // L2 channel closed but trades still work
                        debug!("L2 channel closed for {}", peer_addr);
                        // Don't break - continue receiving trades
                    }
                }
            }
        }
    }
});
```

---

## Testing Plan

### Test 1: Build and Deploy
```bash
# Kill existing server
pkill -9 -f barter-data-server

# Build release
cargo build --release -p barter-data-server

# Start with production logging
RUST_LOG=warn ./target/release/barter-data-server &
```

### Test 2: Clean Latency Test (No TUIs, 60 seconds)
```bash
# Run latency test script
/opt/homebrew/Caskroom/miniconda/base/bin/python3 << 'EOF'
import asyncio
import websockets
import json
import time
from datetime import datetime, timezone
from collections import defaultdict

async def run_test():
    uri = "ws://127.0.0.1:9001"
    latencies = defaultdict(list)

    async with websockets.connect(uri) as ws:
        await ws.recv()  # skip welcome

        print("Collecting 60 seconds of latency data...")
        start = time.time()
        while time.time() - start < 60:
            msg = await ws.recv()
            recv_time = datetime.now(timezone.utc)

            try:
                data = json.loads(msg)
                if data.get('kind') == 'trade':
                    exch_time = datetime.fromisoformat(data['time_exchange'].replace('Z', '+00:00'))
                    recv_server = datetime.fromisoformat(data['time_received'].replace('Z', '+00:00'))

                    # Processing latency (what we control)
                    proc_ms = (recv_time - recv_server).total_seconds() * 1000
                    latencies[data['exchange']].append(proc_ms)
            except:
                pass

        # Print results
        print("\n" + "="*70)
        print("PROCESSING LATENCY (recv→client) - TARGET: p99 < 20ms")
        print("="*70)

        all_vals = []
        for exch in sorted(latencies.keys()):
            arr = sorted(latencies[exch])
            n = len(arr)
            if n < 10:
                continue
            all_vals.extend(arr)
            p50 = arr[n//2]
            p95 = arr[int(n*0.95)]
            p99 = arr[int(n*0.99)] if n > 100 else arr[-1]
            print(f"{exch:<25} p50={p50:>6.1f}ms  p95={p95:>6.1f}ms  p99={p99:>6.1f}ms  n={n}")

        all_vals.sort()
        n = len(all_vals)
        print("\n" + "-"*70)
        print(f"ALL EXCHANGES ({n} samples):")
        print(f"  p50: {all_vals[n//2]:.1f}ms")
        print(f"  p95: {all_vals[int(n*0.95)]:.1f}ms")
        print(f"  p99: {all_vals[int(n*0.99)]:.1f}ms")
        print(f"  Max: {max(all_vals):.1f}ms")

        # Pass/Fail
        p99 = all_vals[int(n*0.99)]
        if p99 < 20:
            print(f"\n✅ PASS: p99 ({p99:.1f}ms) < 20ms target")
        else:
            print(f"\n❌ FAIL: p99 ({p99:.1f}ms) >= 20ms target")

asyncio.run(run_test())
EOF
```

**Expected Result:** p99 < 20ms

### Test 3: Verify L2 Still Works (Start TUIs)
```bash
# Start market_microstructure TUI
./target/release/market-microstructure &

# Wait 10 seconds, check L2 panel shows data
# Look for "Book: XX% BID | BNC:XX% BBT:XX%" line
```

**Expected Result:** L2 imbalance values appear in TUI

### Test 4: Latency Under Load (With TUIs Running)
```bash
# With TUIs running, re-run latency test
# Same script as Test 2
```

**Expected Result:** p99 still < 20ms even with TUI clients connected

### Test 5: Verify No Regressions
```bash
# Check all event types are received
/opt/homebrew/Caskroom/miniconda/base/bin/python3 << 'EOF'
import asyncio
import websockets
import json
from collections import Counter

async def check_events():
    uri = "ws://127.0.0.1:9001"
    kinds = Counter()

    async with websockets.connect(uri) as ws:
        await ws.recv()  # welcome

        for _ in range(1000):
            msg = await ws.recv()
            data = json.loads(msg)
            kinds[data.get('kind', 'unknown')] += 1

        print("Event types received:")
        for k, v in sorted(kinds.items()):
            print(f"  {k}: {v}")

        # Verify critical types present
        required = ['trade', 'order_book_l2']
        missing = [r for r in required if kinds[r] == 0]
        if missing:
            print(f"\n❌ MISSING: {missing}")
        else:
            print(f"\n✅ All required event types present")

asyncio.run(check_events())
EOF
```

**Expected Result:** Both `trade` and `order_book_l2` events received

---

## Rollback Plan

If issues arise:
```bash
# Revert to previous version
git checkout barter-data-server/src/main.rs

# Rebuild and redeploy
cargo build --release -p barter-data-server
pkill -9 -f barter-data-server
RUST_LOG=warn ./target/release/barter-data-server &
```

---

## Success Criteria

| Metric | Target | Current |
|--------|--------|---------|
| Processing latency p50 | < 2ms | 0.4ms ✅ |
| Processing latency p95 | < 15ms | 60ms ❌ |
| Processing latency p99 | < 20ms | 100ms+ ❌ |
| L2 data flows to TUIs | Yes | Yes ✅ |
| No client code changes | Yes | Yes ✅ |

After implementation:
| Metric | Target | Expected |
|--------|--------|----------|
| Processing latency p50 | < 2ms | < 1ms |
| Processing latency p95 | < 15ms | < 10ms |
| Processing latency p99 | < 20ms | < 15ms |

---

## Handoff Prompt for LLM Coder Agent

```
You are implementing L2 separation in barter-data-server to achieve <20ms p99 trade latency.

FILE TO MODIFY: barter-data-server/src/main.rs (759 lines)

CHANGES REQUIRED (in order):

1. Line ~27-31: Replace single L2_THROTTLE_MS constant with per-exchange constants:
   - L2_THROTTLE_BINANCE_MS = 100
   - L2_THROTTLE_BYBIT_MS = 100
   - L2_THROTTLE_OKX_MS = 150

2. Line ~32: Add helper function get_l2_throttle_ms(exchange: &str) -> u64

3. Lines ~100-110: Create TWO broadcast channels:
   - tx_trades (10k buffer) for trades/liqs/OI/CVD/L1
   - tx_l2 (50k buffer) for L2 orderbook only

4. Lines ~118-121: Pass both channels to start_websocket_server

5. Lines ~228-265: Route L2 events to tx_l2 channel (not tx_trades)
   - Keep the throttling logic but use per-exchange rates
   - Add `continue` after sending to tx_l2 so it doesn't fall through

6. Lines ~304-328: Change tx.send() to tx_trades.send()

7. Line ~344-346: Update start_websocket_server signature to take both channels

8. Line ~354-355: Pass both channels to handle_client

9. Lines ~359-364: Update handle_client signature to take both channels

10. Lines ~376, ~388-412: Update send_task to use tokio::select! with biased:
    - Priority 1: rx_trades.recv()
    - Priority 2: rx_l2.recv()
    - L2 lag logged at debug level (OK to skip)
    - Trade lag logged at warn level

CONSTRAINTS:
- Do NOT modify any TUI code
- Do NOT change the WebSocket port (keep 9001)
- Do NOT remove any existing functionality
- Keep all logging at current levels

AFTER CHANGES:
1. cargo build --release -p barter-data-server
2. Run latency test (see Testing Plan in this document)
3. Verify p99 < 20ms
4. Verify L2 still shows in TUIs
```

---

## Reference: Current Code Structure

```
barter-data-server/src/main.rs
├── Constants (L2_THROTTLE_MS)                    Line 27-28
├── MarketEventMessage struct                     Line 30-91
├── main() async                                  Line 93-341
│   ├── Create broadcast channel                  Line 100-110
│   ├── Spawn WebSocket server                    Line 118-121
│   ├── Init market streams                       Line 127
│   ├── L2 throttle state (Mutex<HashMap>)        Line 138
│   └── Main event loop                           Line 141-341
│       ├── Event type detection                  Line 197-200
│       ├── L2 throttle check (BOTTLENECK)        Line 228-265
│       └── Broadcast to clients                  Line 304-328
├── start_websocket_server()                      Line 344-357
├── handle_client()                               Line 359-446
│   ├── Subscribe to broadcast                    Line 376
│   └── Send task (loop rx.recv)                  Line 388-412
├── init_market_streams()                         Line 448-579
└── Helper functions                              Line 645-758
```
