# Validation Steps for Legacy TUI Fixes

## Changes Made ✅

### 1. TUI Perp-Only Filters (`barter-data-tui/src/main.rs`)
- **OrderBook L1**: Now filters OUT spot entries (lines 350-354)
  - Only shows instruments with "perpetual" or "futures" in the key
  - Prevents panel crowding from 9+ instruments

- **Open Interest**: Only tracks perpetual/futures (lines 507-512)
  - Filters based on `instrument.kind` field

- **CVD**: Only tracks perpetual/futures (lines 518-523)
  - Same filter logic as OI

### 2. Server Debug Logging (`barter-data-server/src/main.rs`)
- **Added OI event logging** (lines 141-150):
  ```
  OI EVENT {exchange} {base}/{quote} contracts: {value} notional: {value}
  ```

- **Added OI broadcast logging** (lines 167-176):
  ```
  BROADCASTING open_interest to {N} clients: {exchange} {base}/{quote}
  ```

Both changes compile successfully ✅

---

## Validation Tests

### **Test 1: Restart Server with New Logging**

```bash
# Kill existing server
pkill -f barter-data-server

# Start with OI debug logging (in terminal 1)
cargo run --release -p barter-data-server 2>&1 | tee server_oi_debug.log
```

**What to look for** in the logs:
- ✅ `OI EVENT BinanceFuturesUsd btc/usdt contracts: XXXXX`  (every 10 seconds)
- ✅ `OI EVENT BinanceFuturesUsd eth/usdt contracts: XXXXX`  (every 10 seconds)
- ✅ `OI EVENT BybitPerpetualsUsd btc/usdt contracts: XXXXX` (real-time)
- ✅ `OI EVENT Okx btc/usdt contracts: XXXXX` (real-time)
- ✅ `BROADCASTING open_interest to N clients: ...`

**If NO OI events appear:**
- ❌ Binance REST poller is failing silently
- ❌ Bybit/OKX WebSocket streams not delivering OI
- Check for errors: `grep -i "open.*interest\|error" server_oi_debug.log`

---

### **Test 2: Run Legacy TUI and Check OrderBook L1**

```bash
# In terminal 2
cargo run --release -p barter-data-tui
```

**What to expect:**

#### **OrderBook L1 Panel (top-right):**
Should show **3-6 perpetual instruments** (not 9+):
```
📊 ORDERBOOK L1
 BinanceFuturesUsd-btc/usdt
   Bid: $84,500.30  qty: 4.5
   Ask: $84,500.40  qty: 3.8
   Spread: $0.10  0.001%

 BybitPerpetualsUsd-btc/usdt
   Bid: $84,501.20  qty: 2.1
   Ask: $84,501.30  qty: 5.3
   Spread: $0.10  0.001%
```

**✅ Expected**: Bid/ask/spread rows visible (NOT just instrument names)
**❌ Before fix**: Only instrument names, no prices (too crowded)

---

### **Test 3: Verify OI Panel Populates and PERSISTS**

**What to expect:**

#### **Open Interest Panel (middle-right):**
```
📊 OPEN INTEREST
 BinanceFuturesUsd-btc/usdt
   Value: 98776  — 0.00%
 BinanceFuturesUsd-eth/usdt
   Value: 1869450  ↑ 0.06%
 Okx-btc/usdt
   Value: 2654363  ↓ -0.00%
```

**✅ Expected**: Values appear AND stay visible (update every 10s)
**❌ Before**: Values appeared briefly then disappeared

**Wait 30 seconds** - values should update, not disappear.

---

### **Test 4: WebSocket Traffic Validation**

```bash
# Run while TUI is running
./validate_oi_events.sh
```

**Expected output:**
```
Message type counts:
  71 "kind":"order_book_l1"
  17 "kind":"trade"
  11 "kind":"cumulative_volume_delta"
   6 "kind":"open_interest"        <-- SHOULD NOW APPEAR!
```

**✅ If `open_interest` count > 0**: Server is broadcasting correctly
**❌ If count = 0**: Server issue - check `server_oi_debug.log`

---

## Debugging Guide

### If OI Panel Still Empty:

1. **Check server logs** for OI events:
   ```bash
   grep "OI EVENT" server_oi_debug.log | head -10
   ```

2. **Check TUI is filtering correctly**:
   - OI panel should only show keys like:
     - `BinanceFuturesUsd-*` ✅
     - `BybitPerpetualsUsd-*` ✅
     - `Okx-*` (Perpetual) ✅
   - NOT keys like:
     - `BinanceSpot-*` ❌
     - `BybitSpot-*` ❌

3. **Check instrument.kind format**:
   ```bash
   # Capture an OI message and check the "kind" field
   wscat -c ws://127.0.0.1:9001 2>&1 | grep "open_interest" | head -1 | python3 -m json.tool
   ```

   Should show:
   ```json
   {
     "instrument": {
       "kind": "Perpetual"  <-- Must contain "perpetual" or "future"
     }
   }
   ```

### If OrderBook L1 Still Shows No Prices:

1. **Check how many instruments**:
   - Count rows in OrderBook L1 panel
   - Should be ≤ 6 (not 9+)

2. **Check panel height**:
   - Resize terminal to be taller
   - Each instrument needs 4 lines (name + bid + ask + spread)

---

## Success Criteria

### ✅ All Tests Pass If:

1. **Server logs show**:
   - `OI EVENT` messages every 10 seconds (Binance)
   - `OI EVENT` messages real-time (Bybit, OKX)
   - `BROADCASTING open_interest to N clients`

2. **TUI OrderBook L1 panel shows**:
   - 3-6 perpetual instruments (not 9+)
   - Bid/ask/spread rows for each (not just names)

3. **TUI Open Interest panel shows**:
   - Values that PERSIST (don't disappear)
   - Updates every 10 seconds
   - No empty panel after 30 seconds

4. **WebSocket validation shows**:
   - `"kind":"open_interest"` messages captured
   - Count ≥ 4 per 15-second sample (at least 1 from Binance REST)

---

## Next Steps If Tests Fail

### If NO OI events in server logs:
→ **Root cause**: Binance REST poller or Bybit/OKX WebSocket streams failing
→ **Action**: Add more detailed error logging to `binance_open_interest_poller` function

### If OI events in logs but NOT broadcast:
→ **Root cause**: Event processing/serialization issue
→ **Action**: Check if `DataKind::OpenInterest` matches correctly

### If OI broadcast but TUI doesn't show:
→ **Root cause**: TUI filter logic rejecting valid perpetuals
→ **Action**: Check `instrument.kind.to_lowercase().contains("perpetual")` logic
