# Environment Variables Configuration

All barter TUIs support runtime configuration via environment variables. No code changes or recompilation required!

## 🎯 Quick Start Examples

### Default Configuration
```bash
cargo run --release --bin market-microstructure
```

### Custom Whale Threshold ($100K instead of default $50K)
```bash
WHALE_THRESHOLD=100000 cargo run --release --bin market-microstructure
```

### Monitor Different Assets
```bash
TICKERS=AVAX,ARB,MATIC cargo run --release --bin market-microstructure
```

### Connect to Remote Server
```bash
WS_URL=ws://prod-server.example.com:9001 cargo run --release --bin market-microstructure
```

### Combine Multiple Settings
```bash
WHALE_THRESHOLD=25000 \
MEGA_WHALE_THRESHOLD=2000000 \
TICKERS=BTC,ETH \
WS_URL=ws://127.0.0.1:9001 \
cargo run --release --bin market-microstructure
```

---

## 📋 Available Environment Variables

### Core Configuration

#### `WS_URL`
**What**: WebSocket server URL to connect to
**Default**: `ws://127.0.0.1:9001`
**Example**:
```bash
WS_URL=ws://192.168.1.100:9001 cargo run --release --bin market-microstructure
```

#### `TICKERS`
**What**: Comma-separated list of trading pairs to monitor
**Default**: `BTC,ETH,SOL`
**Example**:
```bash
# Monitor only Bitcoin and Ethereum
TICKERS=BTC,ETH cargo run --release --bin market-microstructure

# Monitor altcoins
TICKERS=AVAX,ARB,MATIC,OP cargo run --release --bin market-microstructure
```

---

### Whale Detection Thresholds

#### `WHALE_THRESHOLD`
**What**: Minimum trade size (USD) to be considered a "whale" trade
**Default**: `500000` ($500,000)
**Where**: Shared state module - affects all TUIs
**Example**:
```bash
# Lower threshold to see more trades
WHALE_THRESHOLD=100000 cargo run --release --bin market-microstructure

# Even lower for altcoins
WHALE_THRESHOLD=50000 cargo run --release --bin market-microstructure
```

#### `MEGA_WHALE_THRESHOLD`
**What**: Trade size that triggers the "⚠️" mega whale warning in display
**Default**: `5000000` ($5,000,000)
**Where**: TUI display layer only
**Example**:
```bash
# More sensitive mega whale alerts
MEGA_WHALE_THRESHOLD=2000000 cargo run --release --bin market-microstructure
```

#### `MAX_WHALES`
**What**: Maximum number of whale trades kept in memory buffer
**Default**: `500`
**Notes**: Increased from 50 to handle high-frequency perp whales while preserving spot whales
**Example**:
```bash
# Smaller buffer for memory-constrained systems
MAX_WHALES=200 cargo run --release --bin market-microstructure

# Larger buffer for high-volume markets
MAX_WHALES=1000 cargo run --release --bin market-microstructure
```

---

### Liquidation Thresholds

#### `LIQ_DANGER_THRESHOLD`
**What**: Liquidation cluster size (USD) to trigger cascade risk detection
**Default**: `1000000` ($1,000,000)
**Where**: Shared state module - cascade level calculation
**Example**:
```bash
# More sensitive cascade detection
LIQ_DANGER_THRESHOLD=500000 cargo run --release --bin market-microstructure
```

#### `LIQ_DISPLAY_DANGER_THRESHOLD`
**What**: Liquidation cluster size (USD) to show "DANGER" warning in TUI
**Default**: `1000000` ($1,000,000)
**Where**: TUI display layer only
**Example**:
```bash
# Show danger warnings for smaller clusters
LIQ_DISPLAY_DANGER_THRESHOLD=250000 cargo run --release --bin market-microstructure
```

---

### Institutional Flow Thresholds

#### `STRONG_FLOW_THRESHOLD`
**What**: Net flow (USD) to show "↑↑" (STRONG BUY) or "↓↓" (STRONG SELL)
**Default**: `1000000` ($1,000,000)
**Where**: Institutional Flow TUI only
**Example**:
```bash
# More sensitive flow signals
STRONG_FLOW_THRESHOLD=500000 cargo run --release --bin institutional-flow
```

#### `WEAK_FLOW_THRESHOLD`
**What**: Net flow (USD) to show "↑" (BULLISH) or "↓" (BEARISH)
**Default**: `100000` ($100,000)
**Where**: Institutional Flow TUI only
**Example**:
```bash
# Catch smaller flow signals
WEAK_FLOW_THRESHOLD=50000 cargo run --release --bin institutional-flow
```

---

### Server Configuration (barter-data-server)

#### `WS_ADDR`
**What**: WebSocket server bind address and port
**Default**: `0.0.0.0:9001`
**Example**:
```bash
# Bind to localhost only (more secure)
WS_ADDR=127.0.0.1:9001 cargo run --release -p barter-data-server

# Use different port
WS_ADDR=0.0.0.0:8080 cargo run --release -p barter-data-server
```

#### `WS_BUFFER_SIZE`
**What**: Broadcast channel buffer size for market events
**Default**: `10000`
**Notes**: Increase if clients are frequently lagging
**Example**:
```bash
# Larger buffer for high-throughput scenarios
WS_BUFFER_SIZE=50000 cargo run --release -p barter-data-server

# Smaller buffer to conserve memory
WS_BUFFER_SIZE=5000 cargo run --release -p barter-data-server
```

