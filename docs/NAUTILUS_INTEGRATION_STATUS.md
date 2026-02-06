# Nautilus Integration Status Report

**Date:** 2026-02-05
**Reporter:** Claude Opus 4.5
**Purpose:** Status update for architect/Codex on Nautilus compatibility

---

## Executive Summary

| Component | Status | Notes |
|-----------|--------|-------|
| **Schema Compatibility** | ✅ VERIFIED | 16-byte fixed-point matches Nautilus 1.222.0+ |
| **Trade Parquet** | ✅ VERIFIED | Decodes to `TradeTick` objects |
| **Bar Parquet** | ✅ VERIFIED | Decodes to `Bar` objects |
| **Extended Bar Parquet** | ⚠️ PARTIAL | Schema OK, CustomData class exists but needs catalog wiring |
| **Feature Parquet (TPO, etc.)** | ⚠️ PARTIAL | Schema OK, CustomData classes NOT YET IMPLEMENTED |
| **Full Backtest E2E** | 🔴 NOT TESTED | Existing script needs catalog setup first |

---

## Data Available for Testing

### Exchange/Instrument Coverage

| Exchange | Instrument | Data Type | Status |
|----------|------------|-----------|--------|
| **Binance** | BTCUSDT-PERP | Trades | ✅ Available |
| **Binance** | BTCUSDT-PERP | 1-min Bars | ✅ Available |
| **Binance** | BTCUSDT-PERP | Extended Bars (43 fields) | ✅ Available |
| **Binance** | BTCUSDT-PERP | TPO Brackets | ✅ Available |
| **Binance** | BTCUSDT-PERP | Large Trades | ✅ Available |
| **Binance** | BTCUSDT-PERP | Profile Events | ✅ Available |

**Note:** Currently **Binance-only**, **PERP-only**, **BTC-only**.

### Test Data Location

```
test_output/tpo_65min_2309/
├── raw/
│   ├── trades/BTCUSDT_PERP_BINANCE/          # 12.5M+ trades
│   ├── bars_1m/BTCUSDT_PERP_BINANCE_.../     # 1,833 bars
│   └── extended_bars_1m/BTCUSDT_PERP_.../    # 1,833 extended bars
└── features/BTCUSDT_PERP_BINANCE/
    ├── tpo_brackets.parquet                   # TPO analysis
    ├── large_trades.parquet                   # Whale trades
    └── profile_events.parquet                 # Market events
```

---

## Schema Verification Results

### Trades Schema
```
Columns: [price, size, aggressor_side, trade_id, ts_event, ts_init]
Types:   [fixed_size_binary[16], fixed_size_binary[16], uint8, string, uint64, uint64]
Metadata: instrument_id, price_precision, size_precision
Status:  ✅ MATCHES NAUTILUS EXPECTED SCHEMA
```

### Bars Schema
```
Columns: [open, high, low, close, volume, ts_event, ts_init]
Types:   [fixed_size_binary[16] x5, uint64, uint64]
Metadata: bar_type, instrument_id, price_precision, size_precision
Status:  ✅ MATCHES NAUTILUS EXPECTED SCHEMA
```

### Encoding Verification
```
Precision Mode: HIGH (16-byte, 1e16 scalar)
Byte Order:     Little-endian
Sample Decode:  $73,046.30 (valid BTC price range)
Status:         ✅ MATCHES NAUTILUS RUST CORE
```

---

## What Has Been Tested

### ✅ Verified Working

1. **Schema validation** (`scripts/validation/test_nautilus_load.py`)
   - Column names, types, order
   - Metadata presence
   - Fixed-point decoding

2. **Manual object creation** (tested today)
   - `TradeTick` objects from barter parquet: **WORKS**
   - `Bar` objects from barter parquet: **WORKS**
   - `Price.from_raw()` / `Quantity.from_raw()`: **WORKS**

