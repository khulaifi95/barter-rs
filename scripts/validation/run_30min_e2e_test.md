# 30-Minute End-to-End Validation Test

## Objective
Validate the complete data pipeline from collector through feature processing:
1. barter-data-server collects live market data (1-minute bars + trades)
2. barter-features processes the data in watch mode
3. Output is validated for correctness

## Prerequisites

### 1. Build Both Components
```bash
cd /Users/screener-m3/projects/barter-rs
cargo build --release -p barter-data-server
cargo build --release -p barter-features
```

### 2. Create Test Directories
```bash
mkdir -p /tmp/e2e_test/raw
mkdir -p /tmp/e2e_test/features
mkdir -p /tmp/e2e_test/checkpoints
```

## Test Execution

### Step 1: Start Collector (Terminal 1)
```bash
cd /Users/screener-m3/projects/barter-rs

# Set environment for collector
export BARTER_DATA_OUTPUT_DIR=/tmp/e2e_test/raw
export BARTER_DATA_SYMBOLS=BTCUSDT
export BARTER_DATA_EXCHANGE=binance
export RUST_LOG=info

# Run collector
./target/release/barter-data-server
```

### Step 2: Start Feature Processor (Terminal 2)
```bash
cd /Users/screener-m3/projects/barter-rs

# Set environment for features
export BARTER_FEATURES_INPUT_DIR=/tmp/e2e_test/raw
export BARTER_FEATURES_OUTPUT_DIR=/tmp/e2e_test/features
export BARTER_FEATURES_CHECKPOINT_DIR=/tmp/e2e_test/checkpoints
export RUST_LOG=info

# Run in watch mode
./target/release/barter-features --mode watch
```

### Step 3: Wait 30+ Minutes
- Let both processes run for at least 30 minutes
- Monitor logs for errors
- Check that files are being created

### Step 4: Validate Results

#### Check Raw Data (Collector Output)
```bash
# List collected files
find /tmp/e2e_test/raw -name "*.parquet" -type f

# Check extended bars exist and have data
python3 << 'EOF'
import pyarrow.parquet as pq
from pathlib import Path

raw_dir = Path("/tmp/e2e_test/raw")
for pq_file in raw_dir.rglob("extended_bars*.parquet"):
    table = pq.read_table(pq_file)
    print(f"{pq_file}: {table.num_rows} rows")
    if table.num_rows > 0:
        df = table.to_pandas()
        print(f"  Time range: {df['ts_event'].min()} to {df['ts_event'].max()}")
        print(f"  Columns: {list(df.columns)[:10]}...")
EOF
```

#### Check Feature Output
```bash
python3 << 'EOF'
import pyarrow.parquet as pq
from pathlib import Path

features_dir = Path("/tmp/e2e_test/features")

# Check TPO brackets
for pq_file in features_dir.rglob("tpo_brackets.parquet"):
    table = pq.read_table(pq_file)
    print(f"\nTPO Brackets: {pq_file}")
    print(f"  Rows: {table.num_rows}")
    if table.num_rows > 0:
        df = table.to_pandas()
        # Decode values (÷1e9)
        df['vol_poc_usd'] = df['vol_poc'] / 1e9
        df['vol_vah_usd'] = df['vol_vah'] / 1e9
        df['vol_val_usd'] = df['vol_val'] / 1e9
        print(f"  Labels: {df['label'].tolist()}")
        print(f"  POC range: ${df['vol_poc_usd'].min():.2f} - ${df['vol_poc_usd'].max():.2f}")
        print(f"  VAH range: ${df['vol_vah_usd'].min():.2f} - ${df['vol_vah_usd'].max():.2f}")
        print(f"  VAL range: ${df['vol_val_usd'].min():.2f} - ${df['vol_val_usd'].max():.2f}")

        # Validate relationships
        errors = []
        for i, row in df.iterrows():
            if not (row['vol_val'] <= row['vol_poc'] <= row['vol_vah']):
                errors.append(f"Row {i}: VAL <= POC <= VAH violated")
        if errors:
            print(f"  ERRORS: {errors[:5]}")
        else:
            print(f"  ✅ All VAL <= POC <= VAH relationships valid")

# Check profile events
for pq_file in features_dir.rglob("profile_events.parquet"):
    table = pq.read_table(pq_file)
    print(f"\nProfile Events: {pq_file}")
    print(f"  Rows: {table.num_rows}")
    if table.num_rows > 0:
        df = table.to_pandas()
        print(f"  Event types: {df['event_type'].value_counts().to_dict()}")

# Check large trades
for pq_file in features_dir.rglob("large_trades.parquet"):
    table = pq.read_table(pq_file)
    print(f"\nLarge Trades: {pq_file}")
    print(f"  Rows: {table.num_rows}")
    if table.num_rows > 0:
        df = table.to_pandas()
        df['notional_usd_decoded'] = df['notional_usd'] / 1e9
        print(f"  Categories: {df['category'].value_counts().to_dict()}")
        print(f"  Notional range: ${df['notional_usd_decoded'].min():,.0f} - ${df['notional_usd_decoded'].max():,.0f}")
        print(f"  Source precision: {df['source_precision'].iloc[0]}")
EOF
```

#### Run Full Validation Script
```bash
# Find the output directory for today
TODAY=$(date +%Y-%m-%d)
OUTPUT_DIR="/tmp/e2e_test/features/BTCUSDT-PERP.BINANCE/${TODAY}"

if [ -d "$OUTPUT_DIR" ]; then
    python scripts/validation/validate_features.py "$OUTPUT_DIR"
else
    echo "Output directory not found: $OUTPUT_DIR"
    echo "Available directories:"
    find /tmp/e2e_test/features -type d
fi
```

## Expected Results After 30 Minutes

### Collector Output
- `/tmp/e2e_test/raw/BTCUSDT-PERP.BINANCE/{date}/extended_bars_1m.parquet`
  - Should have ~30 rows (one per minute)
  - All OHLCV columns populated
  - buy_volume + sell_volume ≈ volume

### Feature Output
- `/tmp/e2e_test/features/BTCUSDT-PERP.BINANCE/{date}/tpo_brackets.parquet`
  - Should have 1 bracket (label "A") after 30 minutes
  - POC, VAH, VAL should be valid prices
  - VAL <= POC <= VAH must hold

- `/tmp/e2e_test/features/BTCUSDT-PERP.BINANCE/{date}/profile_events.parquet`
  - Should have SessionOpen event
  - May have IbComplete if 2 brackets completed

## Success Criteria

1. ✅ Collector produces extended_bars with ~30 rows
2. ✅ Feature processor produces tpo_brackets.parquet
3. ✅ TPO bracket has valid POC/VAH/VAL (realistic BTC prices)
4. ✅ VAL <= POC <= VAH relationship holds
5. ✅ Labels are sequential starting with "A"
6. ✅ Metadata (schema_version, source_precision) present
7. ✅ No errors in either process logs

## Cleanup
```bash
# Stop both processes (Ctrl+C)
# Remove test data
rm -rf /tmp/e2e_test
```
