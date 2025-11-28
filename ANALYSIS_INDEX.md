# TRADE MESSAGE FLOW ANALYSIS - DOCUMENT INDEX

This folder contains a comprehensive analysis of the barter-data-server trade message routing and WebSocket handling system.

## Documents Overview

### 1. MESSAGE_FLOW_SUMMARY.txt (Start Here)
**Purpose**: High-level overview and quick reference guide
**Contents**:
- Problem statement
- Complete 9-stage message flow with locations
- 5 critical failure points
- Root cause hypothesis
- Expected vs actual behavior
- Debugging strategy

**Key Findings**:
- Trade messages enter at network layer (confirmed)
- SILENTLY DROPPED at deserialization stage (most likely)
- Failure Point #1: BybitTradeMessage::deserialize() missing "topic" field
- Files: `barter-data/src/exchange/bybit/trade.rs:30-37`

---

### 2. MESSAGE_FLOW_ANALYSIS.md (Detailed Reference)
**Purpose**: Comprehensive technical documentation
**Contents**:
- Executive summary with context
- 9 detailed entry points with code snippets
- Complete message flow diagrams
- Exchange-specific deserialization logic
- Routing logic with failure modes
- Subscription confirmation/ACK mechanism
- Error handling vs silent drops
- Summary table of all pipeline stages

**Best For**: Understanding the complete system architecture

---

### 3. DEBUGGING_GUIDE.md (Practical Steps)
**Purpose**: Step-by-step debugging instructions
**Contents**:
- Quick diagnosis checklist
- 10 debugging steps with examples
- Code modifications needed for debug logging
- How to interpret debug output
- Common causes and fixes
- Manual test procedures

**Best For**: Actually debugging the issue

---

## Quick Reference: Critical Code Locations

### Failure Point #1: Deserialization (MOST LIKELY CULPRIT)
```
File: barter-data/src/exchange/bybit/trade.rs:30-37
Function: BybitTradeMessage::deserialize()
Issue: if value.get("topic") returns None → BybitTradeMessage::Ignore
Impact: Silent drop, message never reaches transformer
```

### Failure Point #2: Routing
```
File: barter-data/src/transformer/stateless.rs:66-69
Function: StatelessTransformer::transform()
Issue: if input.id() returns None → return vec![]
Impact: Silent drop for Ignore variant messages
```

### Failure Point #3: Parse Error (Logged)
```
File: barter-data/src/exchange/bybit/message.rs:62-95
Function: de_message_subscription_id()
Issue: Topic format doesn't match "<type>.<symbol>"
Impact: Deserialization error (NOT silent)
```

### Failure Point #4: Map Lookup (Logged)
```
File: barter-data/src/transformer/stateless.rs:72
Function: StatelessTransformer::transform()
Issue: subscription_id not in instrument_map
Impact: Error result (NOT silent)
```

### Success Point: Application Logging
```
File: barter-data-server/src/main.rs:142-151
Where: Main event loop processes MarketEvent
Output: "SPOT TRADE >=50k ..." entries in server_debug.log
```

---

## Root Cause Hypothesis

**Most Likely**: Bybit publicTrade messages missing "topic" field at root level

**Evidence**:
1. Raw WebSocket capture shows messages arriving at network level
2. server_debug.log shows ZERO "SPOT TRADE" entries (recent)
3. Historical logs (01:50-01:52) show entries were working
4. Liquidations ARE working (same deserialization pattern)
5. No error messages (indicates silent drop, not error)

**Consequence**:
- Every trade message becomes `BybitTradeMessage::Ignore`
- Transformer returns empty vec (line 68)
- Message never reaches application
- No log entry generated
- No broadcast to clients

---

## How to Verify

### Step 1: Check Raw WebSocket Format
```bash
# Look for "topic" field in captured messages
grep '"topic"' ws_capture.log | head -5

# Or examine the full message structure
cat ws_capture.log | grep "publicTrade" | head -1 | jq .
```

