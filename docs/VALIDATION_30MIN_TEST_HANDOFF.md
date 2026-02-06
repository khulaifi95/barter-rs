# 30-Minute Data Accuracy Validation Test

## Objective

Run a 30-minute capture and compare data accuracy between:
1. **Barter (Our System)** - Parquet data from barter-data-server
2. **MMT (mmt.gg)** - Professional trading terminal (manual observation)
3. **Binance API** - Source of truth (REST API verification)

---

## Pre-Test Setup

### 1. Clean Environment
```bash
cd /Users/screener-m3/projects/barter-rs
rm -rf /tmp/validation_30min
mkdir -p /tmp/validation_30min
```

### 2. Build Check
```bash
cargo check -p barter-data-server
```

### 3. Open MMT in Browser
- URL: https://mmt.gg/app/terminal
- Select: **BTC/USD@BINANCEF** (Binance Futures Perpetual)
- Timeframe: **1m**
- Enable: Bar Statistics panel (shows Delta, Buy Vol, Sell Vol, Total Vol)

---

## Test Execution

### Start Capture (Record exact start time!)

```bash
# Record start time
echo "Test started at: $(date -u '+%Y-%m-%d %H:%M:%S') UTC"

# Run capture for 30 minutes
PARQUET_ENABLED=1 \
PARQUET_OUTPUT_DIR=/tmp/validation_30min \
PARQUET_FLUSH_INTERVAL_SECS=30 \
RUST_LOG=info \
timeout 1860 cargo run -p barter-data-server --bin barter-data-server 2>&1 | tee /tmp/validation_30min/server.log

echo "Test ended at: $(date -u '+%Y-%m-%d %H:%M:%S') UTC"
```

Note: `timeout 1860` = 31 minutes (30 min + 1 min buffer for final flush)

### During Test

1. **Monitor server logs** for errors:
   - No "flush failed" messages
   - No "dropped" messages
   - Bars being written (check `bars_total` in flush logs)

2. **Note MMT values** for 3-5 specific candles (screenshot recommended):
   - Pick candles at ~10min, ~20min, ~25min into test
   - Record: Time, Delta, Buy Vol, Sell Vol, Total Vol

---

## Post-Test Validation

### Step 1: Verify Capture Completeness

```bash
echo "=== CAPTURE SUMMARY ==="
echo "Trades files: $(find /tmp/validation_30min/trades -name '*.parquet' | wc -l)"
echo "Bars files: $(find /tmp/validation_30min/bars_1m -name '*.parquet' | wc -l)"
echo "Extended bars: $(find /tmp/validation_30min/extended_bars_1m -name '*.parquet' | wc -l)"

# Count rows
python3 -c "
import pyarrow.parquet as pq
from pathlib import Path
base = Path('/tmp/validation_30min')
for cat in ['trades', 'bars_1m', 'extended_bars_1m']:
    total = sum(pq.read_table(str(f)).num_rows for f in (base/cat).rglob('*.parquet'))
    print(f'{cat}: {total:,} rows')
"
```

**Expected:**
- ~25-30 extended bar files (one per flush cycle)
- ~30 bars (one per minute)
- Thousands of trades

### Step 2: Run API Comparison

```bash
python3 scripts/validation/compare_with_api.py /tmp/validation_30min --symbol BTCUSDT
```

### Step 3: Generate Detailed Report

```bash
python3 scripts/validation/generate_30min_report.py /tmp/validation_30min
```

---

## Expected Results

### Pass Criteria

| Metric | Threshold | Notes |
|--------|-----------|-------|
| Price match | 100% exact | All close prices must match API |
| Volume match | >99% within 0.1% | Except first/last partial candles |
| Delta match | >99% within 0.1% | Except first/last partial candles |
| Buy/Sell match | >99% within 0.1% | Except first/last partial candles |
| No gaps | 0 gaps | All 30 minutes covered |
| No flush errors | 0 errors | Check server.log |

### Expected Comparison vs MMT

Based on previous 5-minute test:
- **Our accuracy**: 99.99% match to Binance API
- **MMT accuracy**: ~99.5% match (0.04-0.51 deviations)
- **Our system should be more accurate than MMT**

---

## Report Template

The final report should include:

```markdown
# 30-Minute Validation Report

**Date:** YYYY-MM-DD
**Time Range:** HH:MM - HH:MM UTC
**Instrument:** BTCUSDT Perpetual (Binance Futures)

## Summary
- Total candles: X
- Complete candles: X (excluding first/last partial)
- Accuracy vs API: XX.XX%

## Detailed Comparison

| Time | Metric | Binance API | Barter | MMT | Barter Err | MMT Err |
|------|--------|-------------|--------|-----|------------|---------|
| ... | ... | ... | ... | ... | ... | ... |

## Error Analysis

### Barter Errors
- Average error: X.XXXX
- Max error: X.XXXX (candle HH:MM)
- Candles with >1% error: X

### MMT Errors (if recorded)
- Average error: X.XXXX
- Max error: X.XXXX (candle HH:MM)

## Conclusion
[Pass/Fail] - [Summary statement]
```

---

## Troubleshooting

### No parquet files created
- Check `PARQUET_ENABLED=1` is set
- Check output directory exists and is writable
- Check server.log for errors

### Gaps in data
- Check for WebSocket disconnection messages in log
- Verify network stability during test

### Large errors on specific candles
- First/last candles expected to be partial
- Check if server restarted during that minute
- Verify timestamp alignment (our ts_event = close time)

---

## Files Reference

| File | Purpose |
|------|---------|
| `/tmp/validation_30min/` | Test output directory |
| `server.log` | Server output during test |
| `trades/` | Raw trade parquet files |
| `bars_1m/` | Nautilus-compatible OHLCV bars |
| `extended_bars_1m/` | Full 43-field bars with derivatives |
| `scripts/validation/compare_with_api.py` | API comparison script |
| `scripts/validation/generate_30min_report.py` | Report generator (to be created) |
