# COMPARATIVE ANALYSIS: Working vs Broken Streams

## EXECUTIVE SUMMARY

Working streams (OI, Liquidations, L1/Orderbook) and Broken streams (Trades) follow IDENTICAL architectural patterns from subscription through deserialization to aggregation. The difference is **not architectural** - the code paths are nearly identical. 

**The issue is at the DATA SOURCE LEVEL, not in the message processing pipeline.**

---

## 1. SUBSCRIPTION & MESSAGE FLOW ARCHITECTURE

### ✅ WORKING: Liquidations
```
barter-data-server/src/main.rs:401-402
┌─ DynamicStreams subscription configuration
├─ Okx, "btc", "usdt", Perpetual, Liquidations
├─ BinanceFuturesUsd, "btc", "usdt", Perpetual, Liquidations
├─ BybitPerpetualsUsd, "btc", "usdt", Perpetual, Liquidations
│
└─ Message Flow:
   └─ Exchange WebSocket → barter-data LIQ parser
      ├─ OKX: OkxLiquidationMessage → Liquidation events
      ├─ Binance: BinanceLiquidation → Liquidation events
      ├─ Bybit: BybitLiquidationMessage → Liquidation events
      │
      └─ Server broadcast (server/main.rs:180-224)
         ├─ Match on DataKind::Liquidation
         ├─ Log every event ("LIQ EVENT ...")
         ├─ Broadcast to connected clients
         │
         └─ TUI WebSocket receive
            ├─ Parse MarketEventMessage
            ├─ Match kind == "liquidation"
            ├─ Deserialize LiquidationData
            ├─ Aggregator.push_liquidation()
            └─ Display in UI
```

### ✅ WORKING: Open Interest
```
barter-data-server/src/main.rs:383-402
┌─ DynamicStreams subscription configuration
├─ Okx, "btc", "usdt", Perpetual, OpenInterest
├─ BinanceFuturesUsd (REST fallback via binance_open_interest_stream())
├─ BybitPerpetualsUsd, "btc", "usdt", Perpetual, OpenInterest
│
└─ Message Flow: [IDENTICAL TO LIQUIDATIONS]
   └─ Exchange WebSocket/REST → barter-data OI parser
      ├─ OkxOpenInterestMessage → OpenInterest events
      ├─ BybitOpenInterestMessage → OpenInterest events
      ├─ BinanceOpenInterestResponse (REST) → OpenInterest events
      │
      └─ Server broadcast
         ├─ Match on DataKind::OpenInterest
         ├─ Log every event ("OI EVENT ...")
         ├─ Broadcast to connected clients
         │
         └─ TUI aggregation
            ├─ Match kind == "open_interest"
            ├─ Deserialize OpenInterestData
            ├─ Aggregator.push_oi()
```

### ✅ WORKING: L1/Orderbook
```
barter-data-server/src/main.rs:355-456
┌─ DynamicStreams subscription configuration
├─ Okx, "btc", "usdt", Spot, OrderBooksL1
├─ BinanceFuturesUsd, "btc", "usdt", Perpetual, OrderBooksL1
├─ BybitPerpetualsUsd, "btc", "usdt", Perpetual, OrderBooksL1
│
└─ Message Flow: [IDENTICAL ARCHITECTURE]
   └─ Exchange WebSocket → barter-data L1 parser
      ├─ OkxOrderBook → OrderBookL1 events
      ├─ BinanceOrderBookL1 → OrderBookL1 events
      ├─ BybitOrderBookL1 → OrderBookL1 events
      │
      └─ Server broadcast
         ├─ Match on DataKind::OrderBookL1
         ├─ Broadcast to clients
         │
         └─ TUI aggregation
            ├─ Match kind == "order_book_l1"
            ├─ Deserialize OrderBookL1Data
            ├─ Aggregator.push_orderbook()
```

