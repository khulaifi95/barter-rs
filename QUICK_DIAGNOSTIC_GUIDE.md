# QUICK DIAGNOSTIC GUIDE - 15-20 Minute Evidence Gathering

**Goal**: Confirm exact failure mode and decide on fix within 15-20 minutes

---

## STEP 1: Run Diagnostic Capture (5-10 minutes)

```bash
cd /Users/screener-m3/projects/barter-rs

# Run the diagnostic capture script
./run_diagnostic_capture.sh

# The script will:
# - Kill existing server
# - Start server with diagnostic logging
# - Capture all output
# - Wait for you to press ENTER
# - Analyze and display results
```

**Let it run for 5-10 minutes** to capture sufficient trade activity.

---

## STEP 2: Interpret Results (2 minutes)

The script will automatically analyze and show:

### Scenario A: Messages Being Dropped ⚠️

```
⚠️  CRITICAL: Found 150 messages being dropped!

Sample of dropped messages:
[BYBIT TRADE DEBUG] ⚠️  MESSAGE WITHOUT TOPIC - WILL BE DROPPED!
[BYBIT TRADE DEBUG] Full message: {
  "type": "delta",
  "ts": 1732428486868,
  "data": [ ... ]
}

🔍 DIAGNOSIS: Messages are arriving but missing 'topic' field
✅ FIX: Remove topic check in deserializer (Option 1)
```

**This confirms our hypothesis** → Proceed to FIX (Step 3)

### Scenario B: No Messages At All ⚠️

```
⚠️  WARNING: No Bybit trade messages received at all!
🔍 DIAGNOSIS: Subscription or channel issue - messages not arriving
```

**Different problem** → Check subscriptions, not parser

### Scenario C: Messages Processing OK ✅

```
✅ Bybit trade messages received and processed
  Total messages: 500
  Successfully deserialized: 500
```

**Parser works!** → Issue is downstream (server/TUI)

---

## STEP 3: Apply Fix Based on Evidence (5 minutes)

### If Scenario A (Messages Dropped - MOST LIKELY)

Edit `barter-data/src/exchange/bybit/trade.rs` lines 46-64:

**REMOVE THIS** (the topic check):
```rust
match value.get("topic") {
    Some(topic) if topic.is_string() => {
        eprintln!("[BYBIT TRADE DEBUG] ✓ Processing message with topic");
        let raw = serde_json::to_string(&value).map_err(serde::de::Error::custom)?;
        match serde_json::from_str::<BybitPayload<Vec<BybitTradeInner>>>(&raw) {
            Ok(payload) => {
                eprintln!("[BYBIT TRADE DEBUG] ✓ Successfully deserialized {} trades", payload.data.len());
                Ok(BybitTradeMessage::Payload(payload))
            }
            Err(e) => {
                eprintln!("[BYBIT TRADE DEBUG] ✗ Deserialization failed: {}", e);
                Err(serde::de::Error::custom(e))
            }
        }
    }
    _ => {
        eprintln!("[BYBIT TRADE DEBUG] → Ignoring message (no topic or non-string topic)");
        Ok(BybitTradeMessage::Ignore)
    }
}
```

**REPLACE WITH THIS** (attempt deserialization always):
```rust
// Attempt to deserialize regardless of topic field
let raw = serde_json::to_string(&value).map_err(serde::de::Error::custom)?;
match serde_json::from_str::<BybitPayload<Vec<BybitTradeInner>>>(&raw) {
    Ok(payload) => {
        eprintln!("[BYBIT TRADE DEBUG] ✓ Successfully deserialized {} trades", payload.data.len());
        Ok(BybitTradeMessage::Payload(payload))
    }
    Err(_) => {
        // Only ignore if genuine deserialization failure (heartbeat, subscription response)
        eprintln!("[BYBIT TRADE DEBUG] → Not a trade message, ignoring");
        Ok(BybitTradeMessage::Ignore)
    }
}
```

**Build and test**:
```bash
cd barter-data && cargo build
cd ../barter-data-server && cargo build
./run_diagnostic_capture.sh  # Run for 5-10 min
```

### If Scenario B (No Messages)

Check subscriptions in `barter-data-server/src/main.rs`:

```bash
# Search for Bybit trade subscriptions
grep -A 5 'BybitSpot.*PublicTrades' barter-data-server/src/main.rs
grep -A 5 'BybitFutures.*PublicTrades' barter-data-server/src/main.rs

# Check WebSocket URLs
grep 'wss://stream.bybit.com' barter-data/src/exchange/bybit/*/mod.rs
```

Verify channel names match API:
- Spot: `publicTrade.{SYMBOL}`
- Perpetual: `publicTrade.{SYMBOL}`

### If Scenario C (Messages OK, But Not Displaying)

Issue is in server broadcast or TUI:

```bash
# Check if trades are being broadcast
grep 'Broadcast.*receivers' /tmp/bybit_trade_diagnostic.log | grep -i trade

# Check server broadcast channel
grep 'tx.send' barter-data-server/src/main.rs -A 3

# Check TUI aggregation
grep 'public_trades\|trade' barter-trading-tuis/src/shared/state.rs
```

---

## STEP 4: Verify Fix (5-10 minutes)

After applying fix, run validation:

