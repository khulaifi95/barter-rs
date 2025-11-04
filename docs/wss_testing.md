# WebSocket Testing Guide - Core Exchanges

Quick reference for testing OKX, Bybit, and Binance WebSocket data feeds in Postman.

---

## 1. OKX

**WebSocket URL:** `wss://ws.okx.com:8443/ws/v5/public`

**Ping Requirement:** Send text `"ping"` every 29 seconds

**Docs:** https://www.okx.com/docs-v5/en/#websocket-api-public-channel

### 1.1 Public Trades
```json
{
  "op": "subscribe",
  "args": [
    {
      "channel": "trades",
      "instId": "BTC-USDT-SWAP"
    }
  ]
}
```
**Expected Response:**
- Subscription confirmation
- Real-time trades with `instId`, `px` (price), `sz` (size), `side`, `ts` (timestamp)

### 1.2 Liquidations
```json
{
  "op": "subscribe",
  "args": [
    {
      "channel": "liquidation-orders",
      "instType": "SWAP"
    }
  ]
}
```
**Note:** Subscribes to ALL SWAP liquidations (not instrument-specific)

**Alternative - Other instrument types:**
```json
{"channel": "liquidation-orders", "instType": "FUTURES"}
{"channel": "liquidation-orders", "instType": "SPOT"}
{"channel": "liquidation-orders", "instType": "OPTION"}
```

**Expected Response:**
- Contains `instId` (e.g., "BTC-USDT-SWAP"), `instFamily`, `details` array
- Each detail has `side`, `bkPx` (price), `sz` (size), `bkLoss` (loss), `ts`

### 1.3 Open Interest
```json
{
  "op": "subscribe",
  "args": [
    {
      "channel": "open-interest",
      "instId": "BTC-USDT-SWAP"
    }
  ]
}
```
**Expected Response:**
- Contains `instId`, `oi` (open interest), `oiCcy` (currency), `ts`

### 1.4 CVD (Cumulative Volume Delta)
Uses the same trades channel as 1.1 above - CVD is calculated client-side from trades.

---

## 2. Binance Spot

**WebSocket URL:** `wss://stream.binance.com:9443/ws`

**Ping Requirement:** None (handled by server)

**Docs:** https://binance-docs.github.io/apidocs/spot/en/#websocket-market-streams

### 2.1 Public Trades
```json
{
  "method": "SUBSCRIBE",
  "params": ["btcusdt@trade"],
  "id": 1
}
```
**Note:** Market must be lowercase

**Expected Response:**
```json
{
  "e": "trade",
  "E": 1234567890,
  "s": "BTCUSDT",
  "t": 12345,
  "p": "50000.00",
  "q": "0.001",
  "m": true,
  "T": 1234567890
}
```

### 2.2 OrderBook L1 (Best Bid/Ask)
```json
{
  "method": "SUBSCRIBE",
  "params": ["btcusdt@bookTicker"],
  "id": 1
}
```
**Expected Response:**
- Contains `u` (update ID), `s` (symbol), `b` (best bid), `B` (bid qty), `a` (best ask), `A` (ask qty)

### 2.3 OrderBook L2 (Depth Updates)
```json
{
  "method": "SUBSCRIBE",
  "params": ["btcusdt@depth@100ms"],
  "id": 1
}
```
**Expected Response:**
- Delta updates with `e`: "depthUpdate", `E` (event time), `s` (symbol), `U` (first update ID), `u` (final update ID)
- `b` (bids array), `a` (asks array)

### 2.4 CVD (Cumulative Volume Delta)
Uses the same trades channel as 2.1 above

---

## 3. Binance Futures USD

**WebSocket URL:** `wss://fstream.binance.com/ws`

**Ping Requirement:** None (handled by server)

**Docs:** https://binance-docs.github.io/apidocs/futures/en/#websocket-market-streams

### 3.1 Public Trades
```json
{
  "method": "SUBSCRIBE",
  "params": ["btcusdt@trade"],
  "id": 1
}
```
**Note:** Market must be lowercase

**Expected Response:** Similar to spot trades

### 3.2 Liquidations
```json
{
  "method": "SUBSCRIBE",
  "params": ["btcusdt@forceOrder"],
  "id": 1
}
```
**Expected Response:**
```json
{
  "e": "forceOrder",
  "E": 1234567890,
  "o": {
    "s": "BTCUSDT",
    "S": "SELL",
    "o": "LIMIT",
    "f": "IOC",
    "q": "0.001",
    "p": "50000.00",
    "ap": "49900.00",
    "X": "FILLED",
    "l": "0.001",
    "z": "0.001",
    "T": 1234567890
  }
}
```

### 3.3 OrderBook L1 (Best Bid/Ask)
```json
{
  "method": "SUBSCRIBE",
  "params": ["btcusdt@bookTicker"],
  "id": 1
}
```

