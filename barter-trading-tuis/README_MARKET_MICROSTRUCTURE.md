# Market Microstructure Dashboard

Real-time orderflow and market activity monitoring for active trading decisions.

## Overview

The Market Microstructure Dashboard is an institutional-grade TUI that provides real-time insights into market microstructure, orderflow dynamics, and whale activity across BTC, ETH, and SOL.

**Refresh Rate:** 250ms
**Primary Use:** Active trading decisions

## Features

### 6 Professional Panels

1. **Orderflow Imbalance (1m window)**
   - Buy volume vs sell volume with visual progress bars
   - Imbalance percentage: Buy % vs Sell %
   - Net flow in $ per minute
   - Trend indicators: ↑↑ (strong buy) / ↑ / → / ↓ / ↓↓ (strong sell)

2. **Spot vs Perp Basis**
   - Basis: perp_price - spot_price
   - Basis percentage
   - Market state: CONTANGO / BACKWARDATION / STEEP
   - *Note: Currently estimated from spread (requires spot data feed)*

3. **Liquidation Clusters**
   - Groups liquidations by $100 price buckets
   - Shows concentration and cascade risk
   - DANGER ZONE alerts for high-risk levels
   - Long/Short split counts

4. **Funding Momentum**
   - Current funding rates with trend arrows
   - ↑↑↑ EXTREME alerts for >0.04%
   - LONGS PAY vs SHORTS PAY indicators
   - *Note: Requires funding rate data feed*

5. **Whale Detector (>$500K)**
   - Real-time feed of large trades
   - Shows: time, ticker, side, $ value, price, exchange
   - ⚠️ flag for mega trades >$5M
   - Color-coded by side (GREEN=buy, RED=sell)

6. **CVD Divergence**
   - Compares price direction vs CVD direction
   - BULLISH: Price ↓ but CVD ↑ (accumulation)
   - BEARISH: Price ↑ but CVD ↓ (distribution)
   - ALIGNED: Price and CVD same direction

## Installation

### Prerequisites

- Rust 1.70+
- Running `barter-data-server` on `ws://127.0.0.1:9001`

### Build

```bash
cd barter-trading-tuis
cargo build --release --bin market-microstructure
```

## Usage

### Start the Dashboard

```bash
# From the barter-trading-tuis directory
cargo run --release --bin market-microstructure

# Or run the binary directly
./target/release/market-microstructure
```

### Controls

- `q` - Quit the application

### Layout

```
┌─ ORDERFLOW IMBALANCE ──────────────────────────┬─ SPOT vs PERP BASIS ─────────┐
│ BTC  [████████░░] 73% BUY   Δ +$2.3M/min ↑     │ BTC  +$38 (0.04%) CONTANGO   │
│ ETH  [███░░░░░░░] 31% BUY   Δ -$1.1M/min ↓     │ ETH  -$12 (-0.32%) BACKWRD   │
│ SOL  [██████████] 92% BUY   Δ +$0.8M/min ↑↑    │ SOL  +$0.8 (0.52%) STEEP     │
├─ LIQUIDATION CLUSTERS ─────────────────────────┼─ FUNDING MOMENTUM ───────────┤
│ BTC Liquidations:                               │ Data not available           │
│ $94.5K ██████ (127 L, 45 S) DANGER ZONE        │                              │
│ $96.2K ███ (82 L, 23 S)                        │                              │
├─ WHALE DETECTOR (>$500K) ──────────────────────┼─ CVD DIVERGENCE ─────────────┤
│ 10:32:15 BTC SELL $2.3M @$95.8K [BNC]         │ BTC: Price ↑ CVD ↓ BEARISH   │
│ 10:31:44 ETH BUY  $1.8M @$3.2K  [OKX]         │ ETH: Price ↓ CVD ↑ BULLISH   │
│ 10:30:22 BTC BUY  $5.1M @$95.7K [BBT] ⚠️      │ SOL: Price ↑ CVD ↑ ALIGNED   │
└────────────────────────────────────────────────┴──────────────────────────────┘
```

## Color Coding

- **GREEN** - Buy pressure / Bullish signals
- **RED** - Sell pressure / Bearish signals / Danger zones
- **YELLOW** - Caution / Neutral / Warnings
- **MAGENTA** - Whale activity (>$500K trades)
- **CYAN** - Panel borders / Exchange names
- **BLUE** - Aligned signals

## Data Requirements

The dashboard requires a WebSocket connection to `barter-data-server` which aggregates data from:

- **Binance Futures** (BNC)
- **OKX** (OKX)
- **Bybit** (BBT)

### Required Event Types

- `trade` - For orderflow and whale detection
- `liquidation` - For cluster analysis
- `cumulative_volume_delta` - For CVD divergence
- `order_book_l1` - For spread and price data
- `open_interest` - (optional)

### Future Data Needs

- **Spot prices** - For accurate basis calculation
- **Funding rates** - For funding momentum panel

## Technical Details

### Performance

- **Refresh Rate:** 250ms (4 FPS)
- **Event Processing:** Async, non-blocking
- **Memory:** Windowed data (1-5 minute retention)
- **CPU:** Low overhead with efficient aggregation

### Data Windows

- **Orderflow:** 1-minute rolling window
- **Liquidations:** 5-minute retention
- **CVD/Price:** 60-second history for divergence
- **Whale Trades:** Last 10 trades displayed

### Thresholds

- **Whale Trade:** $500,000+ USD value
- **Mega Whale:** $5,000,000+ USD value (⚠️ flag)
- **Liquidation Bucket:** $100 price intervals
- **Danger Zone:** >$1M liquidation volume in bucket

## Architecture

```
┌─────────────────────────────────────┐
│  WebSocket Server (port 9001)       │
│  - Binance, OKX, Bybit aggregation  │
└─────────────┬───────────────────────┘
              │ Market Events
              ▼
┌─────────────────────────────────────┐
│  WebSocket Client                   │
│  - Auto-reconnect                   │
│  - Heartbeat (30s ping)             │
│  - Event deserialization            │
└─────────────┬───────────────────────┘
              │
              ▼
┌─────────────────────────────────────┐
│  Event Processing (Async)           │
│  - Per-ticker metrics               │
│  - Orderflow aggregation            │
│  - Liquidation clustering           │
│  - Whale detection                  │
│  - CVD divergence analysis          │
└─────────────┬───────────────────────┘
              │
              ▼
┌─────────────────────────────────────┐
│  Ratatui UI (250ms refresh)         │
│  - 6 professional panels            │
│  - Color-coded signals              │
│  - Progress bars & trend arrows     │
└─────────────────────────────────────┘
```

## Troubleshooting

### "Connection failed" or no data

1. Verify `barter-data-server` is running on port 9001
2. Check server logs for errors
3. Ensure firewall allows localhost connections

### UI not updating

1. Check WebSocket connection status (logs)
2. Verify server is sending events
3. Restart the dashboard

### Missing panels or "Data not available"

1. **Basis Panel:** Requires spot price data (not yet implemented)
2. **Funding Panel:** Requires funding rate events (not yet implemented)
3. Other panels need time to accumulate data (1-5 minutes)

## Future Enhancements

- [ ] Add spot price feed for accurate basis calculation
- [ ] Integrate funding rate data
- [ ] Add historical comparison (vs 1h/24h ago)
- [ ] Alert system for extreme conditions
- [ ] Export capability for trade journal
- [ ] Configurable thresholds
- [ ] Multi-timeframe analysis

## License

MIT

## Author

Built for barter-rs institutional trading infrastructure
