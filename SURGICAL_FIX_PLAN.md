# SURGICAL FIX PLAN: TRADE STREAM RESTORATION

**Created**: 2025-11-24
**Issue**: Bybit trade messages silently dropped during deserialization
**Root Cause**: Two-stage conditional deserialization returns `Ignore` when "topic" field missing
**Confidence**: 95%
**Risk Level**: MEDIUM (touching core deserialization logic)

---

## PRE-FIX VERIFICATION (DO FIRST!)

Before implementing any fix, perform these verification steps to confirm the diagnosis:

### Step 1: Examine actual WebSocket messages

```bash
# Check if ws_capture.log exists and contains Bybit trade messages
cat ws_capture.log | grep -i "publictrade" | head -20

# Look for messages WITHOUT "topic" field
cat ws_capture.log | grep -A 5 "publictrade" | grep -v "topic"
```

**What to look for**:
- Messages with "topic" field → Should be processed correctly
- Messages WITHOUT "topic" field → Being silently dropped
- Message format variations (snapshot vs delta)

### Step 2: Check current server logs

```bash
# Check recent server logs for any trade messages
tail -100 server_debug.log | grep -i "trade"

# Count trade messages vs OI/Liquidation messages
grep -c "SPOT TRADE" server_debug.log
grep -c "LIQ EVENT" server_debug.log
grep -c "OI EVENT" server_debug.log
```

**Expected**:
- Trade count: 0 or very low
- Liquidation count: High
- OI count: High

### Step 3: Verify Bybit connection is active

```bash
# Check server logs for Bybit subscription confirmations
grep -i "bybit" server_debug.log | grep -i "success\|subscribe"
```

**Expected**:
- Successful subscription messages for Bybit publicTrade channels
- No error messages about failed subscriptions

### Step 4: Test with live WebSocket connection (Optional)

```bash
# Install websocat if needed: brew install websocat

# Connect to Bybit spot and subscribe to BTC trades
echo '{"op":"subscribe","args":["publicTrade.BTCUSDT"]}' | \
  websocat wss://stream.bybit.com/v5/public/spot

# Leave running for 30 seconds and observe message format
```

**What to look for**:
- Format of first message (snapshot with "topic"?)
- Format of subsequent messages (delta without "topic"?)
- Presence or absence of "topic" field

---

## FIX OPTIONS

Three fix options presented in order of preference (safest to most aggressive):

---

## OPTION 1: MINIMAL PATCH (RECOMMENDED)

**Strategy**: Make "topic" field optional in deserialization without changing overall structure

**Risk Level**: LOW
**Complexity**: LOW
**Testing Required**: MEDIUM
**Reversibility**: EASY

### Implementation

**File**: `barter-data/src/exchange/bybit/trade.rs`

**Change**: Modify the custom deserializer to attempt deserialization even without "topic"

```rust
// BEFORE (Lines 23-40)
impl<'de> Deserialize<'de> for BybitTradeMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;

        match value.get("topic") {
            Some(topic) if topic.is_string() => {
                let raw = serde_json::to_string(&value).map_err(serde::de::Error::custom)?;
                serde_json::from_str(&raw)
                    .map(BybitTradeMessage::Payload)
                    .map_err(serde::de::Error::custom)
            }
            _ => Ok(BybitTradeMessage::Ignore),  // ← PROBLEM
        }
    }
}

// AFTER (OPTION 1: Try deserialization regardless of "topic")
impl<'de> Deserialize<'de> for BybitTradeMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;

        // Attempt deserialization regardless of "topic" field presence
        let raw = serde_json::to_string(&value).map_err(serde::de::Error::custom)?;
        match serde_json::from_str::<BybitPayload<Vec<BybitTradeInner>>>(&raw) {
            Ok(payload) => Ok(BybitTradeMessage::Payload(payload)),
            Err(_) => {
                // Only return Ignore if deserialization truly fails
                // This catches subscription responses and heartbeats
                Ok(BybitTradeMessage::Ignore)
            }
        }
    }
}
```

