# Barter Data TUI

A beautiful Terminal User Interface (TUI) for visualizing real-time cryptocurrency market data from the barter-data-server.

![TUI Preview](https://via.placeholder.com/800x400?text=Barter+Data+TUI)

## Features

- 📊 **Real-time Liquidations Feed**: Rolling list of liquidation events with color coding
- 📈 **Open Interest Tracking**: Live statistics with trend indicators and sparklines
- 💹 **CVD Analysis**: Cumulative Volume Delta with buy/sell pressure metrics
- 🔄 **Auto-Reconnect**: Automatically reconnects to the server if connection is lost
- 🎨 **Color-Coded UI**: Easy-to-read color scheme for different event types

## Prerequisites

You need to have the `barter-data-server` running first:

```bash
# In one terminal, start the data server
cargo run -p barter-data-server
```

## Usage

```bash
# In another terminal, start the TUI
cargo run -p barter-data-tui

# Or from the barter-data-tui directory
cargo run
```

**Controls:**
- Press `q` or `Esc` to quit

## UI Layout

The TUI features a modern, clean 4-panel layout:

```
╔═══════════════════════════════════════════════════════════╗
║  ● CONNECTED  ⏱ 09:10:22.123  ◆ BARTER DATA TERMINAL ◆  ║
╠═══════════════════════════════╦═══════════════════════════╣
║                               ║                           ║
║  ⚡ LIQUIDATIONS FEED (45)    ║  📊 OPEN INTEREST         ║
║  ╭───────────────────────────╮║  ╭───────────────────────╮║
║  │ 09:10:22 ▼ [  Okx   ]    │║  │ Okx-btc/usdt          │║
║  │ btc/usdt  $95,000.00      │║  │ Value:   2,328,014 ↑  │║
║  │ Qty:0.5000                │║  │         +0.12%        │║
║  ├───────────────────────────┤║  │ ▁▂▃▄▅▆▇█              │║
║  │ 09:10:21 ▲ [Binance ]    │║  ├───────────────────────┤║
║  │ btc/usdt  $94,950.00      │║  │ Bybit-btc/usdt        │║
║  │ Qty:1.2000                │║  │ Value:   1,234,567 ↓  │║
║  ├───────────────────────────┤║  │         -0.05%        │║
║  │        ...more...         │║  │ ▇▆▅▄▃▂▁               │║
║  ╰───────────────────────────╯║  ╰───────────────────────╯║
║                               ║                           ║
║                               ║  💹 CUMULATIVE VOL DELTA  ║
║                               ║  ╭───────────────────────╮║
║                               ║  │ Okx-btc/usdt          │║
║                               ║  │ Δ Base:    -144.0600  │║
║                               ║  │ Δ Quote: -14,672,028  │║
║                               ║  │ ████░░░░░ 35.2%       │║
║                               ║  │ ▁▂▃▄▅▆▇█              │║
║                               ║  ╰───────────────────────╯║
╚═══════════════════════════════╩═══════════════════════════╝
```

**Color Scheme:**
- Purple double-line borders on status bar
- Red rounded borders on liquidations
- Blue rounded borders on open interest
- Pink rounded borders on CVD
- Dark backgrounds with bright text and accents

## Panels Explained

### 1. Status Bar (Top)
- **Connection Status**: Shows whether connected to the server (green) or disconnected (red)
- **Last Update**: Timestamp of the most recent market event
- **Help**: Keyboard shortcuts

### 2. Liquidations Panel (Bottom Left)
A real-time rolling feed of liquidation events:
- **Color Coding**:
  - 🟢 Green: Buy liquidations
  - 🔴 Red: Sell liquidations
- **Information**:
  - Timestamp (HH:MM:SS)
  - Exchange (OKX, Binance, Bybit)
  - Trading pair (BTC/USDT, ETH/USDT)
  - Side (BUY/SELL)
  - Liquidation price
  - Quantity liquidated
- **Capacity**: Shows the last 100 liquidations

### 3. Open Interest Panel (Top Right)
Statistics and trends for open interest across exchanges:
- **Current Value**: Current open interest in contracts
- **Trend Indicator**:
  - ↑ Increasing
  - ↓ Decreasing
  - — Stable
- **Change %**: Percentage change from previous value
- **Sparkline**: Visual trend chart of historical data (60 data points)

### 4. CVD Panel (Bottom Right)
Cumulative Volume Delta analysis:
- **Delta Base**: Net cumulative volume in base currency (positive = more buying, negative = more selling)
- **Delta Quote**: Net cumulative volume in quote currency
- **Buy Pressure %**:
  - >60% (Green): Strong buying pressure
  - 40-60% (Yellow): Balanced
  - <40% (Red): Strong selling pressure
- **Sparkline**: Visual representation of CVD trend

## Statistics & Indicators

### Open Interest Indicators
- **Trend Detection**: Automatically detects increasing/decreasing trends
- **Percentage Change**: Shows rate of change for quick assessment
- **Historical Tracking**: Maintains 60 data points for trend analysis

### CVD Indicators
- **Buy Pressure**: Calculated as `(delta + |delta|) / (2 * |delta|) * 100%`
  - Shows the proportion of buying vs selling volume
  - Values above 50% indicate net buying
  - Values below 50% indicate net selling
- **Visual Trend**: Sparkline shows absolute volume changes over time

## Data Sources

The TUI receives data from three exchanges:
- **OKX**: Liquidations, Open Interest, CVD
- **Bybit Perpetuals**: Liquidations, Open Interest, CVD
- **Binance Futures**: Liquidations, CVD, Open Interest (REST polling)

All data is for BTC/USDT perpetual contracts.

## Technical Details

### Architecture

```
┌──────────────────────┐
│ barter-data-server   │
│ (WebSocket Server)   │
└──────────┬───────────┘
           │ ws://127.0.0.1:9001
           │
┌──────────▼───────────┐
│ WebSocket Client     │
│ (async connection)   │
└──────────┬───────────┘
           │
┌──────────▼───────────┐
│ Event Processing     │
│ (parse & classify)   │
└──────────┬───────────┘
           │
┌──────────▼───────────┐
│ Application State    │
│ (Arc<Mutex<...>>)    │
└──────────┬───────────┘
           │
┌──────────▼───────────┐
│ Ratatui Rendering    │
│ (4 panel layout)     │
└──────────────────────┘
```

### Technologies
- **ratatui**: Terminal UI framework
- **crossterm**: Cross-platform terminal manipulation
- **tokio-tungstenite**: Async WebSocket client
- **tokio**: Async runtime
- **serde**: JSON serialization/deserialization

### Performance
- **Update Rate**: 250ms refresh rate
- **History Capacity**:
  - Liquidations: 100 events
  - Open Interest: 60 data points per instrument
  - CVD: 60 data points per instrument
- **Memory**: Minimal footprint with bounded data structures

## Customization

To modify the data sources, edit the server at `barter-data-server/src/main.rs` to add/remove exchanges and instruments.

To change UI colors or layout, edit `barter-data-tui/src/main.rs`:
- Colors are defined in the `render_*` functions
- Layout constraints are in the `ui` function
- Panel sizes can be adjusted with `Constraint` values

## Troubleshooting

### Connection Issues

**Problem**: TUI shows "DISCONNECTED"

**Solutions**:
1. Make sure the server is running: `cargo run -p barter-data-server`
2. Check the server is on port 9001: `netstat -an | grep 9001`
3. Check logs for error messages

### No Data Appearing

**Problem**: TUI connected but no events showing

**Solutions**:
1. Wait a few seconds - some events (like liquidations) are sporadic
2. Check the server logs to ensure it's receiving data from exchanges
3. Ensure the exchanges are not blocking connections

### Display Issues

**Problem**: UI looks corrupted or doesn't render properly

**Solutions**:
1. Resize your terminal window
2. Ensure your terminal supports 256 colors
3. Try a different terminal emulator
4. Press `q` to quit and restart

## Development

### Adding New Indicators

To add a new indicator:

1. Add a new field to `AppState`:
```rust
struct AppState {
    // ... existing fields
    my_indicator: HashMap<String, MyIndicatorStats>,
}
```

2. Create a stats struct:
```rust
struct MyIndicatorStats {
    value: f64,
    history: VecDeque<f64>,
    // ... other fields
}
```

3. Add processing in `process_event`:
```rust
"my_indicator" => {
    if let Ok(data) = serde_json::from_value::<MyData>(event.data) {
        s.update_my_indicator(key, data);
    }
}
```

4. Add rendering function:
```rust
fn render_my_indicator(f: &mut Frame, area: Rect, state: &AppState) {
    // ... rendering logic
}
```

5. Update UI layout in `ui` function

### Running in Development

```bash
# With debug logging
RUST_LOG=debug cargo run -p barter-data-tui

# Build release version for better performance
cargo build --release -p barter-data-tui
./target/release/barter-data-tui
```

## License

MIT

## Credits

Built with:
- [ratatui](https://github.com/ratatui-org/ratatui) - Terminal UI framework
- [barter-data](https://github.com/barter-rs/barter-rs) - Market data streaming library
