# DEBUGGING GUIDE: Trade Message Silent Drop Issue

## Quick Diagnosis Checklist

- [ ] Verify raw WebSocket capture shows "topic" field in trade messages
- [ ] Check subscription validation confirms successful ACK
- [ ] Enable debug logging at deserialization points
- [ ] Verify instrument_map contains expected subscription IDs
- [ ] Check if recent logs show any subscription errors
- [ ] Verify Bybit API hasn't changed message format

---

## Step 1: Verify Raw WebSocket Messages Have "topic" Field

### What to Look For:
Raw Bybit trade messages MUST have this structure:
```json
{
    "topic": "publicTrade.BTCUSDT",
    "type": "snapshot",
    "ts": 1234567890,
    "data": [
        {
            "T": 1234567890,
            "s": "BTCUSDT",
            "S": "Buy",
            "v": "0.001",
            "p": "50000.00",
            "L": "PlusTick",
            "i": "trade-id-123",
            "BT": false
        }
    ]
}
```

### Check Existing Captures:
```bash
# Look at the WebSocket capture from earlier
cat ws_capture.log | head -100

# Search for trade messages with "topic"
grep -o '"topic":"publicTrade' ws_capture.log | head -10

# Check message structure
grep -A 10 '"topic":"publicTrade' ws_capture.log | head -30
```

---

## Step 2: Add Debug Logging to Key Points

### Location 1: Deserialization (barter-data/src/exchange/bybit/trade.rs:28)

Add before the match statement:
```rust
eprintln!("[BYBIT_TRADE] Received message with keys: {:?}", value.keys());
```

### Location 2: Routing (barter-data/src/transformer/stateless.rs:66)

Add at function start:
```rust
eprintln!("[TRANSFORMER] Processing message, subscription_id: {:?}", input.id());
```

### Location 3: Instrument Map (barter-data/src/lib.rs:244)

Add after subscription:
```rust
eprintln!("[INIT] Instrument map entries: {}", instrument_map.inner().len());
for key in instrument_map.inner().keys() {
    eprintln!("[INIT]   - {}", key);
}
```

---

## Step 3: Rebuild and Observe Output

```bash
cd /Users/screener-m3/projects/barter-rs
cargo build --bin barter_data_server 2>&1 | tail -20
./target/debug/barter_data_server 2>&1 | grep -E '\[BYBIT_TRADE\]|\[TRANSFORMER\]|\[INIT\]|SPOT TRADE' | head -50
```

---

## Step 4: Cross-Check with Liquidation Logs

Liquidations ARE working (confirmed in server_debug.log).
Liquidations use:
- File: `barter-data/src/exchange/bybit/liquidation.rs`
- Topic: "allLiquidation"

Compare with trades:
- File: `barter-data/src/exchange/bybit/trade.rs`
- Topic: "publicTrade"

Both use same deserialization pattern. If liquidations work but trades don't:
→ The "topic" field for trades might have different format

---

## Step 5: Manual Test - Parse Sample Message

Create test file: `test_bybit_parse.rs`

```rust
use serde_json::Value;

fn main() {
    let sample_trades = r#"
    {
        "topic": "publicTrade.BTCUSDT",
        "type": "snapshot",
        "ts": 1234567890,
        "data": [{"T": 1234567890, "s": "BTCUSDT", "S": "Buy", "v": "0.001", "p": "50000"}]
    }
    "#;
    
    let value: Value = serde_json::from_str(sample_trades).unwrap();
    println!("Has topic: {}", value.get("topic").is_some());
    println!("Topic value: {:?}", value.get("topic"));
}
```

Run:
```bash
rustc --edition 2021 -L $(find /Users/screener-m3/projects/barter-rs/target -name serde_json) test_bybit_parse.rs
./test_bybit_parse
```

---

## Likely Root Causes & Checks

### Cause #1: Topic Field Missing (Most Likely)
**Check**: Does ws_capture.log show "topic" in trade messages?
**Fix**: Update `value.get("topic")` path in trade.rs

### Cause #2: Subscription Not Confirmed
**Check**: Look for "validated exchange WebSocket subscriptions" in log
**Fix**: Verify ACK received before messages processed

### Cause #3: Subscription ID Mismatch
**Check**: Compare IDs in instrument_map with message topics
**Fix**: Ensure subscription format matches parser

### Cause #4: Bybit API Changed Format
**Check**: Consult https://bybit-exchange.github.io/docs/v5/websocket/public/trade
**Fix**: Update deserialization logic

