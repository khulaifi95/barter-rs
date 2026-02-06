# Barter Data Server - Validation Experiment Handoff

## Objective
Run a 15-minute validation test to verify the **collector-only** pipeline is healthy before cloud deployment.
This aligns with the current decision: **PERP-only**, **Binance-only** parquet capture, **BTC** as the initial asset.

## Validation Policy (When / Why / How)
- **When**: Before any cloud/VPS deployment, and after any change to data ingestion,
  normalization, or parquet writing.
- **Why**: To confirm data correctness, feed availability, and API parity on a clean
  15-minute run.
- **How**: Use the **official validation scripts only** (below). Avoid ad‑hoc runs
  or custom scripts that aren’t checked into the repo.
- **Change control**: Updates to validation scripts or criteria require review and
  must be committed before use in production checks.

## Pre-Validation Checklist

### 1. Binary Status
- [x] Binary built: `target/release/barter-data-server` (Jan 30 13:30)
- [x] Latest commits included: da886e8 (stream/parquet filters)

### 2. Data Feeds Configuration (Collector Mode)

| Feed | Exchange | Status | Notes |
|------|----------|--------|-------|
| **L1 (Best Bid/Ask)** | Binance | ✅ Enabled | PERP only |
| **Trades** | Binance | ✅ Enabled | Used for delta/CVD |
| **Open Interest** | Binance | ✅ Enabled | PERP only |
| **Liquidations** | Binance | ✅ Enabled | PERP only |
| **CVD** | Binance | ✅ Enabled | From trades |
| **Funding Rate** | Binance | ✅ Enabled | Via REST polling |
| **L2 Orderbook** | Binance | ⚠️ Optional | Disable for lean mode |

### 3. Known Behaviors
- **Liquidations = 0**: Normal for quiet minutes.
- **Depth bands = 0**: Expected when L2 is disabled.
- **OI change mismatch across restarts**: Normal, resets on startup.

---

## Validation Experiment

### Run Command
```bash
cd /Users/screener-m3/projects/barter-rs

# Option A: Use validation script (15 minutes default)
./scripts/validation/run_collector_validation.sh

# Option B: Manual run (15 minutes)
export PARQUET_ENABLED=1
export PARQUET_OUTPUT_DIR="/tmp/barter_validation_15m"
export PARQUET_FLUSH_INTERVAL_SECS=60
export RUST_LOG=info
export STREAM_L2=0  # Lean mode (set to 1 if capturing L2 deltas)
export STREAM_ASSETS=BTC
export STREAM_VENUES=BINANCE
export STREAM_PERP=1
export STREAM_SPOT=0
export PARQUET_ASSETS=BTC
export PARQUET_VENUES=BINANCE
export PARQUET_WRITE_TRADES=0
export PARQUET_WRITE_BARS=1
export PARQUET_WRITE_EXTENDED=1
export PARQUET_WRITE_L2=0       # Off by default (high volume)
export PARQUET_L2_MAX_DEPTH=50  # Only used when PARQUET_WRITE_L2=1
export PARQUET_L2_SAMPLE_MS=0   # 0 = write every L2 update

mkdir -p "$PARQUET_OUTPUT_DIR"
timeout 900 ./target/release/barter-data-server 2>&1 | tee "$PARQUET_OUTPUT_DIR/server.log"
```

### Expected Output During Run
```
INFO  Initializing market data streams...
INFO  Binance trades stream connected
INFO  Starting Parquet writer task...
INFO  WebSocket server listening on 0.0.0.0:9002
```

### Metrics to Monitor (printed every 60s)
```
Event counts: binance=XXXX okx=XXXX bybit=XXXX
Trade skew: avg=XX ms, max=XX ms
Parquet: bars=XX, extended=XX, trades=XX
```

---

## Post-Run Validation

### 1. Run Validation Script
```bash
cd ~/projects/nautilus_trader && source .venv/bin/activate
cd ~/projects/barter-rs
python3 scripts/validation/validate_parquet.py /tmp/barter_validation_15m

# Optional: API parity checks (time-aligned)
python3 scripts/validation/validate_parquet.py --api /tmp/barter_validation_15m
```

### 2. Expected Results

