# Deployment Checklist: barter-data-server + TUIs

## Pre-Deployment Verification

### 1. Run Tests
```bash
cargo test -p barter-data-server
cargo test -p barter-trading-tuis --lib
```
Expected: All tests pass (18 server + 180 TUI lib tests)

### 2. Run Load Test (Optional but Recommended)
```bash
LOAD_TEST_RATE=5000 LOAD_TEST_DURATION_SECS=300 cargo run --bin load-test --release
```
Monitor for:
- `dropped=0` (no backpressure)
- `latency_avg < 1000us` (sub-millisecond processing)
- Stable memory (no RSS growth over time)

---

## Environment Variables

### Server (barter-data-server)

| Variable | Default | Description | Recommendation |
|----------|---------|-------------|----------------|
| `WS_ADDR` | `0.0.0.0:9001` | WebSocket bind address | Keep default or use specific IP |
| `WS_BINARY_FRAMES` | `true` | Use binary WS frames (faster) | **Keep `true` for TUIs** |
| `WS_ENVELOPE` | `false` | Wrap messages in envelope | Keep `false` unless schema evolution needed |
| `WS_SOURCE` | `barter-data-server` | Source identifier in envelope | Only used if envelope enabled |
| `WS_TRADES_BUFFER` | `100000` | Trade broadcast channel size | Increase if seeing lag warnings |
| `WS_L2_BUFFER` | `50000` | L2 broadcast channel size | Lower priority, can lag |
| `AGG_EVENT_BUFFER` | `10000` | Aggregator channel size | **Increase to 50000+ for production** |
| `SNAPSHOT_SECS` | `1` | Snapshot publish interval | Keep 1s for real-time |
| `TICKERS` | `BTC,ETH,SOL` | Tickers to subscribe | Comma-separated, uppercase |
| `L1_THROTTLE_MS` | `50` | L1 orderbook throttle | Reduce flood, keep 50-100ms |
| `L2_THROTTLE_OKX_MS` | `150` | OKX L2 throttle | Per-exchange tuning |
| `L2_THROTTLE_BYBIT_MS` | `100` | Bybit L2 throttle | Per-exchange tuning |
| `SPOT_LOG_THRESHOLD` | `50000` | Log trades above this USD | Debug only |
| `RUST_LOG` | `info` | Log level | Use `warn` in production |

### Client (TUIs)

| Variable | Default | Description | Recommendation |
|----------|---------|-------------|----------------|
| `WS_ENVELOPE` | `false` | Must match server setting | **Sync with server** |
| `WS_URL` | `ws://127.0.0.1:9001` | Server WebSocket URL | Point to server |

### Critical Compatibility

> **WS_BINARY_FRAMES** and **WS_ENVELOPE** must match between server and all clients.
> Mismatch causes silent parsing failures.

---

## Ports & Network

| Service | Default Port | Protocol | Notes |
|---------|--------------|----------|-------|
| WebSocket Server | 9001 | WS/WSS | Main data feed |
| IBKR Bridge | 5001 | WS | Traditional markets (optional) |

### Firewall Rules
```bash
# Allow WebSocket connections
ufw allow 9001/tcp

# If using IBKR bridge
ufw allow 5001/tcp
```

---

## Startup Commands

### Production Server
```bash
# Set environment
export WS_BINARY_FRAMES=true
export WS_ENVELOPE=false
export AGG_EVENT_BUFFER=50000
export TICKERS=BTC,ETH,SOL
export RUST_LOG=warn

# Run with nohup (or use systemd)
nohup cargo run --bin barter-data-server --release > /tmp/data-server.log 2>&1 &
```

### Systemd Service (Recommended)
```ini
# /etc/systemd/system/barter-data-server.service
[Unit]
Description=Barter Data Server
After=network.target

[Service]
Type=simple
User=trader
WorkingDirectory=/home/trader/barter-rs
Environment="WS_BINARY_FRAMES=true"
Environment="WS_ENVELOPE=false"
Environment="AGG_EVENT_BUFFER=50000"
Environment="TICKERS=BTC,ETH,SOL"
Environment="RUST_LOG=warn"
ExecStart=/home/trader/barter-rs/target/release/barter-data-server
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable barter-data-server
sudo systemctl start barter-data-server
```