#### `SPOT_LOG_THRESHOLD`
**What**: Minimum spot trade size (USD) to log for debugging
**Default**: `50000` ($50,000)
**Notes**: Used for verifying spot data streams are working
**Example**:
```bash
# Log all large spot trades
SPOT_LOG_THRESHOLD=25000 cargo run --release -p barter-data-server

# Only log very large trades
SPOT_LOG_THRESHOLD=100000 cargo run --release -p barter-data-server
```

---

## 🧪 Testing Different Configurations

### Scenario 1: High-Volume Trading Environment
```bash
# Reduce noise by raising thresholds
WHALE_THRESHOLD=100000 \
MEGA_WHALE_THRESHOLD=10000000 \
MAX_WHALES=1000 \
cargo run --release --bin market-microstructure
```

### Scenario 2: Altcoin Monitoring (Lower Volumes)
```bash
# Lower thresholds to catch smaller trades
WHALE_THRESHOLD=10000 \
MEGA_WHALE_THRESHOLD=500000 \
TICKERS=AVAX,ARB,MATIC \
cargo run --release --bin market-microstructure
```

### Scenario 3: Risk-Focused Dashboard
```bash
# Focus on liquidation risks
LIQ_DANGER_THRESHOLD=250000 \
LIQ_DISPLAY_DANGER_THRESHOLD=250000 \
TICKERS=BTC,ETH \
cargo run --release --bin risk-scanner
```

### Scenario 4: Production Deployment
```bash
# Connect to production server with standard settings
WS_URL=ws://prod.internal:9001 \
TICKERS=BTC,ETH,SOL,AVAX,ARB \
WHALE_THRESHOLD=50000 \
cargo run --release --bin market-microstructure
```

---

## 🔧 Permanent Configuration

### Option 1: Shell Profile (.bashrc, .zshrc)
```bash
# Add to ~/.bashrc or ~/.zshrc
export WHALE_THRESHOLD=75000
export TICKERS=BTC,ETH,SOL,AVAX
export WS_URL=ws://127.0.0.1:9001
```

### Option 2: .env File (Using direnv)
```bash
# Install direnv: brew install direnv (macOS) or apt install direnv (Linux)
# Create .envrc in project root:
echo 'export WHALE_THRESHOLD=75000' >> .envrc
echo 'export TICKERS=BTC,ETH,SOL' >> .envrc
direnv allow
```

### Option 3: Launch Script
```bash
#!/bin/bash
# launch-tui.sh
export WHALE_THRESHOLD=75000
export TICKERS=BTC,ETH,SOL
export WS_URL=ws://127.0.0.1:9001
cargo run --release --bin market-microstructure
```

---

## 📊 Variable Reference Table

### TUI Client Variables
| Variable | Default | Type | Scope |
|----------|---------|------|-------|
| `WS_URL` | `ws://127.0.0.1:9001` | String | TUI |
| `TICKERS` | `BTC,ETH,SOL` | String (CSV) | TUI |
| `WHALE_THRESHOLD` | `500000` | Number | Shared State |
| `MEGA_WHALE_THRESHOLD` | `5000000` | Number | TUI Display |
| `MAX_WHALES` | `500` | Number | Shared State |
| `LIQ_DANGER_THRESHOLD` | `1000000` | Number | Shared State |
| `LIQ_DISPLAY_DANGER_THRESHOLD` | `1000000` | Number | TUI Display |
| `STRONG_FLOW_THRESHOLD` | `1000000` | Number | TUI Display |
| `WEAK_FLOW_THRESHOLD` | `100000` | Number | TUI Display |

### Server Variables
| Variable | Default | Type | Scope |
|----------|---------|------|-------|
| `WS_ADDR` | `0.0.0.0:9001` | String | Server |
| `WS_BUFFER_SIZE` | `10000` | Number | Server |
| `SPOT_LOG_THRESHOLD` | `50000` | Number | Server |

**Scope Legend:**
- **Shared State**: Affects data aggregation/calculation logic
- **TUI Display**: Affects only visualization/display
- **TUI**: Affects TUI client configuration
- **Server**: Affects barter-data-server behavior

---

## 🐛 Debugging

### Check Current Values
All TUIs will use default values if environment variables are not set. To verify what values are being used:

```bash
# Print and run
echo "WHALE_THRESHOLD=${WHALE_THRESHOLD:-500000}"
WHALE_THRESHOLD=250000 cargo run --release --bin market-microstructure
```

### Invalid Values
If an environment variable contains an invalid value (non-numeric where number expected), the default will be used:

```bash
# This will fall back to default $500,000
WHALE_THRESHOLD=invalid cargo run --release --bin market-microstructure
```

---

## 💡 Pro Tips

1. **Start with defaults** - Run with defaults first, then tune based on what you see
2. **Lower thresholds for altcoins** - They have lower notional volumes
3. **Increase MAX_WHALES in high-frequency markets** - Prevents spot whales from being pushed out by perp flood
4. **Match thresholds** - Keep `LIQ_DANGER_THRESHOLD` and `LIQ_DISPLAY_DANGER_THRESHOLD` the same unless you want different logic vs display behavior

---

## 🚀 Next Steps

1. Try the Quick Start examples above
2. Experiment with different thresholds for your markets
3. Create a launch script with your preferred settings
4. Share your optimal configurations with the team!

For questions or issues, see the main README or open an issue on GitHub.