| Check | Expected | Critical? |
|-------|----------|-----------|
| Binance L1 (bid/ask > 0) | 100% | Yes |
| Trades (volume > 0) | 100% | Yes |
| Delta = buy_vol - sell_vol | 100% match | Yes |
| CVD continuity | cvd_t = cvd_{t-1} + delta | Yes |
| OI change continuity | oi_change = oi - prev_oi (contiguous bars) | Yes |
| OI > 0 | >90% | Yes |
| Funding rate set | >0 | Yes |
| liq_total = liq_buy + liq_sell | 100% match | Yes |
| spread_bps ≥ 0 | 100% | Yes |
| book_imbalance ∈ [-1,1] | 100% | Yes |
| Depth monotonicity | 10bps ≤ 50bps ≤ 100bps | Yes (if L2 enabled) |
| OHLC integrity | high ≥ max(open,close), low ≤ min(open,close) | Yes |
| Minute continuity | No 60s gaps | Yes |
| Quote vol sanity | |qv−vol×close|/qv ≤ 5% | Warning |

### 3. Cross-Validation with API

Compare captured data with exchange APIs:
```bash
# Get current Binance BTC price
curl -s "https://fapi.binance.com/fapi/v1/ticker/price?symbol=BTCUSDT" | jq

# Compare with last bar's close price in parquet

# Get current Binance BTC open interest
curl -s "https://fapi.binance.com/fapi/v1/openInterest?symbol=BTCUSDT" | jq

# Get latest Binance funding rate
curl -s "https://fapi.binance.com/fapi/v1/fundingRate?symbol=BTCUSDT&limit=1" | jq
```

---

## Data Quality Checks

### Volume Consistency
```python
# quote_volume should be approximately volume * close
# Allow 5% deviation due to VWAP vs close price
deviation = abs(quote_volume - volume * close) / quote_volume
assert deviation < 0.05
```

### OI Change Consistency
```python
# In continuous run, oi_change should match bar-to-bar diff
# (Does not apply across restarts)
assert oi_change[i] == open_interest[i] - open_interest[i-1]
```

### No Negative Values
```python
assert (volume >= 0).all()
assert (quote_volume >= 0).all()
assert (open_interest >= 0).all()
assert (liq_total_usd >= 0).all()
```

---

## Success Criteria

### PASS - Ready for Cloud Deployment
- [ ] All critical checks pass
- [ ] Binance L1 data flowing
- [ ] Trades flowing (Binance)
- [ ] OI and funding data present
- [ ] No crashes or disconnections during 15-minute run
- [ ] Parquet files generated every minute

### FAIL - Needs Investigation
- [ ] Any critical check fails
- [ ] Exchange disconnections > 3
- [ ] Missing data for entire exchange
- [ ] Parquet write errors

---

## Files Generated

```
/tmp/barter_validation_15m/
├── server.log              # Full server output
├── bars_1m/
│   └── BTCUSDT_PERP_BINANCE_1_MINUTE_LAST_EXTERNAL/
│       └── 2026-02-02/
│           └── *.parquet   # ~15 files
├── extended_bars_1m/
│   └── BTCUSDT_PERP_BINANCE/
│       └── 2026-02-02/
│           └── *.parquet
└── trades/                 # If PARQUET_WRITE_TRADES=1
```

---

## Cloud Deployment Config (After Validation)

### AWS/Hetzner Lean Mode (Collector-only)
```bash
# Disable L2 for RAM savings (~150MB saved)
STREAM_L2=0

# Enable Parquet
PARQUET_ENABLED=1
PARQUET_OUTPUT_DIR=/data/parquet  # or S3 path
PARQUET_WRITE_TRADES=0
PARQUET_WRITE_BARS=1
PARQUET_WRITE_EXTENDED=1

# Filter assets/venues
STREAM_ASSETS=BTC
STREAM_VENUES=BINANCE
STREAM_PERP=1
STREAM_SPOT=0
PARQUET_ASSETS=BTC
PARQUET_VENUES=BINANCE
```

### S3 Upload (if using AWS)
```bash
S3_ENABLED=1
S3_BUCKET=my-parquet-bucket
S3_REGION=ap-southeast-2
AWS_ACCESS_KEY_ID=...
AWS_SECRET_ACCESS_KEY=...
```

---

## Contact

If issues arise during validation:
1. Check `server.log` for errors
2. Verify network connectivity to exchanges
3. Check for rate limiting (403/429 errors)
