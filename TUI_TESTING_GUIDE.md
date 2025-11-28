# TUI Testing Guide

## Prerequisites

1. **Server must be running**:
   ```bash
   # Check if server is running
   ps aux | grep barter-data-server | grep -v grep

   # If not running, start it:
   cargo run --release -p barter-data-server > server.log 2>&1 &
   ```

2. **Build all TUIs** (if not already built):
   ```bash
   cargo build --release -p barter-trading-tuis
   ```

---

## TUI #1: Market Microstructure Dashboard

**What it shows:**
- Orderflow imbalance (1m window)
- Spot vs Perp basis
- Liquidation clusters
- Whale detector (configurable threshold)
- CVD divergence signals

**Run it:**
```bash
./run_tui.sh
# OR with custom whale threshold:
WHALE_THRESHOLD=10000 ./run_tui.sh
```

**Expected data:**
- Whale trades from OKX-SPOT, OKX-PERP, BBT-SPOT, BBT-PERP
- Basis showing CONTANGO/BACKWARDATION
- CVD values updating in real-time
- Liquidation clusters (if any)

---

## TUI #2: Institutional Flow Monitor

**What it shows:**
- **Net Flow (5m)**: Buy/sell pressure over 5 minutes
- **Aggressor Ratio (1m)**: Buy vs sell aggression percentage
- **Exchange Dominance**: Volume % by exchange over last 60s
- **Orderbook Depth Imbalance**: L1 bid/ask ratio (proxy)
- **Momentum Signals**: VWAP deviation, tick direction, trade speed

**Run it:**
```bash
./run_tui2.sh
```

**Expected data:**
- Net flow showing positive/negative values with arrows (↑↑, ↑, →, ↓, ↓↓)
- Aggressor showing BUY/SELL percentages and ratio
- Exchange dominance bars for all active exchanges
- Orderbook showing bid/ask imbalance
- VWAP deviation for BTC, ETH, SOL
- Tick direction counts

**Environment variables:**
```bash
# Customize thresholds
STRONG_FLOW_THRESHOLD=2000000 \
WEAK_FLOW_THRESHOLD=200000 \
./run_tui2.sh
```

---

## TUI #3: Risk & Arbitrage Scanner

**What it shows:**
- **Liquidation Cascade Risk**: Risk score (LOW/MEDIUM/HIGH)
- **Next Cascade Level**: Price and volume of next liquidation zone
- **Protection Level**: Support level below price
- **Arbitrage & Basis**: Spot vs perp pricing differences
- **Market Regime**: TRENDING/DOWNTREND/RANGING
- **Correlation Matrix**: BTC/ETH/SOL correlations

**Run it:**
```bash
./run_tui3.sh
```

**Expected data:**
- Risk score bar (green/yellow/red)
- Next liquidation level with price distance %
- Basis for BTC, ETH, SOL (CONTANGO/BACKWRD/NEUTRAL)
- Market regime based on tick direction
- CVD totals and velocity
- Correlation matrix with color-coded values

**Update rate:** Every 5 seconds (slower refresh for risk analysis)

---

## Running Multiple TUIs Simultaneously

### Option 1: Multiple Terminal Windows
```bash
# Terminal 1
./run_tui.sh

# Terminal 2
./run_tui2.sh

# Terminal 3
./run_tui3.sh
```

### Option 2: Using tmux
```bash
# Create 3-pane layout
tmux new-session \; \
  split-window -h \; \
  split-window -v \; \
  select-pane -t 0 \; \
  send-keys './run_tui.sh' C-m \; \
  select-pane -t 1 \; \
  send-keys './run_tui2.sh' C-m \; \
  select-pane -t 2 \; \
  send-keys './run_tui3.sh' C-m
```

---

## Troubleshooting

### TUI shows "Waiting for data..." or "Disconnected"
```bash
# Check server is running
ps aux | grep barter-data-server

# Check server logs for errors
tail -50 server.log

# Restart server
pkill barter-data-server
cargo run --release -p barter-data-server > server.log 2>&1 &
sleep 5  # Wait for server to start
```

### Terminal gets corrupted
```bash
# Reset terminal
./reset_terminal.sh
# OR manually:
reset
stty sane
tput rmcup
tput cnorm
```

### TUI won't respond to 'q' or Ctrl+C
```bash
# Force kill
pkill -9 market-microstructure
pkill -9 institutional-flow
pkill -9 risk-scanner

# Reset terminal
./reset_terminal.sh
```

### Check debug logs
```bash
# TUI 1
tail -f tui_clean.log

# TUI 2
tail -f tui2_debug.log

# TUI 3
tail -f tui3_debug.log

# Whale detection
tail -f whale_debug.log
```

---

## Data Validation Checklist

### ✅ TUI #1 (Market Microstructure)
- [ ] Orderflow bars showing for BTC/ETH/SOL
- [ ] Basis panel showing spot/perp spread
- [ ] Whale trades appearing (check exchange labels: OKX-SPOT, BBT-PERP, etc.)
- [ ] CVD values updating
- [ ] Connection status shows "CONNECTED" (if visible)

### ✅ TUI #2 (Institutional Flow)
- [ ] Net flow values changing
- [ ] Aggressor ratio showing buy/sell %
- [ ] Exchange dominance bars present
- [ ] Orderbook showing bid/ask values
- [ ] VWAP deviation showing for all tickers
- [ ] Tick direction counts updating
- [ ] Footer shows "CONNECTED"

### ✅ TUI #3 (Risk Scanner)
- [ ] Cascade risk bar visible
- [ ] Next cascade level showing price + distance %
- [ ] Basis showing for BTC/ETH/SOL
- [ ] Market regime displaying state
- [ ] CVD showing totals and velocity
- [ ] Correlation matrix populated
- [ ] Footer shows "CONNECTED"

---

## Performance Notes

- **TUI #1**: Updates every 250ms (4 FPS) - Real-time whale tracking
- **TUI #2**: Updates every 1 second - Flow analysis
- **TUI #3**: Updates every 5 seconds - Risk metrics

All TUIs connect to same server and share the same aggregation engine for consistency.

---

## Exit TUIs

Press **'q'** or **'ESC'** to exit cleanly.

All TUIs have panic handlers and trap scripts for automatic terminal cleanup!
