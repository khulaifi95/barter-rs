# Forward Test Runbook (Barter → Nautilus, UDS)
**Date:** 2026-02-06

## Goal
Run a real‑time forward test where Nautilus consumes live Barter UDS data and executes a strategy.

## Prereqs
- `barter-data-server` built (release recommended)
- Nautilus adapter built (`nautilus-barter`)
- UDS available (macOS/Linux)

## 1) Start Barter (UDS streaming)
```bash
export UDS_ENABLED=1
export UDS_PATH=/tmp/barter-data.sock
export PARQUET_ENABLED=0           # no disk writes
export PARQUET_WRITE_EXTENDED=true # needed for extended_bar_1m
export PARQUET_ASSETS=BTC
export PARQUET_VENUES=BINANCE
export RUST_LOG=info

/Users/screener-m3/projects/barter-rs/target/release/barter-data-server
```

## 2) Smoke‑check UDS kinds (coverage)
```bash
cd /Users/screener-m3/projects/nautilus_trader
BARTER_SMOKE_MODE=coverage \
BARTER_SMOKE_REQUIRED_KINDS=trade,order_book_l1,order_book_l2,candle_1m,extended_bar_1m \
BARTER_SMOKE_DURATION_SECS=70 \
BARTER_SMOKE_TIMEOUT_SECS=70 \
cargo run -p nautilus-barter --bin uds_smoke
```

Expected: `OK: coverage mode complete` and all required kinds present.

## 3) Live data tester (Rust)
This runs a Nautilus `DataTester` actor against the live UDS feed and auto‑stops
after 10 minutes by default.

```bash
cd /Users/screener-m3/projects/nautilus_trader
BARTER_TEST_SECS=600 \
cargo run --example node_data_tester --package nautilus-barter
```

Expected: live data events logged, then clean shutdown at ~10 minutes.

## 4) Live strategy (optional, future)
The adapter is Rust‑first; a minimal Python live example can be added later
to wire a `PyStrategy` into `LiveNode` for real‑time signal evaluation.

## Notes
- `candle_1m` and `extended_bar_1m` are emitted on minute boundaries.
- If you don’t see them, wait a full minute or increase the smoke duration.
- You can enable Parquet simultaneously by setting `PARQUET_ENABLED=1` and `PARQUET_OUTPUT_DIR=/tmp/...`.


## Latest validation (2026-02-06)
10‑minute live capture + Nautilus order‑book imbalance backtest on the captured L2 data.

### Capture run
```
TEST_DIR=/tmp/barter_forward_l2_20260206_082510
UDS_ENABLED=1 UDS_PATH=/tmp/barter-data.sock PARQUET_ENABLED=1 PARQUET_OUTPUT_DIR="$TEST_DIR" PARQUET_WRITE_L2=1 PARQUET_WRITE_EXTENDED=true PARQUET_ASSETS=BTC PARQUET_VENUES=BINANCE cargo run -p barter-data-server --release
```

### Live UDS tester
```
cd /Users/screener-m3/projects/nautilus_trader
BARTER_TEST_SECS=600 cargo run --example node_data_tester --package nautilus-barter
```

### Catalog + backtest
```
uv run python   /Users/screener-m3/projects/barter-rs/scripts/validation/setup_nautilus_catalog.py   --source-dir "$TEST_DIR"   --catalog-dir /tmp/nautilus_catalog_l2_forward   --clean

uv run python   /Users/screener-m3/projects/nautilus_trader/examples/backtest/crypto_orderbook_imbalance_parquet.py   --catalog-dir /tmp/nautilus_catalog_l2_forward   --instrument-id BTCUSDT-PERP.BINANCE
```

### Results (captured 10 minutes)
```
trades:            275,797 rows
bars_1m:           10 rows
extended_bars_1m:  10 rows
order_book_deltas: 587,431 rows
```

### Notes
- Backtest ran successfully but placed no trades in the 10‑minute window (strategy thresholds too conservative for short samples).
- Live UDS log: `/tmp/barter_forward_l2_20260206_082510/data_tester.log`
- Catalog: `/tmp/nautilus_catalog_l2_forward`


## Backtest trade simulation flags
Use these to force trades (or run signal‑only) against a captured catalog.

### Simulated trades (force activity)
```
uv run python   /Users/screener-m3/projects/nautilus_trader/examples/backtest/crypto_orderbook_imbalance_parquet.py   --catalog-dir /tmp/nautilus_catalog_l2_forward   --instrument-id BTCUSDT-PERP.BINANCE   --trigger-min-size 1   --trigger-imbalance-ratio 0.95   --min-seconds-between-triggers 0.1   --max-trade-size 0.01
```

### Dry‑run (signals only, no orders)
```
uv run python   /Users/screener-m3/projects/nautilus_trader/examples/backtest/crypto_orderbook_imbalance_parquet.py   --catalog-dir /tmp/nautilus_catalog_l2_forward   --instrument-id BTCUSDT-PERP.BINANCE   --dry-run
```

### When to use which
- Use **dry‑run** to validate signal logic and event flow without simulated fills.
- Use **simulated trades** to verify fills, commissions, and PnL wiring end‑to‑end.


## Context matrix (what runs where)

### Forward testing (real-time)
- **Required:** Barter data server + UDS stream + Nautilus live node (data client)
- **Not required:** Parquet, DuckDB, catalog
- **Purpose:** Validate live data flow and strategy execution

### Backtesting (historical)
- **Required:** Parquet files + catalog + Nautilus backtest
- **Not required:** UDS streaming
- **Purpose:** Fast replay, PnL/statistics

### QA / Verification (optional)
- **DuckDB** (or PyArrow) only used to validate row counts or sanity after capture.

## Tools and parameters used

### Barter (live capture + optional archive)
- `UDS_ENABLED=1`
- `UDS_PATH=/tmp/barter-data.sock`
- `PARQUET_ENABLED=1` (optional)
- `PARQUET_OUTPUT_DIR=/tmp/barter_forward_l2_...`
- `PARQUET_WRITE_L2=1`
- `PARQUET_WRITE_EXTENDED=true`
- `PARQUET_ASSETS=BTC`
- `PARQUET_VENUES=BINANCE`

### Nautilus (live)
- `cargo run --example node_data_tester --package nautilus-barter`
- `BARTER_TEST_SECS=600` (or 900 for 15m)

### Nautilus (backtest on captured data)
- `setup_nautilus_catalog.py --source-dir <capture> --catalog-dir <catalog> --clean`
- `crypto_orderbook_imbalance_parquet.py` with:
  - `--trigger-min-size`
  - `--trigger-imbalance-ratio`
  - `--min-seconds-between-triggers`
  - `--max-trade-size`
  - `--dry-run` (signal-only)
