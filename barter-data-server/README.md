# Barter Data Server

A WebSocket server that broadcasts real-time market data from multiple cryptocurrency exchanges using the barter-data library.

## Features

- **Real-time Market Data**: Streams live market data from OKX, Bybit, and Binance Futures
- **WebSocket Server**: Broadcasts events to multiple connected clients via WebSocket
- **Multiple Data Types**:
  - Liquidations
  - Open Interest
  - Cumulative Volume Delta (CVD)
  - Binance Open Interest (REST API polling fallback)

## Supported Exchanges

- **OKX**: Liquidations, Open Interest, CVD
- **Bybit Perpetuals**: Liquidations, Open Interest, CVD
- **Binance Futures USD**: Liquidations, CVD, Open Interest (via REST polling)

## Usage

### Running the Server

```bash
# From the workspace root
cargo run -p barter-data-server

# Or from the barter-data-server directory
cargo run
```

The server will start on `ws://127.0.0.1:9001`

### Connecting a Client

You can connect to the WebSocket server using any WebSocket client. For example, using `websocat`:

```bash
websocat ws://127.0.0.1:9001
```

Or using JavaScript in a browser:

```javascript
const ws = new WebSocket('ws://127.0.0.1:9001');

ws.onopen = () => {
    console.log('Connected to barter-data server');
};

ws.onmessage = (event) => {
    const data = JSON.parse(event.data);
    console.log('Market event:', data);
};

ws.onerror = (error) => {
    console.error('WebSocket error:', error);
};

ws.onclose = () => {
    console.log('Disconnected from server');
};
```

### Testing with Postman

1. Create a new WebSocket request in Postman
2. Enter URL: `ws://127.0.0.1:9001`
3. Click "Connect"
4. You will receive a welcome message followed by real-time market events

## Message Format

### Welcome Message

When you first connect, you'll receive a welcome message:

```json
{
  "type": "welcome",
  "message": "Connected to barter-data market feed",
  "timestamp": "2025-11-05T09:10:22.037873Z"
}
```

### Market Event Messages

All subsequent messages are market events in this format:

```json
{
  "time_exchange": "2025-11-05T09:10:22.037873Z",
  "time_received": "2025-11-05T09:10:22.037890Z",
  "exchange": "Okx",
  "instrument": {
    "base": "btc",
    "quote": "usdt",
    "kind": "Perpetual"
  },
  "kind": "liquidation",
  "data": {
    "side": "Buy",
    "price": 95000.0,
    "quantity": 0.5,
    "time": "2025-11-05T09:10:22.037873Z"
  }
}
```

### Event Types

- `"kind": "liquidation"` - Liquidation orders
- `"kind": "open_interest"` - Open interest updates
- `"kind": "cumulative_volume_delta"` - CVD updates
- `"kind": "trade"` - Public trades (if enabled)
- `"kind": "order_book_l1"` - Level 1 order book (if enabled)

## Configuration

To modify which exchanges and instruments are tracked, edit the `init_market_streams()` function in `src/main.rs`:

```rust
DynamicStreams::init([
    vec![
        (BybitPerpetualsUsd, "btc", "usdt", Perpetual, OpenInterest),
        (BybitPerpetualsUsd, "eth", "usdt", Perpetual, OpenInterest),
    ],
    // Add more subscription batches here...
])
```

To change the server address, modify the `server_addr` in `main()`:

```rust
let server_addr = "127.0.0.1:9001".parse::<SocketAddr>().unwrap();
```

## Environment Variables

Set `RUST_LOG` to control logging level:

```bash
# Info level (default)
RUST_LOG=info cargo run -p barter-data-server

# Debug level (more verbose)
RUST_LOG=debug cargo run -p barter-data-server

# Trace level (very verbose)
RUST_LOG=trace cargo run -p barter-data-server
```

## Architecture

The server uses:
- **tokio-tungstenite** for WebSocket server functionality
- **tokio broadcast channels** to fan out market events to multiple clients
- **barter-data** for exchange integrations and market data streaming
- **Futures streams** to combine WebSocket and REST API data sources

```
┌─────────────────┐
│  OKX Exchange   │
└────────┬────────┘
         │
         │  WebSocket
         │
┌────────▼────────┐     ┌──────────────────┐
│ Market Streams  │────▶│ Broadcast Channel│
└────────┬────────┘     └────────┬─────────┘
         │                       │
┌────────▼────────┐              │
│ Bybit Exchange  ���              │
└────────┬────────┘     ┌────────▼─────────┐
         │              │  WebSocket Server│
         │              │  (port 9001)     │
┌────────▼────────┐     └────────┬─────────┘
│Binance Exchange │              │
└─────────────────┘              │
                        ┌────────▼─────────┐
                        │  WS Clients      │
                        │  (browsers, etc) │
                        └──────────────────┘
```

## Performance Notes

- The server uses a broadcast channel with capacity of 1000 messages
- If a slow client can't keep up, it will be disconnected to prevent memory issues
- CVD events are not throttled at the server level (throttling happens in the example client)
- Binance open interest is polled every 10 seconds via REST API

## License

MIT