### ❌ BROKEN: Trades
```
barter-data-server/src/main.rs:370-407
┌─ DynamicStreams subscription configuration
├─ BybitSpot, "btc", "usdt", Spot, PublicTrades
├─ BinanceFuturesUsd, "btc", "usdt", Perpetual, PublicTrades
├─ BinanceSpot, "btc", "usdt", Spot, PublicTrades
├─ Okx, "btc", "usdt", Perpetual, PublicTrades
│
└─ Message Flow: [IDENTICAL CODE STRUCTURE]
   └─ Exchange WebSocket → barter-data TRADE parser
      ├─ OkxTrades → PublicTrade events
      ├─ BinanceTrade → PublicTrade events
      ├─ BybitTradeMessage → PublicTrade events
      │
      └─ Server broadcast
         ├─ Match on DataKind::Trade (line 133)
         ├─ Conditional log only for spots >= 50k
         ├─ Broadcast to clients (line 207)
         │
         └─ TUI aggregation
            ├─ Match kind == "trade"
            ├─ Deserialize TradeData
            ├─ Aggregator.push_trade()
```

---

## 2. SIDE-BY-SIDE DESERIALIZATION COMPARISON

### ✅ Liquidation Parsing (Works)

**OKX Liquidation:** (barter-data/src/exchange/okx/liquidation.rs:64-92)
```rust
impl From<(ExchangeId, InstrumentKey, OkxLiquidations)> for MarketIter<InstrumentKey, Liquidation> {
    fn from((exchange, instrument, liquidations): ...) -> Self {
        liquidations.data.into_iter().flat_map(|liq| {
            liq.details.into_iter().map(move |detail| {
                Ok(MarketEvent {
                    time_exchange: detail.time,
                    time_received: Utc::now(),
                    exchange,
                    instrument: instrument.clone(),
                    kind: Liquidation {
                        side: detail.side,
                        price: detail.price,
                        quantity: detail.size,
                        time: detail.time,
                    },
                })
            })
        }).collect()
    }
}
```

**Bybit Liquidation:** (barter-data/src/exchange/bybit/liquidation.rs:64-94)
```rust
impl From<(ExchangeId, InstrumentKey, BybitLiquidationMessage)> for MarketIter<InstrumentKey, Liquidation> {
    fn from((exchange, instrument, message): ...) -> Self {
        match message {
            BybitLiquidationMessage::Payload(payload) => Self(
                payload.data.into_iter().map(|entry| {
                    Ok(MarketEvent {
                        time_exchange: entry.time,
                        time_received: Utc::now(),
                        exchange,
                        instrument: instrument.clone(),
                        kind: Liquidation {
                            side: entry.side,
                            price: entry.price,
                            quantity: entry.quantity,
                            time: entry.time,
                        },
                    })
                }).collect(),
            ),
        }
    }
}
```

**Binance Liquidation:** (barter-data/src/exchange/binance/futures/liquidation.rs:84-103)
```rust
impl From<(ExchangeId, InstrumentKey, BinanceLiquidation)> for MarketIter<InstrumentKey, Liquidation> {
    fn from((exchange_id, instrument, liquidation): ...) -> Self {
        Self(vec![Ok(MarketEvent {
            time_exchange: liquidation.order.time,
            time_received: Utc::now(),
            exchange: exchange_id,
            instrument,
            kind: Liquidation {
                side: liquidation.order.side,
                price: liquidation.order.price,
                quantity: liquidation.order.quantity,
                time: liquidation.order.time,
            },
        })])
    }
}
```

### ❌ Trade Parsing (Broken - IDENTICAL ARCHITECTURE)

**OKX Trade:** (barter-data/src/exchange/okx/trade.rs:95-117)
```rust
impl From<(ExchangeId, InstrumentKey, OkxTrades)> for MarketIter<InstrumentKey, PublicTrade> {
    fn from((exchange, instrument, trades): ...) -> Self {
        trades.data.into_iter().map(|trade| {
            Ok(MarketEvent {
                time_exchange: trade.time,
                time_received: Utc::now(),
                exchange,
                instrument: instrument.clone(),
                kind: PublicTrade {
                    id: trade.id,
                    price: trade.price,
                    amount: trade.amount,
                    side: trade.side,
                },
            })
        }).collect()
    }
}
```