### 3.4 OrderBook L2 (Depth Updates)
```json
{
  "method": "SUBSCRIBE",
  "params": ["btcusdt@depth@100ms"],
  "id": 1
}
```

### 3.5 CVD (Cumulative Volume Delta)
Uses the same trades channel as 3.1 above

---

## 4. Bybit Spot

**WebSocket URL:** `wss://stream.bybit.com/v5/public/spot`

**Ping Requirement:** Send every 5 seconds:
```json
{"op": "ping"}
```

**Docs:** https://bybit-exchange.github.io/docs/v5/ws/connect

### 4.1 Public Trades
```json
{
  "op": "subscribe",
  "args": ["publicTrade.BTCUSDT"]
}
```
**Note:** Market must be uppercase

**Expected Response:**
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
      "i": "abc123"
    }
  ]
}
```

### 4.2 OrderBook L1
```json
{
  "op": "subscribe",
  "args": ["orderbook.1.BTCUSDT"]
}
```

### 4.3 OrderBook L2 (50 levels)
```json
{
  "op": "subscribe",
  "args": ["orderbook.50.BTCUSDT"]
}
```

### 4.4 CVD (Cumulative Volume Delta)
Uses the same trades channel as 4.1 above

---

## 5. Bybit Perpetuals USD (Linear)

**WebSocket URL:** `wss://stream.bybit.com/v5/public/linear`

**Ping Requirement:** Send every 5 seconds:
```json
{"op": "ping"}
```

**Docs:** https://bybit-exchange.github.io/docs/v5/ws/public/all-liquidation

### 5.1 Public Trades
```json
{
  "op": "subscribe",
  "args": ["publicTrade.BTCUSDT"]
}
```

### 5.2 Liquidations (Global - All Instruments)
```json
{
  "op": "subscribe",
  "args": ["allLiquidation"]
}
```
**Note:** This is a global feed for ALL instruments, not symbol-specific

**Expected Response:**
```json
{
  "topic": "allLiquidation",
  "type": "snapshot",
  "ts": 1234567890,
  "data": {
    "updatedTime": 1234567890,
    "symbol": "BTCUSDT",
    "side": "Buy",
    "size": "0.01",
    "price": "50000.00"
  }
}
```

### 5.3 Open Interest (via Tickers)
```json
{
  "op": "subscribe",
  "args": ["tickers.BTCUSDT"]
}
```
**Expected Response:**
- Contains `symbol`, `price24hPcnt`, `openInterest`, `openInterestValue`, etc.

### 5.4 OrderBook L1
```json
{
  "op": "subscribe",
  "args": ["orderbook.1.BTCUSDT"]
}
```

### 5.5 OrderBook L2 (50 levels)
```json
{
  "op": "subscribe",
  "args": ["orderbook.50.BTCUSDT"]
}
```

### 5.6 CVD (Cumulative Volume Delta)
Uses the same trades channel as 5.1 above

---

## Feature Support Summary

| Exchange | Trades | Liquidations | Open Interest | CVD | OrderBook L1 | OrderBook L2 |
|----------|--------|--------------|---------------|-----|--------------|--------------|
| OKX | ✅ | ✅ (type-level) | ✅ | ✅ | ❌ | ❌ |
| Binance Spot | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ |
| Binance Futures | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ |
| Bybit Spot | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ |
| Bybit Perpetuals | ✅ | ✅ (global) | ✅ | ✅ | ✅ | ✅ |

---

## Key Differences

### Liquidations
- **OKX**: Subscribe by instrument type (`SWAP`, `FUTURES`, etc.), receives all instruments of that type
- **Binance**: Subscribe per instrument (`btcusdt@forceOrder`)
- **Bybit**: Global feed only (`allLiquidation`), receives all instruments

### Instrument Naming
- **OKX**: Dash-separated uppercase (e.g., `BTC-USDT-SWAP`)
- **Binance**: Concatenated lowercase (e.g., `btcusdt`)
- **Bybit**: Concatenated uppercase (e.g., `BTCUSDT`)

### Channel Format
- **OKX**: Separate `channel` and `instId`/`instType` fields
- **Binance**: Combined format `market@channel` in params array
- **Bybit**: Combined format `channel.market` in args array

---

## Testing in Postman

1. **Create WebSocket Request:**
   - Click "New" → "WebSocket Request"
   - Enter WebSocket URL
   - Click "Connect"

2. **Send Subscription:**
   - Copy subscription JSON
   - Paste in message field
   - Click "Send"

3. **Observe Response:**
   - First message: Subscription confirmation
   - Following messages: Real-time data

4. **Keep Alive (if needed):**
   - OKX: Send `"ping"` every 29 seconds
   - Bybit: Send `{"op":"ping"}` every 5 seconds
   - Binance: No action needed

5. **Test Multiple Instruments:**
   - OKX/Binance: Change `instId`/market name
   - Bybit: Change symbol in args array