---

## Log Locations

| Log | Location | Contents |
|-----|----------|----------|
| Server stdout | `/tmp/data-server.log` | METRICS, FEEDS, warnings |
| TUI audit | `./audit_*.jsonl` | State transitions (if enabled) |

### Key Log Patterns to Monitor

```bash
# Normal operation
grep "METRICS:" /tmp/data-server.log | tail -5
# Example: METRICS: trades/min=9065 (151.1/s), skew_avg=50ms

# Feed health
grep "FEEDS:" /tmp/data-server.log | tail -5
# Example: FEEDS: binance=56625/min, okx=2693/min, agg_dropped=0

# Warnings (should be rare)
grep -E "STALE:|WARN|dropped" /tmp/data-server.log | tail -10
```

---

## Monitoring & Alerts

### Health Metrics (from METRICS log line)

| Metric | Healthy | Warning | Critical |
|--------|---------|---------|----------|
| `trades/min` | > 1000 | < 500 | 0 |
| `skew_avg` | < 100ms | 100-500ms | > 500ms |
| `agg_dropped` | 0 | 1-100 | > 100 |

### Feed Health (from FEEDS log line)

| Feed | Healthy | Warning | Critical |
|------|---------|---------|----------|
| `binance` | > 10000/min | < 5000/min | 0 |
| `okx` | > 1000/min | < 500/min | 0 |
| `bybit` | > 1000/min | < 500/min | 0 |

### Stale Alerts

Any line containing `STALE:` indicates a feed hasn't sent data for 30+ seconds.

```bash
# Alert on stale feeds
grep "STALE:" /tmp/data-server.log
```

---

## Restart Procedure

### Graceful Restart
```bash
# 1. Stop server
pkill -f barter-data-server
# or: systemctl stop barter-data-server

# 2. Verify stopped
pgrep -f barter-data-server  # Should return nothing

# 3. Start server
systemctl start barter-data-server
# or: nohup cargo run --bin barter-data-server --release > /tmp/data-server.log 2>&1 &

# 4. Verify started
sleep 5
grep "WebSocket server listening" /tmp/data-server.log

# 5. Verify feeds connecting
sleep 30
grep "FEEDS:" /tmp/data-server.log | tail -1
```

### Emergency Restart (Force)
```bash
pkill -9 -f barter-data-server
sleep 2
systemctl start barter-data-server
```

---

## Troubleshooting

### No Data Flowing
1. Check server is running: `pgrep -f barter-data-server`
2. Check WebSocket port: `netstat -tlnp | grep 9001`
3. Check exchange connections: `grep "Reconnecting" /tmp/data-server.log`
4. Check for stale feeds: `grep "STALE:" /tmp/data-server.log`

### High Latency (skew_avg > 500ms)
1. Check `agg_dropped` - if > 0, increase `AGG_EVENT_BUFFER`
2. Check CPU usage - if > 80%, reduce `TICKERS` or upgrade hardware
3. Check network latency to exchanges

### TUI Not Receiving Data
1. Verify `WS_BINARY_FRAMES` matches server setting
2. Verify `WS_ENVELOPE` matches server setting
3. Check TUI logs for parse errors

### Memory Growth
1. Run load test for 30+ minutes, monitor RSS
2. If RSS grows unbounded, restart server and report issue

---

## Pre-Production Checklist

- [ ] All tests pass (`cargo test`)
- [ ] Load test completes with `dropped=0`
- [ ] `WS_BINARY_FRAMES=true` on server and clients
- [ ] `WS_ENVELOPE=false` on server and clients (unless needed)
- [ ] `AGG_EVENT_BUFFER >= 50000`
- [ ] Log rotation configured (logrotate or similar)
- [ ] Monitoring alerts configured for STALE and high `agg_dropped`
- [ ] Firewall allows port 9001
- [ ] Systemd service enabled for auto-restart