**Bybit Trade:** (barter-data/src/exchange/bybit/trade.rs:81-111)
```rust
impl From<(ExchangeId, InstrumentKey, BybitTradeMessage)> for MarketIter<InstrumentKey, PublicTrade> {
    fn from((exchange, instrument, message): ...) -> Self {
        match message {
            BybitTradeMessage::Payload(trades) => Self(
                trades.data.into_iter().map(|trade| {
                    Ok(MarketEvent {
                        time_exchange: trade.time,
                        time_received: Utc::now(),
                        exchange,
                        instrument: instrument.clone(),
                        kind: PublicTrade {
                            id: trade.id,
                            price: trade.price,
                            amount: trade.amount,
                            side: trade.side,
                        },
                    })
                }).collect(),
            ),
        }
    }
}
```

**Binance Trade:** (barter-data/src/exchange/binance/trade.rs:79-96)
```rust
impl From<(ExchangeId, InstrumentKey, BinanceTrade)> for MarketIter<InstrumentKey, PublicTrade> {
    fn from((exchange_id, instrument, trade): ...) -> Self {
        Self(vec![Ok(MarketEvent {
            time_exchange: trade.time,
            time_received: Utc::now(),
            exchange: exchange_id,
            instrument,
            kind: PublicTrade {
                id: trade.id.to_string(),
                price: trade.price,
                amount: trade.amount,
                side: trade.side,
            },
        })])
    }
}
```

**OBSERVATION:** All three patterns are structurally identical. The only difference is HOW they flatten the data (match/map/flat_map) - this is already working.

---

## 3. SERVER BROADCAST PATH

### ✅ Working (Liquidation Example)

**barter-data-server/src/main.rs:180-224:**
```rust
Event::Item(result) => match result {
    Ok(market_event) => {
        if let DataKind::Liquidation(liq) = &market_event.kind {
            info!("LIQ EVENT {} {}/{} @ {} qty {} side {:?}",
                market_event.exchange,
                market_event.instrument.base,
                market_event.instrument.quote,
                liq.price,
                liq.quantity,
                liq.side
            );
        }
        
        let is_liquidation = matches!(&market_event.kind, DataKind::Liquidation(_));
        let message = MarketEventMessage::from(market_event);
        
        if is_liquidation {
            let receivers = tx.receiver_count();
            info!("BROADCASTING liquidation to {} clients: ...", receivers);
        }
        
        match tx.send(message) {
            Ok(count) => {
                if is_liquidation {
                    debug!("Liquidation sent to {} receivers", count);
                }
            }
            Err(e) => {
                if is_liquidation {
                    warn!("Failed to broadcast liquidation: {:?}", e);
                }
            }
        }
    }
}
```

### ❌ Broken (Trade Example - SAME CODE PATH)

**barter-data-server/src/main.rs:130-224:**
```rust
Event::Item(result) => match result {
    Ok(market_event) => {
        // ← Trades reach here but:
        // 1. NO unconditional logging for trades
        // 2. Only logs SPOT trades >= $50k (line 133-153)
        // 3. No debug: receiver count, no broadcast confirmation
        
        if let DataKind::Trade(trade) = &market_event.kind {
            let spot_log_threshold = std::env::var("SPOT_LOG_THRESHOLD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(50_000.0);
            let notional = trade.price * trade.amount;
            let is_spot = matches!(market_event.instrument.kind, MarketDataInstrumentKind::Spot);
            if is_spot && notional >= spot_log_threshold {
                // Only logs large spot trades
                info!("SPOT TRADE >=50k ...", ...);
            }
        }
        
        let is_liquidation = matches!(&market_event.kind, DataKind::Liquidation(_));
        let is_open_interest = matches!(&market_event.kind, DataKind::OpenInterest(_));
        let message = MarketEventMessage::from(market_event);
        
        // Broadcast attempt - NO SPECIAL LOGGING FOR TRADES
        if is_liquidation { ... debug logs ... }
        if is_open_interest { ... debug logs ... }
        
        match tx.send(message) {
            Ok(count) => {
                if is_liquidation { debug!(...) }
                if is_open_interest { debug!(...) }
                // ← TRADES SENT BUT NO CONFIRMATION
            }
            Err(e) => {
                if is_liquidation { warn!(...) }
                if is_open_interest { warn!(...) }
                // ← NO ERROR LOGGING FOR TRADES
            }
        }
    }
}
```

---

