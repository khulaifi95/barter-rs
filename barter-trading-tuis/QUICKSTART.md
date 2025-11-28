# Quick Start Guide - Market Microstructure Dashboard

## 🚀 Fast Track

### 1. Prerequisites

Make sure `barter-data-server` is running:
```bash
# In a separate terminal
cd barter-data-server
cargo run --release
```

It should output something like:
```
WebSocket server listening on ws://0.0.0.0:9001
```

### 2. Build & Run

```bash
# From barter-trading-tuis directory
cargo run --release --bin market-microstructure
```

### 3. Controls

- Press `q` to quit

### 4. What You'll See

Six professional panels updating every 250ms:

```
┌─ ORDERFLOW IMBALANCE (1m) ────────────┬─ SPOT vs PERP BASIS ─────┐
│ Progress bars showing buy/sell flow   │ Basis calculation        │
├─ LIQUIDATION CLUSTERS ────────────────┼─ FUNDING MOMENTUM ───────┤
│ Price levels with cascade risk        │ Funding rate trends      │
├─ WHALE DETECTOR (>$500K) ─────────────┼─ CVD DIVERGENCE ─────────┤
│ Large trades in real-time             │ Smart money signals      │
└────────────────────────────────────────┴──────────────────────────┘
```

---

## 📊 Understanding the Panels

### Panel 1: Orderflow Imbalance
**What it shows:** Buy vs sell pressure over the last minute

**How to read:**
- `[████████░░] 73% BUY` → Strong buying pressure
- `[███░░░░░░░] 31% BUY` → Strong selling pressure
- `Δ +$2.3M/min ↑↑` → Net inflow with strong trend

**Trading signals:**
- >70% BUY + ↑↑ → Strong bullish momentum
- <30% BUY + ↓↓ → Strong bearish momentum
- ~50% + → → Consolidation/ranging

### Panel 2: Spot vs Perp Basis
**What it shows:** Price difference between spot and perpetual contracts

**How to read:**
- `+$38 (0.04%) CONTANGO` → Perps trading above spot (normal)
- `-$12 (-0.32%) BACKWRD` → Perps below spot (bearish sentiment)
- `STEEP` → Basis >0.5% (extreme positioning)

**Trading signals:**
- STEEP CONTANGO → Overleveraged longs, potential correction
- BACKWARDATION → Fear/hedging, potential reversal

*Note: Currently estimated (needs spot data feed)*

### Panel 3: Liquidation Clusters
**What it shows:** Price levels where liquidations are concentrated

**How to read:**
- `$94.5K ██████ (127 L, 45 S)` → Large cluster of long liquidations
- `DANGER ZONE` → High cascade risk (>$1M at level)

**Trading signals:**
- Large cluster above price → Resistance, potential cascade down
- Large cluster below price → Support, potential cascade up
- DANGER ZONE → Stay away or hedge carefully

### Panel 4: Funding Momentum
**What it shows:** Funding rate trends across exchanges

**Expected format:**
- `0.012% ↑↑ LONGS PAY` → Positive funding, longs pay shorts
- `-0.008% ↓ SHORTS PAY` → Negative funding, shorts pay longs
- `0.045% ↑↑↑ EXTREME` → Extreme funding (>0.04%)

**Trading signals:**
- ↑↑↑ EXTREME → Overleveraged, expect reversion
- LONGS PAY + rising → Consider short positioning
- SHORTS PAY + falling → Consider long positioning

*Note: Requires funding rate data feed*

### Panel 5: Whale Detector
**What it shows:** Trades >$500K in real-time

**How to read:**
- `10:32:15 BTC SELL $2.3M @$95.8K [BNC]` → Large sell
- `⚠️` → Mega whale (>$5M)
- `GREEN` text → Buy / `RED` text → Sell

**Trading signals:**
- Multiple buys in short time → Strong accumulation
- Multiple sells → Distribution, potential top
- ⚠️ mega trades → Major player action, pay attention

### Panel 6: CVD Divergence
**What it shows:** Comparison of price vs cumulative volume delta

**How to read:**
- `Price ↓ CVD ↑ BULLISH` → Price down but accumulation (hidden strength)
- `Price ↑ CVD ↓ BEARISH` → Price up but distribution (hidden weakness)
- `Price ≈ CVD ALIGNED` → Healthy trend

**Trading signals:**
- BULLISH divergence → Potential reversal up
- BEARISH divergence → Potential reversal down
- ALIGNED → Trust the trend

---

## 🎯 Trading Workflow Examples

### Scalping Setup
1. Watch **Orderflow Imbalance** for 1-min momentum
2. Check **CVD Divergence** for confirmation
3. Monitor **Whale Detector** for large orders
4. Enter when all align

### Position Sizing
1. Check **Liquidation Clusters** for risk levels
2. Review **Funding Momentum** for positioning
3. Use **Basis** for market sentiment
4. Size accordingly

### Risk Management
1. **Liquidation Clusters** → Set stops away from clusters
2. **Whale Detector** → Watch for distribution
3. **CVD Divergence** → Exit on bearish divergence
4. **Funding** → Reduce leverage if extreme

---

## 🔧 Troubleshooting

### "Starting WebSocket client for ws://127.0.0.1:9001"
✅ Normal - connecting to server

### "Connected to WebSocket server"
✅ Good - receiving data

### "Failed to connect" (repeating)
❌ Problem:
1. Check if `barter-data-server` is running
2. Verify port 9001 is not blocked
3. Check server logs for errors

### Panels show "Waiting for data..." or "Data not available"
⏳ Normal on startup:
- **Orderflow** - needs ~10 seconds of trades
- **Liquidations** - needs liquidation events
- **Whales** - needs >$500K trade
- **CVD** - needs ~30 seconds of data
- **Basis/Funding** - needs specific data feeds (not yet implemented)

### UI not updating
1. Check terminal size (minimum 80x24)
2. Verify WebSocket connection in logs
3. Restart the dashboard

---

## 💡 Tips

1. **Multi-ticker Analysis**
   - Compare BTC/ETH/SOL orderflow
   - Look for correlation or divergence
   - BTC often leads

2. **Time Sensitivity**
   - Orderflow: 1-minute window (very reactive)
   - Liquidations: 5-minute view (medium-term)
   - CVD: 60-second trend (short-term)

3. **Context Matters**
   - High volatility → Orderflow noise increases
   - Low liquidity → Whales have more impact
   - Weekend → Thinner orderbooks, more risk

4. **Combine Signals**
   - Don't trade on one panel alone
   - Wait for confluence (2-3 signals)
   - Use for confirmation, not prediction

---

## 📚 More Information

- **Full Documentation:** `README_MARKET_MICROSTRUCTURE.md`
- **Implementation Details:** `IMPLEMENTATION_SUMMARY.md`
- **Source Code:** `src/bin/market_microstructure.rs`

---

**Happy Trading! 📈**

*Remember: This is a tool for information, not financial advice. Always manage your risk.*
