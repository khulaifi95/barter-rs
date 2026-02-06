# End-to-End Validation Agent Prompt

## Mission
Run a 30+ minute end-to-end validation test of the barter-rs data pipeline:
1. Start the collector (barter-data-server) to collect live BTC market data
2. Start the feature processor (barter-features) in watch mode
3. Wait for data accumulation
4. Validate all output data for correctness

## Prerequisites Check
First, verify both binaries are built:
```bash
ls -la /Users/screener-m3/projects/barter-rs/target/release/barter-data-server
ls -la /Users/screener-m3/projects/barter-rs/target/release/barter-features
```

If not built, run:
```bash
cd /Users/screener-m3/projects/barter-rs
cargo build --release -p barter-data-server -p barter-features
```

## Phase 1: Setup Test Environment

Create isolated test directories:
```bash
rm -rf /tmp/e2e_test
mkdir -p /tmp/e2e_test/raw
mkdir -p /tmp/e2e_test/features
mkdir -p /tmp/e2e_test/checkpoints
mkdir -p /tmp/e2e_test/logs
```

## Phase 2: Start Data Collection

Start the collector in background, capturing logs:
```bash
cd /Users/screener-m3/projects/barter-rs

BARTER_DATA_OUTPUT_DIR=/tmp/e2e_test/raw \
RUST_LOG=info \
./target/release/barter-data-server \
  > /tmp/e2e_test/logs/collector.log 2>&1 &

COLLECTOR_PID=$!
echo "Collector PID: $COLLECTOR_PID"
echo $COLLECTOR_PID > /tmp/e2e_test/collector.pid
```

Wait 30 seconds for collector to initialize and start receiving data:
```bash
sleep 30
```

Verify collector is running and producing data:
```bash
ps aux | grep barter-data-server | grep -v grep
find /tmp/e2e_test/raw -name "*.parquet" 2>/dev/null | head -5
tail -20 /tmp/e2e_test/logs/collector.log
```

## Phase 3: Start Feature Processor

Start feature processor in background:
```bash
cd /Users/screener-m3/projects/barter-rs

BARTER_FEATURES_INPUT_DIR=/tmp/e2e_test/raw \
BARTER_FEATURES_OUTPUT_DIR=/tmp/e2e_test/features \
BARTER_FEATURES_CHECKPOINT_DIR=/tmp/e2e_test/checkpoints \
RUST_LOG=info \
./target/release/barter-features --mode watch \
  > /tmp/e2e_test/logs/features.log 2>&1 &

FEATURES_PID=$!
echo "Features PID: $FEATURES_PID"
echo $FEATURES_PID > /tmp/e2e_test/features.pid
```

## Phase 4: Monitor Progress

Check status every 5 minutes for 30 minutes:

```bash
# Function to check status
check_status() {
    echo "=== Status Check at $(date) ==="

    # Count raw files
    RAW_COUNT=$(find /tmp/e2e_test/raw -name "*.parquet" 2>/dev/null | wc -l)
    echo "Raw parquet files: $RAW_COUNT"

    # Count feature files
    FEATURE_COUNT=$(find /tmp/e2e_test/features -name "*.parquet" 2>/dev/null | wc -l)
    echo "Feature parquet files: $FEATURE_COUNT"

    # Check for errors in logs
    COLLECTOR_ERRORS=$(grep -c -i "error" /tmp/e2e_test/logs/collector.log 2>/dev/null || echo 0)
    FEATURE_ERRORS=$(grep -c -i "error" /tmp/e2e_test/logs/features.log 2>/dev/null || echo 0)
    echo "Collector errors: $COLLECTOR_ERRORS"
    echo "Feature errors: $FEATURE_ERRORS"

    # Check processes are still running
    if ps -p $(cat /tmp/e2e_test/collector.pid 2>/dev/null) > /dev/null 2>&1; then
        echo "Collector: RUNNING"
    else
        echo "Collector: STOPPED"
    fi

    if ps -p $(cat /tmp/e2e_test/features.pid 2>/dev/null) > /dev/null 2>&1; then
        echo "Features: RUNNING"
    else
        echo "Features: STOPPED"
    fi
}

# Run check
check_status
```

