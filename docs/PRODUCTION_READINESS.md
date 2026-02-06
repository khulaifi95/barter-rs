# barter-features Production Readiness Assessment

**Date:** 2026-02-03
**Status:** Ready for staging/testing, needs validation before production

---

## 1. Data Flow Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           barter-data-server (Collector)                     │
│                                                                             │
│  WebSocket feeds → Aggregation → Atomic writes (.tmp → .parquet)           │
│                                                                             │
│  Output directories:                                                        │
│    /data/raw/{instrument}/{date}/extended_bars_1m.parquet                  │
│    /data/raw/{instrument}/{date}/trades.parquet                            │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                           barter-features (This Crate)                       │
│                                                                             │
│  Modes:                                                                     │
│    BATCH: Scan all existing files → process → exit                         │
│    WATCH: Process existing → watch for new files → continuous              │
│                                                                             │
│  Processing:                                                                │
│    1. File watcher detects stable .parquet files (mtime > 2s)              │
│    2. Read parquet → decode Int64×1e9 values                               │
│    3. Accumulate bars per (instrument, session_date)                        │
│    4. Compute TPO brackets, volume profile, events                         │
│    5. Atomic write output files                                            │
│                                                                             │
│  Output directories:                                                        │
│    /data/features/{instrument}/{date}/tpo_brackets.parquet                 │
│    /data/features/{instrument}/{date}/profile_events.parquet               │
│    /data/features/{instrument}/{date}/large_trades.parquet                 │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                           Downstream Consumers                              │
│                                                                             │
│  - Nautilus Trader (backtesting)                                           │
│  - TradingView / visualization                                              │
│  - ML pipelines                                                             │
│  - Real-time dashboards                                                    │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Data Storage Format

### Input: Extended Bars (from collector)
| Column | Type | Description |
|--------|------|-------------|
| ts_event | UInt64 | Bar close time (nanos) |
| ts_init | UInt64 | Ingest time (nanos) |
| ts_open | UInt64 | Bar open time (nanos) |
| open/high/low/close | Int64 | Price × 1e9 |
| volume | Int64 | Volume × 1e9 |
| buy_volume/sell_volume/delta | Int64 | × 1e9 |
| ... | ... | 43 columns total |

### Input: Trades (from collector)
| Column | Type | Description |
|--------|------|-------------|
| price | FixedSizeBinary(8/16) | Price (auto-decoded) |
| size | FixedSizeBinary(8/16) | Size (auto-decoded) |
| aggressor_side | UInt8 | 0=unknown, 1=buy, 2=sell |
| trade_id | Utf8 | Exchange trade ID |
| ts_event | UInt64 | Trade time (nanos) |
| ts_init | UInt64 | Ingest time (nanos) |

### Output: TPO Brackets
| Column | Type | Description |
|--------|------|-------------|
| ts_event | Int64 | Bracket end time (nanos) |
| schema_version | Utf8 | "1.2.0" |
| config_hash | Utf8 | For reproducibility |
| source_precision | Utf8 | Always "standard" |
| output_precision | Int32 | Always 9 (×1e9) |
| label | Utf8 | "A"-"Z", "AA"-"AV" |
| bracket_index | UInt8 | 0-47 |
| vol_poc/vol_vah/vol_val | Int64 | Price × 1e9 |
| ib_high/ib_low | Int64 | Price × 1e9 |
| ... | ... | 37 columns total |

### Output: Large Trades
| Column | Type | Description |
|--------|------|-------------|
| ts_event | Int64 | Trade time (nanos) |
| price | Int64 | Price × 1e9 |
| size | Int64 | Size × 1e9 |
| notional_usd | Int64 | USD value × 1e9 |
| category | Utf8 | "LARGE"/"WHALE"/"MEGA" |
| threshold_used | Int64 | Threshold × 1e9 |

---

## 3. Outstanding Issues Before Production

### Critical (Must Fix)

| Issue | Risk | Workaround |
|-------|------|------------|
| **No integration tests** | Regressions may go undetected | Manual validation required |
| **No graceful shutdown** | Data loss on SIGTERM | Use batch mode, restart at will |
| **Memory unbounded in watch mode** | OOM on long runs | Periodic restarts or batch mode |