**Changes**:
1. Remove "topic" field check
2. Always attempt to deserialize into `BybitPayload<Vec<BybitTradeInner>>`
3. Only return `Ignore` if deserialization genuinely fails
4. Let `BybitPayload` handle "topic" field through its own deserializer

### Pros
- Minimal code change
- Maintains existing structure
- Automatically handles both snapshot and delta formats
- Other parsers (liquidations, OI) unaffected

### Cons
- Still uses `Ignore` variant (silent drops remain possible)
- Relies on `BybitPayload` deserializer to handle missing "topic"
- May need to make "topic" field optional in `BybitPayload` struct

### Verification Steps

1. **Apply the change**:
```bash
cd /Users/screener-m3/projects/barter-rs/barter-data
# Edit src/exchange/bybit/trade.rs
```

2. **Build and check for compile errors**:
```bash
cargo build
```

3. **Run server with debug logging**:
```bash
cd /Users/screener-m3/projects/barter-rs/barter-data-server
cargo run > server_test.log 2>&1 &
SERVER_PID=$!
```

4. **Monitor for 2 minutes**:
```bash
sleep 120
```

5. **Check trade messages**:
```bash
grep -c "SPOT TRADE" server_test.log
grep "publicTrade" server_test.log | head -20
```

6. **Stop server**:
```bash
kill $SERVER_PID
```

7. **Expected results**:
   - Trade messages appear in logs
   - No deserialization errors
   - OI and Liquidations still work

---

## OPTION 2: STRUCTURAL FIX (THOROUGH)

**Strategy**: Remove the `Ignore` variant entirely and handle message filtering differently

**Risk Level**: MEDIUM
**Complexity**: MEDIUM
**Testing Required**: HIGH
**Reversibility**: MODERATE

### Implementation

**File**: `barter-data/src/exchange/bybit/trade.rs`

**Changes**:
1. Remove `BybitTradeMessage` enum entirely
2. Use `BybitPayload<Vec<BybitTradeInner>>` directly
3. Make "topic" field optional in `BybitPayload`
4. Handle subscription responses separately

```rust
// BEFORE: Enum with Ignore variant
#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum BybitTradeMessage {
    Payload(BybitPayload<Vec<BybitTradeInner>>),
    Ignore,
}

// AFTER: Direct type alias
pub type BybitTradeMessage = BybitPayload<Vec<BybitTradeInner>>;

// Make topic optional in BybitPayload (barter-data/src/exchange/bybit/message.rs)
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Deserialize, Serialize)]
pub struct BybitPayload<T> {
    #[serde(alias = "topic", deserialize_with = "de_optional_subscription_id", default)]
    pub subscription_id: Option<SubscriptionId>,  // ← Make optional

    #[serde(rename = "type")]
    pub kind: BybitPayloadKind,

    #[serde(
        alias = "ts",
        deserialize_with = "barter_integration::de::de_u64_epoch_ms_as_datetime_utc"
    )]
    pub time: DateTime<Utc>,

    pub data: T,
}
```

**Update From implementation**:
```rust
// BEFORE
impl<InstrumentKey: Clone> From<(ExchangeId, InstrumentKey, BybitTradeMessage)>
    for MarketIter<InstrumentKey, PublicTrade>
{
    fn from((exchange, instrument, message): (ExchangeId, InstrumentKey, BybitTradeMessage)) -> Self {
        match message {
            BybitTradeMessage::Ignore => Self(vec![]),
            BybitTradeMessage::Payload(trades) => {
                Self(
                    trades.data.into_iter()
                        .map(|trade| Ok(MarketEvent { ... }))
                        .collect()
                )
            }
        }
    }
}

// AFTER
impl<InstrumentKey: Clone> From<(ExchangeId, InstrumentKey, BybitTradeMessage)>
    for MarketIter<InstrumentKey, PublicTrade>
{
    fn from((exchange, instrument, trades): (ExchangeId, InstrumentKey, BybitTradeMessage)) -> Self {
        Self(
            trades.data.into_iter()
                .map(|trade| {
                    Ok(MarketEvent {
                        time_exchange: trade.time,
                        time_received: Utc::now(),
                        exchange,
                        instrument: instrument.clone(),
                        kind: PublicTrade {
                            id: trade.id.clone(),
                            price: trade.price,
                            amount: trade.amount,
                            side: trade.side,
                        },
                    })
                })
                .collect()
        )
    }
}
```