### Step 2: Enable Debug Logging
Add these debug prints at strategic points:
- `trade.rs:30`: Check if "topic" field exists
- `stateless.rs:66`: Check if id() returns None
- `stateless.rs:72`: Check if map lookup succeeds

### Step 3: Rebuild and Run
```bash
cargo build --bin barter_data_server
./target/debug/barter_data_server 2>&1 | grep -E '\[DEBUG\]|SPOT TRADE'
```

### Step 4: Analyze Output
Compare actual flow with expected flow documented in this analysis

---

## Key Files & Functions

| File | Function | Line | Purpose |
|------|----------|------|---------|
| integration/stream/mod.rs | ExchangeStream::poll_next | 41 | Core async polling loop |
| integration/stream/mod.rs | Protocol::parse | 56 | Message deserialization |
| exchange/bybit/trade.rs | BybitTradeMessage::deserialize | 23 | Trade message parsing |
| exchange/bybit/trade.rs | input.id() match | 113 | ID extraction |
| exchange/bybit/message.rs | de_message_subscription_id | 62 | Topic parsing |
| transformer/stateless.rs | transform | 64 | Message routing |
| transformer/stateless.rs | map.find | 72 | Instrument lookup |
| main.rs | Event loop | 124 | Application processing |
| main.rs | Trade logging | 142 | SERVER_DEBUG.LOG output |
| main.rs | Broadcast | 207 | Send to clients |

---

## Expected Message Flow

```
Network → ExchangeStream::poll() → Protocol::parse() 
    → BybitTradeMessage::deserialize()
    → (Check: "topic" field) ← DECISION POINT
    → BybitTradeMessage::Payload | Ignore
    → Transformer::transform()
    → (Check: id() Some/None) ← DECISION POINT
    → instrument_map lookup
    → (Check: Found/Not Found) ← DECISION POINT
    → MarketEvent<PublicTrade>
    → Main event loop
    → (Check: DataKind::Trade) ← DECISION POINT
    → Log: "SPOT TRADE >=50k ..."
    → Broadcast to clients
```

---

## Logging Output Examples

### When Working (Expected):
```
[BYBIT_TRADE] Received message with topic field
[TRANSFORMER] Processing message, subscription_id: Some("publicTrade|BTCUSDT")
[INIT] Instrument map entries: 3
SPOT TRADE >=50k BybitSpot btc/usdt @ 50000 qty 0.001 notional 50.00 side Buy
```

### When Broken (Current):
```
[BYBIT_TRADE] Received message WITHOUT topic field
[TRANSFORMER] DROPPED - No subscription ID!
[INIT] Instrument map entries: 3
(No SPOT TRADE entries)
```

---

## Debugging Priority

1. **P0 - Verify raw WebSocket format**
   - Does ws_capture.log show "topic" field?
   - Is format correct?

2. **P1 - Enable debug logging**
   - Add eprintln! at trade.rs:30
   - Rebuild and capture output

3. **P2 - Check subscription state**
   - Verify "validated exchange WebSocket subscriptions" in logs
   - Count subscription confirmations

4. **P3 - Check instrument map**
   - Print instrument_map contents during init
   - Verify subscription IDs match

5. **P4 - Manual deserialization test**
   - Parse sample Bybit message manually
   - Verify serde deserializer works

---

## Files in This Analysis

- **MESSAGE_FLOW_SUMMARY.txt** - High-level overview and reference
- **MESSAGE_FLOW_ANALYSIS.md** - Complete technical documentation
- **DEBUGGING_GUIDE.md** - Step-by-step debugging instructions
- **ANALYSIS_INDEX.md** - This file

---

## Next Actions

1. Read MESSAGE_FLOW_SUMMARY.txt (5 min)
2. Review MESSAGE_FLOW_ANALYSIS.md (15 min)
3. Follow DEBUGGING_GUIDE.md steps (30 min)
4. Implement fix based on findings (varies)

Good luck with the debugging!

