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

To change the server address, set `WS_ADDR`:

```bash
WS_ADDR=127.0.0.1:9001
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

Common configuration knobs (defaults shown):

```bash
# WebSocket server
WS_ADDR=127.0.0.1:9001
WS_BIND_STRICT=0           # 1 = panic on bind failure
WS_BIND_RETRY_MS=2000
WS_BIND_MAX_RETRIES=0      # 0 = infinite retries
WS_ENVELOPE=0
WS_BINARY_FRAMES=1
WS_SOURCE=barter-data-server
WS_AUTH_TOKEN=             # optional shared secret (x-auth-token or Authorization: Bearer)
WS_MAX_MESSAGE_BYTES=4194304
WS_MAX_FRAME_BYTES=1048576
WS_LOG_MAX_CHARS=256

# Stream filtering / robustness
STREAM_STRICT=0            # 1 = panic on stream init failure
OKX_CTVAL_STRICT=0         # 1 = disable OKX streams if ctVal fetch/coverage fails at startup
STREAM_ASSETS=BTC
STREAM_VENUES=BINANCE
STREAM_SPOT=0
STREAM_PERP=1
STREAM_TRADES=1
STREAM_L1=1
STREAM_L2=1
STREAM_OI=1
STREAM_LIQ=1
STREAM_CVD=1
STREAM_FUNDING=1

# Parquet output
PARQUET_ENABLED=0
PARQUET_OUTPUT_DIR=/data/parquet
PARQUET_FLUSH_INTERVAL_SECS=60
PARQUET_BUFFER=50000
PARQUET_WRITE_TRADES=1
PARQUET_WRITE_BARS=1
PARQUET_WRITE_EXTENDED=1
PARQUET_WRITE_L2=0
PARQUET_L2_MAX_DEPTH=50
PARQUET_L2_SAMPLE_MS=0    # 0 = no sampling (write every L2 update)
PARQUET_ASSETS=BTC
PARQUET_VENUES=BINANCE
PARQUET_INSTRUMENTS=
NAUTILUS_PRECISION=high    # high|standard

# Parquet durability / backpressure
PARQUET_TRADE_SEND_MODE=block      # block|drop
PARQUET_TRADE_SEND_TIMEOUT_MS=0    # 0 = wait forever
PARQUET_EVENT_RETRY_MAX=3
PARQUET_FSYNC=0                    # 1 = fsync each file write

# Bar timestamp mode
BAR_TS_EVENT_MODE=close     # close|open

# Throttling / buffers
L1_THROTTLE_MS=50
AGG_EVENT_BUFFER=300000
```

Notes:
- Defaults for `STREAM_ASSETS`, `STREAM_VENUES`, `PARQUET_ASSETS`, and `PARQUET_VENUES` only apply when the env vars are unset.
- Set an env var to an empty string (e.g. `STREAM_ASSETS=`) to disable that filter.
- Parquet files are written on the flush interval (`PARQUET_FLUSH_INTERVAL_SECS`) or when buffers fill, not per trade.
- Bar directories are keyed by bar type (e.g. `bars_1m/BTCUSDT_PERP_BINANCE_1_MINUTE_LAST_EXTERNAL/`).

## L2 Parquet Tradeoffs (OrderBookDeltas)

- **Accuracy vs storage**: L2 volume scales with update rate × depth × instruments.
  - Higher depth or no sampling = more rows and larger files.
- **Sampling controls**:
  - `PARQUET_L2_MAX_DEPTH` caps per-update levels written.
  - `PARQUET_L2_SAMPLE_MS` skips updates faster than this interval.
- **Latency measurement**:
  - Use `ts_init - ts_event` from the deltas to measure end-to-end latency.
  - Sampling adds up to the sample interval of extra delay for Parquet visibility.
- **Recommended defaults**:
  - Keep `PARQUET_WRITE_L2=0` for normal capture.
  - Enable L2 only for specific windows or order‑flow backtests.

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