### Pros
- Eliminates silent drops completely
- Simpler type structure
- Forces explicit error handling
- Matches Binance/OKX pattern (direct deserialization)

### Cons
- Requires changes to `BybitPayload` struct (affects liquidations, OI)
- More extensive testing required
- May need to filter subscription responses at different layer

### Verification Steps

1. **Apply changes to both files**:
   - `barter-data/src/exchange/bybit/trade.rs`
   - `barter-data/src/exchange/bybit/message.rs`

2. **Build and fix compile errors**:
```bash
cd barter-data
cargo build 2>&1 | tee build_errors.log
```

3. **Update liquidation parser if needed** (it uses same `BybitPayload`):
   - `barter-data/src/exchange/bybit/liquidation.rs`

4. **Run full test suite**:
```bash
cargo test --package barter-data --lib exchange::bybit
```

5. **Integration test with server**:
```bash
cd ../barter-data-server
cargo run > server_test.log 2>&1 &
sleep 120
grep "TRADE\|LIQ\|OI" server_test.log
```

6. **Expected results**:
   - All Bybit streams work (trades, liquidations, OI)
   - Errors are explicit (not silent)
   - Message counts increase for trades

---

## OPTION 3: ADD DEBUG LOGGING FIRST (DIAGNOSTIC)

**Strategy**: Add temporary logging to confirm diagnosis before fixing

**Risk Level**: VERY LOW
**Complexity**: VERY LOW
**Testing Required**: LOW
**Reversibility**: TRIVIAL

### Implementation

**File**: `barter-data/src/exchange/bybit/trade.rs`

**Add temporary debug logging**:

```rust
impl<'de> Deserialize<'de> for BybitTradeMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;

        // TEMPORARY DEBUG LOGGING
        if let Some(data) = value.get("data") {
            eprintln!("[BYBIT TRADE DEBUG] Received message with data field");
            if !value.get("topic").is_some() {
                eprintln!("[BYBIT TRADE DEBUG] WARNING: Message has no 'topic' field!");
                eprintln!("[BYBIT TRADE DEBUG] Full message: {}", serde_json::to_string_pretty(&value).unwrap_or_default());
            }
        }

        match value.get("topic") {
            Some(topic) if topic.is_string() => {
                eprintln!("[BYBIT TRADE DEBUG] Processing message with topic: {}", topic);
                let raw = serde_json::to_string(&value).map_err(serde::de::Error::custom)?;
                serde_json::from_str(&raw)
                    .map(BybitTradeMessage::Payload)
                    .map_err(serde::de::Error::custom)
            }
            _ => {
                eprintln!("[BYBIT TRADE DEBUG] Ignoring message (no topic)");
                Ok(BybitTradeMessage::Ignore)
            }
        }
    }
}
```

### Pros
- Zero risk (only adds logging)
- Confirms diagnosis with real data
- Shows actual message format
- Easy to remove after diagnosis

### Cons
- Doesn't fix the issue
- Creates verbose logs
- Temporary change only

### Verification Steps

1. **Add debug logging**:
```bash
cd /Users/screener-m3/projects/barter-rs/barter-data
# Edit src/exchange/bybit/trade.rs with debug statements
```

2. **Build**:
```bash
cargo build
```

3. **Run server and capture stderr**:
```bash
cd ../barter-data-server
cargo run 2> bybit_trade_debug.log &
SERVER_PID=$!
```

4. **Wait 2 minutes for messages**:
```bash
sleep 120
```

5. **Examine debug output**:
```bash
grep "BYBIT TRADE DEBUG" bybit_trade_debug.log | head -50
```

6. **Count dropped messages**:
```bash
grep -c "WARNING: Message has no 'topic' field" bybit_trade_debug.log
grep -c "Ignoring message" bybit_trade_debug.log
```

7. **Stop server**:
```bash
kill $SERVER_PID
```

