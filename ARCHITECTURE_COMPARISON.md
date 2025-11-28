# MESSAGE FLOW COMPARISON: WORKING vs BROKEN STREAMS

## VISUAL ARCHITECTURE

### ✅ WORKING STREAMS (Liquidations, OI, L1)
```
┌─────────────────────────────────────────────────────────────────────┐
│ EXCHANGE WEBSOCKET (Binance/Bybit/OKX)                              │
│ Sending: liquidation events, open interest updates, orderbook L1    │
└────────────────┬────────────────────────────────────────────────────┘
                 │ ✅ Messages received
                 │
┌────────────────▼────────────────────────────────────────────────────┐
│ BARTER-DATA LIBRARY                                                 │
│ ┌──────────────────────────────────────────────────────────────┐   │
│ │ Stream Consumers (Public/Futures modules)                    │   │
│ │ • OkxLiquidation parser → Liquidation events                │   │
│ │ • BybitLiquidationMessage parser → Liquidation events       │   │
│ │ • BinanceLiquidation parser → Liquidation events            │   │
│ │ • OkxOpenInterest parser → OpenInterest events              │   │
│ │ • BybitOpenInterest parser → OpenInterest events            │   │
│ │ • BinanceOpenInterest parser → OpenInterest events          │   │
│ │ • OrderBook parsers → OrderBookL1 events                    │   │
│ └──────────────────────────────────────────────────────────────┘   │
│                          │                                           │
│                          ▼ MarketEvent<Liquidation/OI/L1>           │
│ ┌──────────────────────────────────────────────────────────────┐   │
│ │ DynamicStreams (subscription manager)                        │   │
│ │ • Aggregates all event types into combined_stream           │   │
│ └──────────────────────────────────────────────────────────────┘   │
└────────────────┬────────────────────────────────────────────────────┘
                 │ ✅ Events flow to consumer
                 │
┌────────────────▼────────────────────────────────────────────────────┐
│ BARTER-DATA-SERVER                                                  │
│ ┌──────────────────────────────────────────────────────────────┐   │
│ │ combined_stream.next().await → Event::Item(Ok(market_event))│   │
│ │                                                              │   │
│ │ if let DataKind::Liquidation(liq) = &market_event.kind {   │   │
│ │     info!("LIQ EVENT {} ...", liq)  ← LOGGED ✅            │   │
│ │ }                                                            │   │
│ │                                                              │   │
│ │ if let DataKind::OpenInterest(oi) = &market_event.kind {   │   │
│ │     info!("OI EVENT {} ...", oi)    ← LOGGED ✅            │   │
│ │ }                                                            │   │
│ │                                                              │   │
│ │ let message = MarketEventMessage::from(market_event);      │   │
│ │ tx.send(message)  ← BROADCAST TO CLIENTS ✅               │   │
│ └──────────────────────────────────────────────────────────────┘   │
└────────────────┬────────────────────────────────────────────────────┘
                 │ ✅ Broadcast to TUIs
                 │
┌────────────────▼────────────────────────────────────────────────────┐
│ TUI WEBSOCKET CLIENT (institutional_flow, etc.)                     │
│ ┌──────────────────────────────────────────────────────────────┐   │
│ │ event_rx.recv() → MarketEventMessage                         │   │
│ │   → Aggregator.process_event()                              │   │
│ │      → match event.kind.as_str() {                          │   │
│ │         "liquidation" → push_liquidation()  ✅               │   │
│ │         "open_interest" → push_oi()        ✅               │   │
│ │         "order_book_l1" → push_orderbook() ✅               │   │
│ │      }                                                       │   │
│ └──────────────────────────────────────────────────────────────┘   │
│                          │                                           │
│                          ▼ TickerSnapshot                           │
│ ┌──────────────────────────────────────────────────────────────┐   │
│ │ UI Rendering (ratatui)                                       │   │
│ │ • Show liquidation clusters                                 │   │
│ │ • Show open interest totals                                 │   │
│ │ • Show orderbook spreads                                    │   │
│ └──────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

### ❌ BROKEN STREAMS (Trades)
```
┌─────────────────────────────────────────────────────────────────────┐
│ EXCHANGE WEBSOCKET (Binance/Bybit/OKX)                              │
│ Configured to send: trade events                                    │
└────────────────┬────────────────────────────────────────────────────┘
                 │ ❌ NO MESSAGES RECEIVED (or silently dropped)
                 │
┌────────────────▼────────────────────────────────────────────────────┐
│ BARTER-DATA LIBRARY                                                 │
│ ┌──────────────────────────────────────────────────────────────┐   │
│ │ Stream Consumers (Public/Futures modules)                    │   │
│ │ • OkxTrade parser [DEFINED but never executes]              │   │
│ │ • BybitTradeMessage parser [DEFINED but never executes]     │   │
│ │ • BinanceTrade parser [DEFINED but never executes]          │   │
│ │ • Trade parsers are IDENTICAL to liquidation parsers        │   │
│ │ • Problem: MESSAGES NEVER ARRIVE TO BE PARSED              │   │
│ └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│ ┌──────────────────────────────────────────────────────────────┐   │
│ │ DynamicStreams (subscription manager)                        │   │
│ │ • Trade subscriptions created                                │   │
│ │ • But no events to aggregate                                 │   │
│ └──────────────────────────────────────────────────────────────┘   │
└────────────────┬────────────────────────────────────────────────────┘
                 │ ❌ Empty stream (no events)
                 │