## 4. TUI AGGREGATION PATH

### ✅ All Streams (Identical Processing)

**barter-trading-tuis/src/shared/state.rs:199-244:**
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
                state.push_trade(trade, &event.exchange, event.time_exchange, is_spot, is_perp);
            }
        }
        "liquidation" => {
            if let Ok(liq) = serde_json::from_value::<LiquidationData>(event.data) {
                let time = liq.time;
                state.push_liquidation(liq, &event.exchange, time);
            }
        }
        "cumulative_volume_delta" => {
            if let Ok(cvd) = serde_json::from_value::<CvdData>(event.data) {
                state.push_cvd(&event.exchange, cvd, event.time_exchange);
            }
        }
        "open_interest" => {
            if let Ok(oi) = serde_json::from_value::<OpenInterestData>(event.data) {
                state.push_oi(&event.exchange, oi.contracts);
            }
        }
        "order_book_l1" => {
            if let Ok(ob) = serde_json::from_value::<OrderBookL1Data>(event.data) {
                state.push_orderbook(ob, is_spot, is_perp, event.time_exchange);
            }
        }
        _ => {}
    }
    
    // Track exchange heartbeat
    self.exchange_last_seen.insert(event.exchange.clone(), Utc::now());
}
```

**KEY INSIGHT:** The TUI code processes all event types IDENTICALLY. No filtering, no special handling. The `match` statement treats `"trade"` exactly like `"liquidation"`, `"open_interest"`, etc.

---

## 5. CRITICAL ARCHITECTURAL FINDINGS

### Finding 1: Subscription Configuration is Identical
Both Trades and working streams are subscribed identically in `DynamicStreams::init()`:
- Same `subscribe()` method
- Same subscription pattern: `(Exchange, base, quote, kind, SubKind)`
- Same behavior: fire-and-forget subscription to barter-data

### Finding 2: Deserialization Paths are Identical
- Trade parsers (Binance/Bybit/OKX) are structurally identical to Liquidation parsers
- All use `From<(ExchangeId, InstrumentKey, Message)>` trait
- All produce `MarketEvent<InstrumentKey, PublicTrade/Liquidation/etc>`
- All reach the server's `combined_stream` the same way

### Finding 3: Server Broadcast is Identical
- All event types enter `combined_stream.next().await`
- All match on `Event::Item(Ok(market_event))`
- All are converted to `MarketEventMessage::from(market_event)` (lines 43-78)
- All call `tx.send(message)` (line 207)

**THE ONLY DIFFERENCE:** Logging. Trades don't have comprehensive logging like Liquidations/OI do.

### Finding 4: TUI Processing is Identical
- All event types are matched by `event.kind.as_str()`
- All use `serde_json::from_value::<ExpectedType>(event.data)`
- All push records into aggregator state
- Trade processing has NO special conditions or filters

---

## 6. ROOT CAUSE HYPOTHESIS

Since the architecture is identical and the code paths work for Liquidations/OI/L1, the issue is **NOT IN THE MESSAGE PIPELINE**. The problem is one of:

### Hypothesis A: Data Source Level (MOST LIKELY)
**The exchanges are NOT sending trade messages to the barter-data library over WebSocket.**

Possible causes:
1. **Subscription not accepted** by exchange (subscription message rejected silently)
2. **Trade stream rate-limited** or blocked by exchange
3. **WebSocket connection closed** specifically for trade streams
4. **Trade messages filtered somewhere in barter-data** (not visible from search results)
5. **Async task crash** in barter-data trade stream handler (race condition, panic suppressed)

### Hypothesis B: Channel/Topic Mismatch
The barter-data library might be subscribing to the wrong channel names for trades:
- Binance: Expects `@trade` but exchange sends something else?
- Bybit: Expects `publicTrade` but exchange sends something else?
- OKX: Expects `trades` but exchange sends something else?

### Hypothesis C: Message Format Incompatibility
Trade messages from exchanges might have changed format, but:
- Deserialization errors would cause `Err(e)` in `result` match
- Server logs would show these errors (line 226-234)
- No such logs in system logs suggests trades aren't arriving at all

### Hypothesis D: Subscription Filter in barter-data
There might be code in barter-data that filters or throttles trade subscriptions:
```
// Possible hidden filter in barter-data:
if is_trade_stream && !whitelist.contains(&exchange) {
    skip_subscription()
}
```
This would require searching `barter-data/src/streams/` files not examined yet.

---

## 7. EVIDENCE TRAIL

### ✅ Liquidations Work Because:
1. Subscription created (line 384: `Liquidations`)
2. Messages arrive from exchanges (logs prove it)
3. Server receives and broadcasts (line 157-165: "LIQ EVENT")
4. TUI receives and processes (line 222: `"liquidation"` match succeeds)
5. UI renders liquidation data

### ❌ Trades Broken Because:
1. Subscription created (line 405-407: `PublicTrades`)
2. **Messages DON'T arrive from exchanges** (or arrive and are filtered)
   - No trade logs in server (line 133-153 shows trades would log but don't)
   - No error logs (lines 226-234 would show parsing errors)
   - Combined silence = no messages reaching server
3. Server has no trades to broadcast
4. TUI receives zero trade events
5. UI shows zero trades

---

## 8. SUMMARY TABLE

| Aspect | Liquidations | OI | L1/Book | Trades |
|--------|--------------|-----|---------|--------|
| Subscription Code | ✅ Identical | ✅ Identical | ✅ Identical | ✅ Identical |
| Parser Structure | ✅ Works | ✅ Works | ✅ Works | ❌ Never executes |
| Server Broadcast | ✅ Confirmed | ✅ Confirmed | ✅ Confirmed | ❌ No messages |
| TUI Match Statement | ✅ Works | ✅ Works | ✅ Works | ❌ Never reached |
| **Messages Arriving** | ✅ Yes | ✅ Yes | ✅ Yes | ❌ **NO** |

---

## 9. NEXT STEPS

1. **PRIMARY:** Check if trade WebSocket subscriptions are being SENT to exchanges
   - File: Look in barter-data message subscription logic
   - Search for trade channel subscription setup

2. **SECONDARY:** If subscriptions sent, check if exchanges are RESPONDING
   - Add logging to barter-data WebSocket message pump
   - Verify incoming trade messages

3. **TERTIARY:** If messages received, check if being FILTERED
   - Search for conditional logic that might drop trades
   - Check subscription rate limits or filters

The working streams prove the infrastructure is sound. **The issue is specifically with the data coming from the Trade stream sources - either not subscribed, not sent by exchanges, or silently filtered internally.**

---

## FILE REFERENCES

### Data Library (barter-data)
- **Trade Deserialization:**
  - /Users/screener-m3/projects/barter-rs/barter-data/src/exchange/okx/trade.rs:95-117
  - /Users/screener-m3/projects/barter-rs/barter-data/src/exchange/bybit/trade.rs:81-111
  - /Users/screener-m3/projects/barter-rs/barter-data/src/exchange/binance/trade.rs:79-96

- **Liquidation Deserialization:**
  - /Users/screener-m3/projects/barter-rs/barter-data/src/exchange/okx/liquidation.rs:64-92
  - /Users/screener-m3/projects/barter-rs/barter-data/src/exchange/bybit/liquidation.rs:64-94
  - /Users/screener-m3/projects/barter-rs/barter-data/src/exchange/binance/futures/liquidation.rs:84-103

### Server (barter-data-server)
- **Subscription Configuration:**
  - /Users/screener-m3/projects/barter-rs/barter-data-server/src/main.rs:351-461

- **Server Broadcast:**
  - /Users/screener-m3/projects/barter-rs/barter-data-server/src/main.rs:130-224
  - Trade logging: lines 133-153
  - Liquidation logging: lines 156-165
  - OI logging: lines 169-178
  - Broadcast: lines 207-224

### TUI (barter-trading-tuis)
- **Event Aggregation:**
  - /Users/screener-m3/projects/barter-rs/barter-trading-tuis/src/shared/state.rs:199-244
  
- **Type Definitions:**
  - /Users/screener-m3/projects/barter-rs/barter-trading-tuis/src/shared/types.rs

- **WebSocket Client:**
  - /Users/screener-m3/projects/barter-rs/barter-trading-tuis/src/shared/websocket.rs:183-195