8. **Analyze results**:
   - If many "WARNING: Message has no 'topic' field" → Diagnosis confirmed
   - If zero warnings → Issue is elsewhere
   - Examine full message JSON to understand format

---

## RECOMMENDED EXECUTION SEQUENCE

### Phase 1: Diagnosis Confirmation (OPTION 3)
1. Add debug logging
2. Run server for 2-5 minutes
3. Analyze debug output
4. Confirm messages without "topic" are being dropped
5. Document actual message format

### Phase 2: Minimal Fix (OPTION 1)
1. Remove debug logging
2. Implement minimal patch (remove "topic" check)
3. Test for 5 minutes
4. Verify trades appear
5. Verify OI/Liquidations still work

### Phase 3: Full Fix (OPTION 2) - If Option 1 fails
1. Implement structural fix
2. Make "topic" optional in `BybitPayload`
3. Update all parsers (trade, liquidation, OI)
4. Full regression testing

---

## VERIFICATION CHECKLIST

After applying any fix, verify:

### Functional Verification
- [ ] BTC perpetual trades flowing (Bybit)
- [ ] ETH perpetual trades flowing (Bybit)
- [ ] SOL perpetual trades flowing (Bybit)
- [ ] BTC spot trades flowing (Bybit)
- [ ] ETH spot trades flowing (Bybit)
- [ ] SOL spot trades flowing (Bybit)
- [ ] Binance trades still work
- [ ] OKX trades still work
- [ ] Liquidations still work (all exchanges)
- [ ] Open Interest still works (all exchanges)
- [ ] OrderBook L1 still works

### Performance Verification
- [ ] No memory leaks (monitor with `htop`)
- [ ] No CPU spikes
- [ ] Message latency acceptable (<100ms)
- [ ] No broadcast channel lagging warnings

### Error Verification
- [ ] No deserialization errors in logs
- [ ] No subscription failures
- [ ] No WebSocket disconnections
- [ ] Graceful handling of malformed messages

### Log Verification
```bash
# Count messages by type
grep -c "SPOT TRADE" server_debug.log    # Should be > 0
grep -c "LIQ EVENT" server_debug.log     # Should be > 0
grep -c "OI EVENT" server_debug.log      # Should be > 0

# Check for errors
grep -i "error\|fail\|panic" server_debug.log

# Verify all exchanges
grep "SPOT TRADE" server_debug.log | grep -o "[A-Z][a-z]*Spot" | sort | uniq -c
# Expected: BinanceSpot, BybitSpot, Okx (all present)
```

---

## ROLLBACK PLAN

If any fix causes issues:

### Immediate Rollback (Git)
```bash
cd /Users/screener-m3/projects/barter-rs/barter-data
git checkout -- src/exchange/bybit/trade.rs
git checkout -- src/exchange/bybit/message.rs  # if Option 2 was attempted
cargo build
```

### Restart Server
```bash
cd /Users/screener-m3/projects/barter-rs/barter-data-server
# Kill current server
pkill -f barter-data-server

# Restart
cargo run > server.log 2>&1 &
```

### Verify Rollback
```bash
# OI and Liquidations should still work
tail -f server.log | grep "LIQ EVENT\|OI EVENT"
```

---

## ALTERNATIVE: UPSTREAM FIX

If local fixes are too risky or don't work:

### File Upstream Issue

Create GitHub issue in `barter-rs/barter-data` repository:

**Title**: Bybit trade messages silently dropped when "topic" field missing

**Body**:
```markdown
## Issue Description
Bybit public trade messages are being silently dropped during deserialization when the "topic" field is missing or malformed.

## Root Cause
`BybitTradeMessage::deserialize()` in `src/exchange/bybit/trade.rs` implements a two-stage deserialization pattern that returns `BybitTradeMessage::Ignore` when the "topic" field is absent.

## Evidence
- Bybit may send delta updates without "topic" field
- Custom deserializer checks for "topic" before attempting full deserialization
- `Ignore` variant converts to empty vector with no error
- Liquidations work because messages always include "topic"

## Proposed Fix
Remove the "topic" field check and attempt deserialization regardless, allowing natural errors to propagate.

## Impact
- Affects all Bybit trade streams (spot and perpetuals)
- Binance and OKX unaffected (different parser design)
- Liquidations and OI unaffected (different message format)

## Files Affected
- `barter-data/src/exchange/bybit/trade.rs:23-40`
```

