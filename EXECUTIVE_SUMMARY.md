# TRADE MESSAGE ROUTING ANALYSIS - EXECUTIVE SUMMARY

## Problem
Bybit publicTrade messages arrive at the network layer but don't appear in server logs or broadcast to clients. System shows ZERO "SPOT TRADE" log entries despite messages being captured at network level.

## Root Cause (Identified)
Trade messages are **silently dropped during deserialization** at:
```
File: barter-data/src/exchange/bybit/trade.rs:30-37
```

The code checks for a "topic" field that may not exist or be in wrong format:
```rust
match value.get("topic") {
    Some(topic) if topic.is_string() => { /* Process */ }
    _ => Ok(BybitTradeMessage::Ignore)  // ← SILENT DROP
}
```

**Result**: Every trade message becomes `Ignore` variant → no logging → no broadcast.

## Complete Message Flow (9 Stages)

1. **Network Reception** → Raw WebSocket message arrives
2. **Async Polling** → `ExchangeStream::poll_next()` reads from stream
3. **Protocol Parse** → `WebSocketSerdeParser` deserializes JSON
4. **Trade Deserialize** → `BybitTradeMessage::deserialize()` [FAILURE POINT]
5. **Message Routing** → `StatelessTransformer::transform()` [SECONDARY FAILURE]
6. **Instrument Lookup** → Maps subscription_id to instrument
7. **Event Conversion** → Creates `MarketEvent<PublicTrade>`
8. **Application Logic** → Main loop checks `DataKind::Trade` [SUCCESS POINT]
9. **Broadcasting** → Logs "SPOT TRADE >=50k ..." and sends to clients

## 5 Critical Failure Points

| # | Location | Trigger | Impact |
|---|----------|---------|--------|
| 1 | trade.rs:30 | Missing "topic" field | Silent drop → Ignore variant |
| 2 | stateless.rs:66 | id() returns None | Silent drop → empty vec |
| 3 | message.rs:62 | Invalid topic format | Error logged (NOT silent) |
| 4 | stateless.rs:72 | Subscription not registered | Error logged (NOT silent) |
| 5 | main.rs:207 | Broadcast overflow | Warning logged (NOT silent) |

**Most likely: Point #1** (Failure Point explains silent drop pattern)

## Key Code Entry Points

```
Entry:  barter-integration/src/stream/mod.rs:56  ← Protocol::parse()
Parse:  barter-data/src/exchange/bybit/trade.rs:23-40
Route:  barter-data/src/transformer/stateless.rs:64
Log:    barter-data-server/src/main.rs:142
Send:   barter-data-server/src/main.rs:207
```

## Evidence of Issue

✓ Raw capture shows messages arriving (network layer works)
✓ Liquidations ARE appearing (same subsystem works)
✓ Historical logs show "SPOT TRADE >=50k" entries (proof system worked)
✓ Recent logs show ZERO trade entries (recent breakage)
✓ No error messages in logs (indicates silent drop, not error)

→ Points to deserialization returning `Ignore` variant

## How Messages Are Dropped (Silently)

```
BybitTradeMessage::Ignore variant created
    ↓
input.id() → returns None (line 113-120)
    ↓
StatelessTransformer::transform() early return (line 66-69)
    ↓
Returns vec![] (empty vector)
    ↓
ExchangeStream buffer gets nothing (line 69-75)
    ↓
Main loop never sees MarketEvent (line 124)
    ↓
No log entry generated
    ↓
No broadcast to clients
```

## Verification Steps (Priority Order)

**P0**: Check `ws_capture.log` for actual message format
- Does it have "topic" field at root level?
- What is the exact value?
- Compare with liquidation messages (working)

**P1**: Add debug logging at `trade.rs:30`
- Print received JSON keys
- Check if "topic" exists
- Rebuild and capture stderr

**P2**: Verify instrument map initialization
- Print map contents during init
- Ensure subscription IDs match expected format

**P3**: Check subscription validation in logs
- Look for "validated exchange WebSocket subscriptions"
- Verify ACK received

**P4**: Manual test deserialization
- Parse sample Bybit message manually
- Verify serde behavior

## Expected Output When Fixed

In server_debug.log:
```
SPOT TRADE >=50k BybitSpot btc/usdt @ 50000 qty 0.001 notional 50.00 side Buy
SPOT TRADE >=50k BybitSpot eth/usdt @ 3000 qty 0.01 notional 30.00 side Sell
```

Currently showing:
```
(no trade entries)
```

## Files Provided in This Analysis

1. **ANALYSIS_INDEX.md** - Start here (document roadmap)
2. **MESSAGE_FLOW_SUMMARY.txt** - Quick reference (all key info)
3. **MESSAGE_FLOW_ANALYSIS.md** - Technical deep-dive (complete reference)
4. **DEBUGGING_GUIDE.md** - Step-by-step debugging (practical instructions)
5. **EXECUTIVE_SUMMARY.md** - This file (high-level overview)

## Recommended Action Plan

1. Read EXECUTIVE_SUMMARY.md (this file) - 5 min
2. Read MESSAGE_FLOW_SUMMARY.txt - 10 min
3. Examine ws_capture.log for "topic" field - 10 min
4. Run P1 debugging steps from DEBUGGING_GUIDE.md - 20 min
5. Implement fix based on findings - varies

## Most Likely Fix

Based on analysis: Update the "topic" field check in `trade.rs:30` to match actual Bybit message format.

Either:
- Change `value.get("topic")` to different field name
- Handle nested field access differently
- Update message format validation

The fix location is clear; the solution depends on actual message format from Bybit.

---

**Next Step**: Examine `ws_capture.log` to determine actual message structure and field names.