Wait for 30 minutes total (check every 5 minutes):
```bash
for i in 1 2 3 4 5 6; do
    echo "Waiting 5 minutes... ($i/6)"
    sleep 300
    check_status
done
```

## Phase 5: Stop Processes

Stop both processes gracefully:
```bash
kill $(cat /tmp/e2e_test/collector.pid) 2>/dev/null
kill $(cat /tmp/e2e_test/features.pid) 2>/dev/null
sleep 5
```

## Phase 6: Validate Results

### 6.1 Validate Collector Output
```python
#!/usr/bin/env python3
import pyarrow.parquet as pq
from pathlib import Path
import sys

print("=" * 60)
print("COLLECTOR OUTPUT VALIDATION")
print("=" * 60)

raw_dir = Path("/tmp/e2e_test/raw")
errors = []
warnings = []

# Find extended bars files
bar_files = list(raw_dir.rglob("extended_bars*.parquet"))
if not bar_files:
    errors.append("No extended_bars parquet files found")
else:
    for pq_file in bar_files:
        print(f"\nFile: {pq_file}")
        table = pq.read_table(pq_file)
        print(f"  Rows: {table.num_rows}")

        if table.num_rows == 0:
            errors.append(f"{pq_file}: Empty file")
            continue

        df = table.to_pandas()

        # Check row count (expect ~30 for 30 minutes)
        if table.num_rows < 25:
            warnings.append(f"{pq_file}: Only {table.num_rows} rows (expected ~30)")

        # Decode OHLCV (÷1e9)
        for col in ['open', 'high', 'low', 'close', 'volume']:
            if col in df.columns:
                df[f'{col}_decoded'] = df[col] / 1e9

        # Check price sanity (BTC should be $50k-$150k)
        if 'close_decoded' in df.columns:
            min_price = df['close_decoded'].min()
            max_price = df['close_decoded'].max()
            print(f"  Price range: ${min_price:,.2f} - ${max_price:,.2f}")

            if min_price < 10000 or max_price > 200000:
                errors.append(f"{pq_file}: Price out of expected range")

        # Check volume is positive
        if 'volume_decoded' in df.columns:
            if (df['volume_decoded'] <= 0).any():
                errors.append(f"{pq_file}: Non-positive volume found")
            print(f"  Volume range: {df['volume_decoded'].min():.4f} - {df['volume_decoded'].max():.4f}")

# Find trades files
trades_files = list(raw_dir.rglob("trades*.parquet"))
print(f"\nTrades files found: {len(trades_files)}")

print("\n" + "=" * 60)
print("COLLECTOR VALIDATION RESULT")
print("=" * 60)
if errors:
    print("ERRORS:")
    for e in errors:
        print(f"  ❌ {e}")
if warnings:
    print("WARNINGS:")
    for w in warnings:
        print(f"  ⚠️ {w}")
if not errors:
    print("✅ Collector output validation PASSED")

sys.exit(1 if errors else 0)
```