---

## POST-FIX VALIDATION

Once trades are flowing again:

### 1. Run for 24 hours
Monitor continuously to ensure stability

### 2. Verify data quality
```bash
# Check trade counts are reasonable
grep -c "SPOT TRADE" server_debug.log

# Verify all exchanges present
grep "SPOT TRADE" server_debug.log | cut -d' ' -f4 | sort | uniq -c

# Check for any anomalies
grep "SPOT TRADE" server_debug.log | awk '{print $7}' | sort -n | tail -20
```

### 3. Compare to historical data
- Trade volume similar to before the issue?
- Price levels match external sources?
- Side distribution reasonable (not all buy or all sell)?

### 4. Monitor TUI display
- Whale detection panel shows fresh data
- Timestamps are current (not stale)
- Multiple exchanges represented

### 5. Commit the fix
```bash
cd /Users/screener-m3/projects/barter-rs
git add barter-data/src/exchange/bybit/trade.rs
git commit -m "fix: handle Bybit trade messages without 'topic' field

Bybit may send trade delta updates without the 'topic' field,
causing the custom deserializer to silently drop these messages
by returning the Ignore variant.

This fix attempts deserialization regardless of 'topic' field
presence, only returning Ignore for genuine deserialization
failures (subscription responses, heartbeats, etc.).

Resolves silent trade message drops for Bybit spot and perpetuals.
"
```

---

## RISK MITIGATION

### Backup Current State
```bash
cd /Users/screener-m3/projects/barter-rs
git stash push -m "before-bybit-trade-fix"
git branch backup-before-trade-fix
```

### Test in Isolation
```bash
# Create test branch
git checkout -b fix/bybit-trade-silent-drops

# Make changes
# ... apply fix ...

# Test thoroughly before merging
```

### Canary Testing
1. Fix only BTC trade stream first
2. Monitor for 30 minutes
3. If successful, enable ETH and SOL
4. Monitor for 1 hour
5. If successful, consider fix complete

---

## SUCCESS CRITERIA

The fix is successful when ALL of the following are true:

✅ Bybit spot trades flowing (visible in logs)
✅ Bybit perpetual trades flowing (visible in logs)
✅ Binance trades still working
✅ OKX trades still working
✅ Liquidations still working (all exchanges)
✅ Open Interest still working (all exchanges)
✅ No deserialization errors
✅ No silent message drops
✅ TUI whale panel shows fresh data
✅ Server stable for 24+ hours
✅ All 6 Bybit streams operational (3 spot + 3 perpetuals for BTC/ETH/SOL)

---

## TIMELINE ESTIMATE

| Phase | Duration | Activities |
|-------|----------|-----------|
| **Diagnosis** (Option 3) | 30 min | Add logging, run server, analyze output |
| **Minimal Fix** (Option 1) | 1 hour | Implement, test, verify |
| **Full Fix** (Option 2) | 3-4 hours | Implement, update related code, full testing |
| **Validation** | 24 hours | Monitor stability, verify data quality |
| **Documentation** | 1 hour | Update docs, commit with proper message |

**Total**: 1.5 hours (Option 1) to 29 hours (Option 2 with full validation)

---

## FINAL RECOMMENDATION

**Execute in this order**:

1. **Pre-fix verification** (15 min)
   - Examine ws_capture.log
   - Confirm current state
   - Document message format

2. **Option 3: Add debug logging** (30 min)
   - Confirm diagnosis with real data
   - Understand exact message format
   - Document findings

3. **Option 1: Minimal patch** (1 hour)
   - Safest fix with smallest code change
   - High probability of success
   - Easy rollback if issues

4. **Only if Option 1 fails → Option 2** (4 hours)
   - More thorough but riskier
   - Requires extensive testing
   - Consider upstream contribution

**DO NOT break the system. Be surgical. Verify at each step.**