### Important (Should Fix)

| Issue | Risk | Workaround |
|-------|------|------------|
| Gap config ignored | Hardcoded 2s tolerance | Acceptable for most cases |
| SessionAccumulator not persisted | Data loss on crash mid-session | Re-run batch mode on restart |
| No session timeout cleanup | Memory leak over time | Periodic restarts |

### Minor (Can Defer)

| Issue | Risk | Workaround |
|-------|------|------------|
| Percentile thresholds not implemented | Only absolute thresholds | $2M/$5M/$10M is reasonable |
| Session state not checkpointed | Must reprocess on crash | Batch mode is idempotent |

---

## 4. Potential Production Errors

### Error: "Missing column: X"
**Cause:** Input parquet schema doesn't match expected format.
**Fix:** Ensure collector is running with correct schema version.

### Error: "Parquet error: Invalid data"
**Cause:** Corrupted input file (collector crash during write).
**Fix:** Atomic writes should prevent this. If it happens, delete the file and let collector regenerate.

### Error: "IO error: No such file"
**Cause:** Race condition - file detected but moved/deleted before read.
**Fix:** Already handled - will skip and retry on next detection.

### Error: "Checkpoint error: config hash changed"
**Cause:** Config changed, checkpoint invalidated.
**Fix:** Expected behavior. All files will be reprocessed.

### Warning: "Gap detected: X bars missing"
**Cause:** Collector had network issues or missed bars.
**Impact:** POC/VA may be slightly off. Event logged for investigation.

### Memory Issues
**Cause:** `active_sessions` HashMap grows unbounded in watch mode.
**Fix:** Call `finalize_old_sessions()` periodically (not currently automated).

---

## 5. VPS Deployment Steps

### Prerequisites
```bash
# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup default stable

# Build release binary
cd /path/to/barter-rs
cargo build --release -p barter-features

# Binary location
ls -la target/release/barter-features
```

### Directory Structure
```bash
# Create directories
sudo mkdir -p /data/raw /data/features /data/_checkpoints
sudo chown -R $USER:$USER /data

# Verify collector output exists
ls /data/raw/*/
# Should see: BTCUSDT-PERP.BINANCE/2026-02-02/extended_bars_1m.parquet
```

### Configuration
```bash
# Option 1: Environment variables
export BARTER_FEATURES_INPUT_DIR=/data/raw
export BARTER_FEATURES_OUTPUT_DIR=/data/features
export BARTER_FEATURES_CHECKPOINT_DIR=/data/_checkpoints
export BARTER_FEATURES_MODE=batch

# Option 2: Config file
cat > /etc/barter-features/config.toml << 'EOF'
[general]
schema_version = "1.2.0"
input_dir = "/data/raw"
output_dir = "/data/features"
checkpoint_dir = "/data/_checkpoints"
mode = "watch"

[tpo]
bracket_minutes = 30
price_bucket_usd = 50.0
value_area_pct = 0.70

[large_trades]
threshold_mode = "absolute"
large_threshold_usd = 2000000.0
whale_threshold_usd = 5000000.0
mega_threshold_usd = 10000000.0

[checkpoint]
enabled = true
save_interval_secs = 30
EOF
```

### Running

```bash
# Batch mode (recommended for initial deployment)
./target/release/barter-features \
  --input /data/raw \
  --output /data/features \
  --mode batch \
  --log-level info

# Watch mode (continuous)
./target/release/barter-features \
  --input /data/raw \
  --output /data/features \
  --mode watch \
  --log-level info
```

### Systemd Service (Optional)
```ini
# /etc/systemd/system/barter-features.service
[Unit]
Description=Barter Features Processor
After=network.target

[Service]
Type=simple
User=barter
WorkingDirectory=/opt/barter
ExecStart=/opt/barter/barter-features --input /data/raw --output /data/features --mode batch
Restart=on-failure
RestartSec=30
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
```

---

## 6. Integration Testing

### Option A: Python Validation Script (Recommended)

Create `scripts/validation/validate_features.py`:

```python
#!/usr/bin/env python3
"""
Validate barter-features output against expected values.

Usage:
  python scripts/validation/validate_features.py /data/features/BTCUSDT-PERP.BINANCE/2026-02-02/
"""

import sys
import pyarrow.parquet as pq
import numpy as np

def validate_tpo_brackets(path):
    """Validate TPO brackets output."""
    table = pq.read_table(path)

    errors = []

    # Check required columns
    required = ["ts_event", "label", "vol_poc", "vol_vah", "vol_val",
                "ib_high", "ib_low", "source_precision", "output_precision"]
    for col in required:
        if col not in table.schema.names:
            errors.append(f"Missing column: {col}")

    if errors:
        return errors

    # Decode values (÷1e9)
    poc = table.column("vol_poc").to_pylist()
    vah = table.column("vol_vah").to_pylist()
    val = table.column("vol_val").to_pylist()

    poc = [v / 1e9 for v in poc]
    vah = [v / 1e9 for v in vah]
    val = [v / 1e9 for v in val]

    # Validate relationships
    for i in range(len(poc)):
        if not (val[i] <= poc[i] <= vah[i]):
            errors.append(f"Row {i}: VAL <= POC <= VAH violated: {val[i]} <= {poc[i]} <= {vah[i]}")

    # Check labels are sequential
    labels = table.column("label").to_pylist()
    expected_labels = ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J",
                       "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T",
                       "U", "V", "W", "X", "Y", "Z", "AA", "AB", "AC", "AD",
                       "AE", "AF", "AG", "AH", "AI", "AJ", "AK", "AL", "AM", "AN",
                       "AO", "AP", "AQ", "AR", "AS", "AT", "AU", "AV"]
    for i, label in enumerate(labels):
        if label != expected_labels[i]:
            errors.append(f"Row {i}: Expected label {expected_labels[i]}, got {label}")
            break  # Only report first mismatch

    # Check metadata
    source_prec = table.column("source_precision").to_pylist()[0]
    if source_prec != "standard":
        errors.append(f"source_precision should be 'standard', got '{source_prec}'")

    output_prec = table.column("output_precision").to_pylist()[0]
    if output_prec != 9:
        errors.append(f"output_precision should be 9, got {output_prec}")

    return errors

def validate_large_trades(path):
    """Validate large trades output."""
    try:
        table = pq.read_table(path)
    except FileNotFoundError:
        return ["File not found (may be expected if no large trades)"]

    errors = []

    # Check thresholds
    notional = table.column("notional_usd").to_pylist()
    notional = [v / 1e9 for v in notional]  # Decode

    categories = table.column("category").to_pylist()

    for i, (n, cat) in enumerate(zip(notional, categories)):
        if cat == "LARGE" and not (2_000_000 <= n < 5_000_000):
            errors.append(f"Row {i}: LARGE trade has notional {n}, expected 2M-5M")
        elif cat == "WHALE" and not (5_000_000 <= n < 10_000_000):
            errors.append(f"Row {i}: WHALE trade has notional {n}, expected 5M-10M")
        elif cat == "MEGA" and n < 10_000_000:
            errors.append(f"Row {i}: MEGA trade has notional {n}, expected 10M+")

    return errors

def main():
    if len(sys.argv) != 2:
        print("Usage: validate_features.py <output_dir>")
        return 2

    output_dir = sys.argv[1]
    all_errors = []

    # Validate TPO brackets
    tpo_path = f"{output_dir}/tpo_brackets.parquet"
    print(f"Validating {tpo_path}...")
    errors = validate_tpo_brackets(tpo_path)
    if errors:
        all_errors.extend([f"TPO: {e}" for e in errors])
    else:
        print("  TPO brackets: OK")

    # Validate large trades
    trades_path = f"{output_dir}/large_trades.parquet"
    print(f"Validating {trades_path}...")
    errors = validate_large_trades(trades_path)
    if errors and "File not found" not in errors[0]:
        all_errors.extend([f"Trades: {e}" for e in errors])
    else:
        print("  Large trades: OK (or no trades)")

    if all_errors:
        print("\nValidation FAILED:")
        for e in all_errors[:10]:  # Limit output
            print(f"  - {e}")
        if len(all_errors) > 10:
            print(f"  ... and {len(all_errors) - 10} more errors")
        return 1

    print("\nValidation PASSED")
    return 0

if __name__ == "__main__":
    sys.exit(main())
```

