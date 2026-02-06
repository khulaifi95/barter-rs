# Nautilus End-to-End Parquet + Features Validation (Collector -> Features -> Nautilus)

## Objective
Validate that:
1) Collector writes raw trades + bars + extended bars Parquet
2) Feature layer consumes extended bars and writes feature Parquet
3) Nautilus can read the data (bars/trades now; features once CustomData classes are added)

---

## Prerequisites
- `barter-data-server` builds and runs
- `barter-features` builds and runs
- Python env with Nautilus installed
- Parquet output directory writable

---

## Step 1: Run collector (BTCUSDT PERP only)

```bash
rm -rf /tmp/nautilus_e2e && mkdir -p /tmp/nautilus_e2e/{raw,features,logs}

PARQUET_ENABLED=1 \
PARQUET_OUTPUT_DIR=/tmp/nautilus_e2e/raw \
PARQUET_FLUSH_INTERVAL_SECS=60 \
BARTER_FILTER_INSTRUMENTS=BTCUSDT-PERP.BINANCE \
RUST_LOG=info \
cargo run -p barter-data-server --bin barter-data-server \
  2>&1 | tee /tmp/nautilus_e2e/logs/collector.log
```

Optional L2 capture (order book deltas):
```bash
export PARQUET_WRITE_L2=1
export PARQUET_L2_MAX_DEPTH=50
export STREAM_L2=1
export PARQUET_L2_SAMPLE_MS=0
```

Run at least 20–30 minutes so you get 2+ TPO brackets.

---

## Step 2: Run feature layer (watch mode)

```bash
BARTER_FEATURES_FILTER_INSTRUMENTS=BTCUSDT-PERP.BINANCE \
BARTER_FEATURES_INPUT_DIR=/tmp/nautilus_e2e/raw \
BARTER_FEATURES_OUTPUT_DIR=/tmp/nautilus_e2e/features \
BARTER_FEATURES_CHECKPOINT_DIR=/tmp/nautilus_e2e/checkpoints \
cargo run -p barter-features --release -- \
  --input /tmp/nautilus_e2e/raw \
  --output /tmp/nautilus_e2e/features \
  --mode watch
```

---

## Step 3: Verify Parquet outputs exist

```bash
find /tmp/nautilus_e2e/raw -type f -name "*.parquet" | head -20
find /tmp/nautilus_e2e/features -type f -name "*.parquet" | head -20
```

Expected directories:
- `/tmp/nautilus_e2e/raw/trades/`
- `/tmp/nautilus_e2e/raw/bars_1m/`
- `/tmp/nautilus_e2e/raw/extended_bars_1m/`
- `/tmp/nautilus_e2e/raw/order_book_deltas/` (if PARQUET_WRITE_L2=1)
- `/tmp/nautilus_e2e/features/tpo_brackets/`
- `/tmp/nautilus_e2e/features/large_trades/`
- `/tmp/nautilus_e2e/features/profile_events/`

---

## Step 4: Quick consistency checks (DuckDB)

```bash
/opt/homebrew/bin/duckdb -c "
SELECT COUNT(*) AS bars,
       MIN(ts_event) AS min_ts,
       MAX(ts_event) AS max_ts
FROM read_parquet('/tmp/nautilus_e2e/raw/extended_bars_1m/*/*/*.parquet');
"

/opt/homebrew/bin/duckdb -c "
SELECT label, COUNT(*) AS brackets
FROM read_parquet('/tmp/nautilus_e2e/features/tpo_brackets/*/*/*.parquet')
GROUP BY label ORDER BY label;
"
```

---

## Step 5: Nautilus integration (current status)

### 5.1 Bars + Trades (ready)
Use Nautilus ParquetDataCatalog to load:
- `/tmp/nautilus_e2e/raw/bars_1m`
- `/tmp/nautilus_e2e/raw/trades`

### 5.2 Extended bars + Features (ready once catalog is built)
Add CustomData classes in `packages/barter-nautilus-data`:
- `ExtendedBar` (already present)
- `TpoBracket`
- `LargeTrade`
- `ProfileEvent`

Use the catalog setup script to ingest custom data:
```
python3 scripts/validation/setup_nautilus_catalog.py \
  --source-dir /tmp/nautilus_e2e/raw \
  --catalog-dir /tmp/nautilus_catalog_barter --clean
```

The script now ingests:
- `raw/extended_bars_1m` → `data/custom_extended_bar1m`
- `features/tpo_brackets` → `data/custom_tpo_bracket`
- `features/large_trades` → `data/custom_large_trade`
- `features/profile_events` → `data/custom_profile_event`

---

## Acceptance Criteria
- No gaps in extended bars for test window
- TPO brackets exist and pass invariants (VAL ≤ POC ≤ VAH)
- Large trades file non-empty if big trades occur
- Nautilus loads bars/trades without schema errors
- (After CustomData classes) Nautilus loads extended bars + features

---

## Notes
- If Nautilus fails to load, verify precision mode matches collector (standard vs high).
- Ensure `BARTER_FEATURES_FILTER_INSTRUMENTS` is set so features match the collector filter.
