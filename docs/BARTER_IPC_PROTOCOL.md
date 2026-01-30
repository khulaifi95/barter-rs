# Barter IPC Protocol Documentation

**Version:** 1.0
**Last Updated:** 2026-01-29
**Status:** Verified

---

## Overview

This document describes the IPC (Inter-Process Communication) protocol used by `barter-data-server` for streaming market data to external consumers. This protocol enables real-time data streaming to TUIs, monitoring tools, and potentially Nautilus Trader for sandbox/validation modes.

## Transport Options

| Transport | Config Variable | Default | Use Case |
|-----------|-----------------|---------|----------|
| **UDS** (Unix Domain Socket) | `UDS_ENABLED`, `UDS_PATH` | `/tmp/barter-data.sock` | Low-latency same-host communication |
| **TCP** | `TCP_ENABLED`, `TCP_ADDR` | `127.0.0.1:9002` | Remote or cross-platform communication |

## Message Format

### Frame Structure

```
┌────────────────┬──────────────────────────────────┐
│ Length (4B BE) │ MessagePack Payload              │
├────────────────┼──────────────────────────────────┤
│ 00 00 01 2D    │ <237 bytes of MessagePack data>  │
└────────────────┴──────────────────────────────────┘
```

- **Length Prefix**: 4-byte big-endian unsigned integer (payload size)
- **Payload**: MessagePack-encoded message

### Message Types

Currently, all messages are wrapped in an `Event` envelope:

```rust
enum UdsMessageRef<'a> {
    Event(&'a MarketEventMessage),
}
```

### MarketEventMessage Schema

```json
{
  "Event": {
    "time_exchange": "2026-01-29T15:30:00.123456Z",   // ISO 8601 timestamp
    "time_received": "2026-01-29T15:30:00.124000Z",   // ISO 8601 timestamp
    "exchange": "BinanceFuturesUsd",                  // Exchange identifier
    "instrument": {
      "base": "btc",                                  // Base currency
      "quote": "usdt",                                // Quote currency
      "kind": "perpetual"                             // Contract type
    },
    "kind": "trade",                                  // Event type (see below)
    "data": { ... }                                   // Event-specific payload
  }
}
```

### Event Types (`kind`)

| Kind | Description | Data Fields |
|------|-------------|-------------|
| `trade` | Trade execution | `id`, `price`, `amount`, `side` |
| `liquidation` | Forced liquidation | `side`, `price`, `quantity`, `time` |
| `open_interest` | OI snapshot | `contracts`, `notional?`, `time?` |
| `funding_rate` | Funding rate | `rate`, `time?`, `next_time?` |
| `cumulative_volume_delta` | CVD update | `delta_base`, `delta_quote` |
| `order_book_l1` | Top of book | `best_bid`, `best_ask`, `last_update_time` |
| `order_book_l2` | Depth snapshot/update | `Snapshot`/`Update` with `bids`, `asks` |

### Example: Trade Event

```json
{
  "Event": {
    "time_exchange": "2026-01-29T15:30:00.123456Z",
    "time_received": "2026-01-29T15:30:00.124000Z",
    "exchange": "BinanceFuturesUsd",
    "instrument": {
      "base": "btc",
      "quote": "usdt",
      "kind": "perpetual"
    },
    "kind": "trade",
    "data": {
      "id": "1234567890",
      "price": 100000.50,
      "amount": 0.001,
      "side": "Buy"
    }
  }
}
```

## Python Client Example

```python
import socket
import struct
import msgpack

def connect_and_receive(host="127.0.0.1", port=9002, max_messages=10):
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.connect((host, port))

    buffer = b""
    messages = 0

    while messages < max_messages:
        data = sock.recv(4096)
        if not data:
            break

        buffer += data

        # Process complete messages
        while len(buffer) >= 4:
            length = struct.unpack(">I", buffer[:4])[0]
            if len(buffer) < 4 + length:
                break

            payload = buffer[4:4+length]
            buffer = buffer[4+length:]

            decoded = msgpack.unpackb(payload, raw=False)
            if "Event" in decoded:
                event = decoded["Event"]
                print(f"{event['kind']}: {event['exchange']}/{event['instrument']['base']}")

            messages += 1

    sock.close()
```

## Nautilus Adapter Requirements

To create a Nautilus adapter for Barter, the following would be needed:

### 1. Data Client

```python
class BarterDataClient(LiveMarketDataClient):
    """Receives market data from Barter via UDS/TCP."""

    def connect(self):
        # Connect to barter-data-server UDS/TCP socket
        pass

    def _handle_message(self, payload: bytes):
        # Decode MessagePack
        # Convert MarketEventMessage to Nautilus types
        # - Trade -> TradeTick
        # - OrderBookL1 -> QuoteTick
        # - OrderBookL2 -> OrderBookDelta/OrderBook
        pass
```

### 2. Type Conversions

| Barter Type | Nautilus Type |
|-------------|---------------|
| `trade` | `TradeTick` |
| `order_book_l1` | `QuoteTick` |
| `order_book_l2` | `OrderBookDelta` / `OrderBook` |
| `liquidation` | Custom / ignore |
| `funding_rate` | Custom / ignore |

### 3. Instrument Mapping

Barter instrument format: `{base}/{quote}/{kind}`
Nautilus instrument format: `{SYMBOL}.{VENUE}`

Example: `btc/usdt/perpetual` → `BTCUSDT-PERP.BINANCE`

## Verification

Run the IPC protocol test:

```bash
# Test MessagePack decoding (no server required)
python scripts/validation/test_barter_ipc.py --test-decode

# Test with running server
python scripts/validation/test_barter_ipc.py --tcp 127.0.0.1:9002
python scripts/validation/test_barter_ipc.py --uds /tmp/barter-data.sock
```

## Configuration

### Environment Variables

```bash
# UDS Configuration
UDS_ENABLED=true
UDS_PATH=/tmp/barter-data.sock
UDS_BUFFER=10000

# TCP Configuration
TCP_ENABLED=true
TCP_ADDR=127.0.0.1:9002
TCP_BUFFER=10000
```

---

## Status

- ✅ Protocol documented
- ✅ MessagePack decoding verified (Python)
- ✅ Test script created
- ⏳ Nautilus adapter (future work, not blocking for Parquet-based backtesting)

**Note:** Parquet-based backtesting (Phase 3) does NOT require the IPC adapter. The adapter is only needed for optional live sandbox/validation modes where Nautilus receives real-time signals from Barter.