### Option B: Rust Integration Test

Create `barter-features/tests/integration.rs`:

```rust
//! Integration tests for barter-features.
//!
//! Run with: cargo test --test integration -- --ignored

use barter_features::{Config, Pipeline};
use std::path::PathBuf;

#[tokio::test]
#[ignore] // Run manually with real data
async fn test_batch_processing() {
    // Requires: /tmp/test_data/raw/{instrument}/{date}/extended_bars_1m.parquet
    let input_dir = PathBuf::from("/tmp/test_data/raw");
    let output_dir = PathBuf::from("/tmp/test_data/features");

    if !input_dir.exists() {
        eprintln!("Test data not found at {:?}", input_dir);
        return;
    }

    let mut config = Config::from_env().unwrap();
    config.general.input_dir = input_dir;
    config.general.output_dir = output_dir.clone();
    config.checkpoint.enabled = false;

    let mut pipeline = Pipeline::new(config).unwrap();
    pipeline.run_batch().await.unwrap();

    // Verify output exists
    let output_files: Vec<_> = std::fs::read_dir(&output_dir)
        .unwrap()
        .flatten()
        .collect();

    assert!(!output_files.is_empty(), "No output files generated");
}
```

### Running Integration Tests

```bash
# 1. Copy some real collector data to test directory
mkdir -p /tmp/test_data/raw/BTCUSDT-PERP.BINANCE/2026-02-02
cp /data/raw/BTCUSDT-PERP.BINANCE/2026-02-02/extended_bars_1m.parquet \
   /tmp/test_data/raw/BTCUSDT-PERP.BINANCE/2026-02-02/

# 2. Run barter-features in batch mode
cargo run --release -p barter-features -- \
  --input /tmp/test_data/raw \
  --output /tmp/test_data/features \
  --mode batch

# 3. Validate output
python scripts/validation/validate_features.py \
  /tmp/test_data/features/BTCUSDT-PERP.BINANCE/2026-02-02/

# 4. Inspect output manually
python -c "
import pyarrow.parquet as pq
t = pq.read_table('/tmp/test_data/features/BTCUSDT-PERP.BINANCE/2026-02-02/tpo_brackets.parquet')
print(t.schema)
print(t.to_pandas().head())
"
```

---

## 7. Monitoring Checklist

### Health Checks
- [ ] Process is running: `pgrep barter-features`
- [ ] Output files being created: `find /data/features -mmin -5 -name "*.parquet"`
- [ ] Checkpoint file updating: `stat /data/_checkpoints/features_state.json`
- [ ] No error logs: `journalctl -u barter-features --since "5 min ago" | grep -i error`

### Data Quality Checks
- [ ] POC is within daily range (not obviously wrong)
- [ ] Volume totals match collector bars
- [ ] No duplicate brackets for same session
- [ ] Labels are sequential (A, B, C, ...)

### Performance Metrics
- [ ] Processing latency < 1s per file
- [ ] Memory usage stable (no growth over time)
- [ ] Disk usage growing as expected

---

## 8. Recommended Deployment Strategy

### Phase 1: Batch Mode Testing (1-2 days)
1. Deploy in batch mode only
2. Run once daily via cron
3. Manually validate output each day
4. Fix any issues found

### Phase 2: Watch Mode Staging (3-5 days)
1. Enable watch mode on staging VPS
2. Monitor memory usage and logs
3. Implement graceful shutdown if needed
4. Add session cleanup timer

### Phase 3: Production (ongoing)
1. Deploy to production VPS
2. Set up monitoring alerts
3. Schedule periodic restarts (e.g., weekly)
4. Document runbook for common issues

---

## 9. Quick Reference

```bash
# Build
cargo build --release -p barter-features

# Test
cargo test -p barter-features

# Run batch (one-shot)
./target/release/barter-features --mode batch --input /data/raw --output /data/features

# Run watch (continuous)
./target/release/barter-features --mode watch --input /data/raw --output /data/features

# Check output
python -c "import pyarrow.parquet as pq; print(pq.read_table('path/to/tpo_brackets.parquet').to_pandas())"

# Validate
python scripts/validation/validate_features.py /data/features/BTCUSDT-PERP.BINANCE/2026-02-02/
```