### 6.2 Validate Feature Output
```python
#!/usr/bin/env python3
import pyarrow.parquet as pq
from pathlib import Path
import sys

print("=" * 60)
print("FEATURE OUTPUT VALIDATION")
print("=" * 60)

features_dir = Path("/tmp/e2e_test/features")
errors = []
warnings = []

# Find TPO brackets
tpo_files = list(features_dir.rglob("tpo_brackets.parquet"))
if not tpo_files:
    errors.append("No tpo_brackets.parquet files found")
else:
    for pq_file in tpo_files:
        print(f"\nTPO Brackets: {pq_file}")
        table = pq.read_table(pq_file)
        print(f"  Rows: {table.num_rows}")

        if table.num_rows == 0:
            warnings.append(f"{pq_file}: Empty (may need more time)")
            continue

        df = table.to_pandas()

        # Check labels
        labels = df['label'].tolist()
        print(f"  Labels: {labels}")
        expected_first = 'A'
        if labels[0] != expected_first:
            errors.append(f"First label should be 'A', got '{labels[0]}'")

        # Decode POC/VAH/VAL
        df['vol_poc_usd'] = df['vol_poc'] / 1e9
        df['vol_vah_usd'] = df['vol_vah'] / 1e9
        df['vol_val_usd'] = df['vol_val'] / 1e9

        print(f"  POC: ${df['vol_poc_usd'].iloc[-1]:,.2f}")
        print(f"  VAH: ${df['vol_vah_usd'].iloc[-1]:,.2f}")
        print(f"  VAL: ${df['vol_val_usd'].iloc[-1]:,.2f}")

        # Validate VAL <= POC <= VAH
        for i, row in df.iterrows():
            if not (row['vol_val'] <= row['vol_poc'] <= row['vol_vah']):
                errors.append(f"Row {i}: VAL <= POC <= VAH violated")

        if not any('VAL <= POC <= VAH' in e for e in errors):
            print("  ✅ VAL <= POC <= VAH: Valid")

        # Check metadata
        if 'source_precision' in df.columns:
            print(f"  source_precision: {df['source_precision'].iloc[0]}")
        if 'schema_version' in df.columns:
            print(f"  schema_version: {df['schema_version'].iloc[0]}")

# Find profile events
event_files = list(features_dir.rglob("profile_events.parquet"))
print(f"\nProfile event files found: {len(event_files)}")
for pq_file in event_files:
    table = pq.read_table(pq_file)
    if table.num_rows > 0:
        df = table.to_pandas()
        print(f"  Events: {df['event_type'].value_counts().to_dict()}")

# Find large trades
trades_files = list(features_dir.rglob("large_trades.parquet"))
print(f"\nLarge trades files found: {len(trades_files)}")
for pq_file in trades_files:
    table = pq.read_table(pq_file)
    if table.num_rows > 0:
        df = table.to_pandas()
        print(f"  Categories: {df['category'].value_counts().to_dict()}")
        print(f"  source_precision: {df['source_precision'].iloc[0]}")

print("\n" + "=" * 60)
print("FEATURE VALIDATION RESULT")
print("=" * 60)
if errors:
    print("ERRORS:")
    for e in errors:
        print(f"  ❌ {e}")
if warnings:
    print("WARNINGS:")
    for w in warnings:
        print(f"  ⚠️ {w}")
if not errors:
    print("✅ Feature output validation PASSED")

sys.exit(1 if errors else 0)
```

### 6.3 Run Official Validation Script
```bash
TODAY=$(date +%Y-%m-%d)
OUTPUT_DIR="/tmp/e2e_test/features/BTCUSDT-PERP.BINANCE/${TODAY}"

python /Users/screener-m3/projects/barter-rs/scripts/validation/validate_features.py "$OUTPUT_DIR"
```

## Phase 7: Generate Report

Create validation report:
```bash
cat << 'EOF' > /tmp/e2e_test/VALIDATION_REPORT.md
# E2E Validation Report

**Date:** $(date)
**Duration:** 30 minutes

## Files Generated

### Collector (Raw Data)
$(find /tmp/e2e_test/raw -name "*.parquet" -exec echo "- {}" \;)

### Feature Processor
$(find /tmp/e2e_test/features -name "*.parquet" -exec echo "- {}" \;)

## Log Summary

### Collector Errors
$(grep -i error /tmp/e2e_test/logs/collector.log | tail -10 || echo "None")

### Feature Processor Errors
$(grep -i error /tmp/e2e_test/logs/features.log | tail -10 || echo "None")

## Validation Results
[Insert validation output here]

EOF

cat /tmp/e2e_test/VALIDATION_REPORT.md
```

## Success Criteria

The test PASSES if:
1. ✅ Collector produces extended_bars with 25+ rows
2. ✅ Feature processor produces tpo_brackets.parquet
3. ✅ TPO brackets have valid BTC prices ($50k-$150k range)
4. ✅ VAL <= POC <= VAH relationship holds for all rows
5. ✅ Labels start with "A" and are sequential
6. ✅ Metadata (schema_version, source_precision) present and correct
7. ✅ No critical errors in logs
8. ✅ validate_features.py passes

## Cleanup

After validation:
```bash
rm -rf /tmp/e2e_test
```