┌────────────────▼────────────────────────────────────────────────────┐
│ BARTER-DATA-SERVER                                                  │
│ ┌──────────────────────────────────────────────────────────────┐   │
│ │ combined_stream.next().await → [WAITING FOR EVENTS]         │   │
│ │                                                              │   │
│ │ if let DataKind::Trade(trade) = &market_event.kind {       │   │
│ │     // This code NEVER EXECUTES ❌                          │   │
│ │     info!("TRADE EVENT ...")                                │   │
│ │ }                                                            │   │
│ │                                                              │   │
│ │ Only logs spot trades >= $50k, and even those don't arrive  │   │
│ │ No log messages for trades → No events to broadcast         │   │
│ └──────────────────────────────────────────────────────────────┘   │
└────────────────┬────────────────────────────────────────────────────┘
                 │ ❌ Nothing to broadcast
                 │
┌────────────────▼────────────────────────────────────────────────────┐
│ TUI WEBSOCKET CLIENT (institutional_flow, etc.)                     │
│ ┌──────────────────────────────────────────────────────────────┐   │
│ │ event_rx.recv() → [WAITING FOR TRADE MESSAGES]              │   │
│ │   → Aggregator.process_event() [NEVER CALLED FOR TRADES]    │   │
│ │      → match event.kind.as_str() {                          │   │
│ │         "trade" → push_trade()  ❌ NEVER REACHED            │   │
│ │      }                                                       │   │
│ │                                                              │   │
│ │ Trade processing code is IDENTICAL to working streams,      │   │
│ │ but it's never invoked because events never arrive          │   │
│ └──────────────────────────────────────────────────────────────┘   │
│                          │                                           │
│                          ▼ TickerSnapshot (no trades data)          │
│ ┌──────────────────────────────────────────────────────────────┐   │
│ │ UI Rendering (ratatui)                                       │   │
│ │ • Show trade_speed: 0 t/s ❌                                 │   │
│ │ • Show avg_trade_usd: $0 ❌                                  │   │
│ │ • Show vwap: None ❌                                         │   │
│ │ • Show exchange dominance: 0% ❌                             │   │
│ │ • Show orderflow: 0 ❌                                       │   │
│ └──────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

---

## DETAILED CODE PATH COMPARISON

### Path Comparison Table

| Stage | Liquidations ✅ | Trades ❌ | Status |
|-------|-----------------|----------|--------|
| Exchange sends events | YES | ??? (likely NO) | Diverges |
| barter-data parser exists | YES | YES | Same |
| Parser executes | YES | NO | Diverges |
| MarketEvent produced | YES | NO | Diverges |
| DynamicStreams receives | YES | NO | Diverges |
| combined_stream has items | YES | NO | Diverges |
| Server receives event | YES | NO | Diverges |
| Server logs event | YES | NO | Diverges |
| Server broadcasts | YES | NO | Diverges |
| TUI receives message | YES | NO | Diverges |
| TUI parses JSON | YES | NO | Diverges |
| TUI matches kind | YES | NO | Diverges |
| Aggregator processes | YES | NO | Diverges |
| UI displays data | YES | NO | Diverges |

---

## KEY INSIGHT: The Cut-Point

**All architectural divergence happens BEFORE the barter-data-server.**

This tells us:
1. The server is working correctly (proven by liquidations/OI/L1 flowing through)
2. The TUI is working correctly (proven by liquidations/OI/L1 displaying)
3. The WebSocket connection is working (proven by other events flowing)
4. The parsers are coded correctly (same structure as working ones)

**The issue is in barter-data's WebSocket subscription/reception layer:**
- Either trades aren't being subscribed to
- Or subscriptions are rejected
- Or trade messages are arriving but filtered/dropped
- Or async task is crashing on trades specifically

---

## SMOKING GUN: Logging Asymmetry

Looking at server/main.rs lines 130-224:

```rust
// LIQUIDATIONS: Get comprehensive logging
if let DataKind::Liquidation(liq) = &market_event.kind {
    info!("LIQ EVENT ... ");  // Line 157
}
if is_liquidation {
    let receivers = tx.receiver_count();
    info!("BROADCASTING liquidation to {} clients: ...", receivers);  // Line 187-192
}
match tx.send(message) {
    Ok(count) => {
        if is_liquidation {
            debug!("Liquidation sent to {} receivers", count);  // Line 210
        }
    }
}

// TRADES: Get conditional/limited logging
if let DataKind::Trade(trade) = &market_event.kind {
    let spot_log_threshold = ...;
    // Only logs if SPOT AND >= $50k
    if is_spot && notional >= spot_log_threshold {
        info!("SPOT TRADE >=50k ...");  // Line 142
    }
    // NO logging for futures trades at all!
}
// NO broadcast logging for trades!

// OPEN INTEREST: Get comprehensive logging
if let DataKind::OpenInterest(oi) = &market_event.kind {
    info!("OI EVENT ... ");  // Line 169
}
```

**This is NOT a code bug - this is intentional design** to reduce log noise for high-frequency trades. But it also means:
- If trades WERE arriving, we'd only see logs for spot trades >= $50k
- Even those logs are missing
- This strongly suggests: **no trades are arriving at all**

---

## CONCLUSION

The working streams prove the entire pipeline works. The broken trades aren't failing anywhere in the pipeline - they're failing at the source.

**Root cause: Trade messages are not arriving from the exchanges to barter-data.**

Next investigation: Check barter-data's trade subscription logic (not examined in this analysis due to scope).