3. **Nautilus imports and types**
   - All required classes importable
   - Type conversions work correctly

### 🔴 NOT YET Tested

1. **Full BacktestEngine run with barter data**
   - Existing script: `scripts/validation/run_nautilus_backtest.py`
   - Requires: Nautilus catalog setup first
   - Blocker: Need to run `setup_nautilus_catalog.py`

2. **Extended Bar CustomData in Nautilus**
   - Class exists: `packages/barter-nautilus-data/barter_nautilus_data/extended_bars.py`
   - NOT tested in actual backtest

3. **Feature data (TPO, Large Trades, Events)**
   - CustomData classes: **NOT IMPLEMENTED**
   - Parquet files: Generated and valid
   - Nautilus integration: **PENDING**

---

## Existing Validation Infrastructure

### Scripts Available

| Script | Purpose | Status |
|--------|---------|--------|
| `test_nautilus_load.py` | Schema validation | ✅ Ready |
| `setup_nautilus_catalog.py` | Create Nautilus catalog | ✅ Ready |
| `run_nautilus_backtest.py` | Full E2E backtest | ⚠️ Needs catalog |
| `validate_parquet.py` | Data quality checks | ✅ Ready |
| `validate_features.py` | Feature invariants | ✅ Ready |

### Python Package

```
packages/barter-nautilus-data/
├── barter_nautilus_data/
│   ├── __init__.py          # Package exports
│   ├── extended_bars.py     # ExtendedBar1m CustomData class
│   ├── schemas.py           # Decoding helpers
│   └── registry.py          # Arrow serializer registration
└── tests/
    └── test_extended_bars_schema.py
```

**Install:** `uv pip install -e packages/barter-nautilus-data`

---

## Missing Pieces for Full E2E

### 1. Nautilus Catalog Setup
```bash
# This creates the catalog structure Nautilus expects
python scripts/validation/setup_nautilus_catalog.py \
  --source /path/to/test_output/tpo_65min_2309/raw \
  --dest /tmp/nautilus_catalog
```

### 2. Feature CustomData Classes (NOT IMPLEMENTED)

Need to create in `packages/barter-nautilus-data/`:

```python
# tpo_brackets.py - TpoBracket CustomData
# large_trades.py - LargeTrade CustomData
# profile_events.py - ProfileEvent CustomData
```

### 3. Full Backtest Run
```bash
python scripts/validation/run_nautilus_backtest.py \
  --catalog-dir /tmp/nautilus_catalog
```

---

## Recommended Next Steps

### Option A: Minimal E2E Test (1-2 hours)
1. Run `setup_nautilus_catalog.py` to create catalog
2. Run `run_nautilus_backtest.py` to verify bars/trades work
3. Document results

### Option B: Full Feature Integration (4-6 hours)
1. Implement `TpoBracket` CustomData class
2. Implement `LargeTrade` CustomData class
3. Implement `ProfileEvent` CustomData class
4. Register all in Nautilus Arrow serializer
5. Run full backtest with features

### Option C: Defer Until Needed
- Current Python integration works for bars/trades
- Features can be loaded separately via pyarrow
- Full Nautilus integration when backtesting actually needs it

---

## Questions for Architect

1. **Priority:** Is full Nautilus E2E test blocking something?

2. **Scope:** Should we test with bars/trades only, or wait until feature CustomData classes are done?

3. **Exchange expansion:** Stay Binance-only or add other exchanges?

4. **Instrument expansion:** Add ETH, SOL, or stay BTC-only for now?

---

## Files Referenced

- `docs/NAUTILUS_E2E_FEATURES_TEST.md` - E2E test procedure
- `docs/VALIDATION_HANDOFF.md` - Collector validation checklist
- `scripts/validation/run_nautilus_backtest.py` - Backtest script
- `scripts/validation/setup_nautilus_catalog.py` - Catalog setup
- `packages/barter-nautilus-data/` - Python integration package