```bash
# Rebuild everything
cd /Users/screener-m3/projects/barter-rs
cd barter-data && cargo build
cd ../barter-data-server && cargo build

# Run for 5-10 minutes
./run_diagnostic_capture.sh

# Should see:
# ✅ Successfully deserialized: <high count>
# ✅ Converted to MarketEvents: <high count>
# ✅ SPOT TRADE >=50k events appearing
```

**Success criteria**:
- Bybit trade messages: > 100 in 5 minutes
- Successfully deserialized: > 100 in 5 minutes
- SPOT TRADE events: > 0 (if large trades occur)
- No "DROPPED" warnings

---

## MANUAL ANALYSIS (If Needed)

If the script doesn't give clear results, manual analysis:

```bash
LOG=/tmp/bybit_trade_diagnostic.log

# 1. Check if trade messages are arriving
grep -c '\[BYBIT TRADE DEBUG\] Message received' $LOG

# 2. Check topic field status
grep 'has .topic. field: false' $LOG | head -10

# 3. See dropped messages
grep -A 15 'MESSAGE WITHOUT TOPIC' $LOG | head -50

# 4. See successfully processed
grep 'Successfully deserialized' $LOG | head -20

# 5. Check for deserialization errors
grep 'Deserialization failed' $LOG | head -20

# 6. Count trades converted to events
grep -c 'Converting .* Bybit trades to MarketEvents' $LOG

# 7. Check server-side spot trade logs
grep 'SPOT TRADE >=50k' $LOG | head -20

# 8. View full message samples
grep -A 20 'Full message:' $LOG | head -100
```

---

## DECISION TREE

```
START
  │
  ├─> Diagnostic shows "DROPPED" messages?
  │   └─> YES → Apply Fix (remove topic check)
  │         │
  │         ├─> Rebuild & test 5-10 min
  │         └─> SUCCESS → Remove diagnostic logs, commit
  │
  ├─> Diagnostic shows NO messages at all?
  │   └─> YES → Check subscriptions/channels
  │         │
  │         └─> Fix subscription → Test 5-10 min
  │
  └─> Diagnostic shows messages processing OK?
      └─> YES → Issue is in server/TUI
            │
            └─> Check broadcast channel & TUI aggregation
```

---

## EXPECTED TIMELINE

| Phase | Duration | Activity |
|-------|----------|----------|
| Diagnostic capture | 5-10 min | Run script, let server collect data |
| Analysis | 2 min | Review script output, identify issue |
| Apply fix | 3 min | Edit code based on evidence |
| Rebuild | 2 min | Compile barter-data + server |
| Verify fix | 5-10 min | Run diagnostic again, confirm trades flow |
| **TOTAL** | **17-27 min** | Complete diagnosis + fix + verification |

---

## CONFIDENCE SHORTCUTS

### High Confidence (95%): Messages dropped due to missing "topic"
**Evidence needed**:
- `grep -c 'MESSAGE WITHOUT TOPIC' $LOG` → > 50
- `grep -c 'has .topic. field: false' $LOG` → > 50
- Sample messages show valid trade data but no "topic"

**Action**: Apply fix immediately (remove topic check)

### Medium Confidence (70%): Wrong channel or subscription
**Evidence needed**:
- `grep -c '\[BYBIT TRADE DEBUG\]' $LOG` → 0
- Subscription confirmations show success
- WebSocket connected

**Action**: Check channel names, subscription payloads

### Low Confidence (40%): Downstream issue (server/TUI)
**Evidence needed**:
- `grep -c 'Successfully deserialized' $LOG` → > 100
- `grep -c 'SPOT TRADE' $LOG` → 0
- Broadcasts show low receiver count

**Action**: Check server broadcast and TUI connection

---

## QUICK COMMANDS REFERENCE

```bash
# Run diagnostic (5-10 min capture)
./run_diagnostic_capture.sh

# Quick check after capture
LOG=/tmp/bybit_trade_diagnostic.log
grep -c 'MESSAGE WITHOUT TOPIC' $LOG        # Should be high if our hypothesis is right
grep -c 'Successfully deserialized' $LOG    # Should be 0 if messages are dropped
grep 'SPOT TRADE >=50k' $LOG | wc -l       # Should be > 0 if large trades occur

# If messages are dropped, apply fix and rebuild
cd barter-data && cargo build
cd ../barter-data-server && cargo build
./run_diagnostic_capture.sh  # Re-run for 5-10 min

# Success check
grep -c 'Successfully deserialized' $LOG    # Should be > 100 after fix
grep 'SPOT TRADE' $LOG | tail -10          # Should show recent trades
```

---

## REMOVING DIAGNOSTIC LOGS (After Fix Confirmed)

Once fix is working, clean up the diagnostic logging:

1. Edit `barter-data/src/exchange/bybit/trade.rs`
2. Remove all `eprintln!` statements
3. Keep the fix (no topic check)
4. Rebuild: `cd barter-data && cargo build && cd ../barter-data-server && cargo build`
5. Test without diagnostic logs
6. Commit the clean fix

**Clean fix should be ~10 lines of code** (just the deserialization logic, no logging).

---

**Speed is key. Gather evidence → Decide → Fix → Verify. 15-20 minutes total.**
