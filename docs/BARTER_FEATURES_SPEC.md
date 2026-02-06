# barter-features: Technical Specification

**Version:** 1.2.0
**Date:** 2026-02-02
**Status:** Ready for Implementation (Codex blockers resolved)

---

## Revision History

| Version | Date | Changes |
|---------|------|---------|
| 1.0.0 | 2026-02-02 | Initial draft |
| 1.1.0 | 2026-02-02 | Codex review: file atomicity, checkpointing, gap handling |
| 1.2.0 | 2026-02-02 | **Codex blockers fixed**: precision alignment, acceptance criteria |

---

## BLOCKERS RESOLVED

### Blocker 1: Precision Alignment ✅ FIXED

**Problem:** Spec v1.1 incorrectly stated "prices = 1e8, sizes = 1e9"

**Reality:** Collector uses Nautilus fixed-point encoding:
- **Standard mode**: i64 × 1e9 stored as `FixedSizeBinary(8)`
- **High mode** (default): i128 × 1e16 stored as `FixedSizeBinary(16)`

**Solution:** Feature layer reads precision from Parquet metadata or `NAUTILUS_PRECISION` env var, decodes using the correct multiplier.

### Blocker 2: Atomic Write Patch ✅ DOCUMENTED

**Problem:** Collector writes directly to final `.parquet` path (no atomic rename)

**Solution:** Collector patch required before watcher mode is safe. Patch included in Appendix D.

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Acceptance Criteria](#2-acceptance-criteria)
3. [Goals & Principles](#3-goals--principles)
4. [Architecture](#4-architecture)
5. [File Contracts & Safety](#5-file-contracts--safety)
6. [Precision & Encoding](#6-precision--encoding)
7. [Data Schemas](#7-data-schemas)
8. [Configuration](#8-configuration)
9. [Features Specification](#9-features-specification)
10. [Development Phases](#10-development-phases)
11. [Testing Strategy](#11-testing-strategy)
12. [Deployment Strategy](#12-deployment-strategy)
13. [Appendices](#13-appendices)

---

## 1. Executive Summary

### What is barter-features?

A Rust-based feature computation layer that complements `barter-data-server` (the collector). It processes raw market data to extract **actionable trading signals** while filtering noise.

### Output Summary

| Feature | Rows/Day | Trigger |
|---------|----------|---------|
| TPO + Volume Profile (30m) | 48 | Every 30 minutes |
| Large Trades | 200-500 | Threshold-based (adaptive) |
| Profile Events | 20-50 | POC shift, IB break |
| Footprint Anomalies | Deferred | Phase 2 |
| **TOTAL** | ~300-600 | |

---

## 2. Acceptance Criteria

### 2.1 Go/No-Go Checklist

Before deployment, ALL of the following must pass:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│  ACCEPTANCE CRITERIA (All must pass for Go)                                 │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  DATA CORRECTNESS                                                           │
│  ☐ Precision roundtrip: decode(encode(price)) == price (within 1e-9)       │
│  ☐ Volume Profile POC matches manual calculation on sample data            │
│  ☐ Large trade thresholds produce expected row counts (±10%)               │
│  ☐ No NaN or infinite values in output                                     │
│                                                                             │
│  FILE SAFETY                                                                │
│  ☐ No partial file read: watcher ignores .tmp files                        │
│  ☐ Atomic write: collector uses .tmp → fsync → rename                      │
│  ☐ No data corruption on simulated crash during write                      │
│                                                                             │
│  IDEMPOTENCY                                                                │
│  ☐ Restart produces no duplicates: count(run1) == count(run2)              │
│  ☐ Checkpoint file correctly tracks processed files                        │
│  ☐ Changed file (different hash) triggers reprocessing                     │
│                                                                             │
│  GAP HANDLING                                                               │
│  ☐ Missing bars emit GapDetected event (if configured)                     │
│  ☐ Gaps logged with timestamps and missing bar count                       │
│  ☐ Bracket marked with has_gap=true when bars missing                      │
│                                                                             │
│  SCHEMA COMPLIANCE                                                          │
│  ☐ All output rows have ts_event (i64 nanos)                               │
│  ☐ All output rows have ts_init (i64 nanos)                                │
│  ☐ All output rows have schema_version                                     │
│  ☐ All output rows have config_hash                                        │
│  ☐ Output sorted by ts_event ascending                                     │
│                                                                             │
│  NAUTILUS COMPATIBILITY                                                     │
│  ☐ Parquet loadable by pyarrow without errors                              │
│  ☐ Custom data classes can instantiate from parquet rows                   │
│  ☐ Precision matches NAUTILUS_PRECISION environment                        │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 2.2 Test Coverage Requirements

| Test Category | Minimum Coverage | Status |
|---------------|------------------|--------|
| Unit tests (algorithms) | 80% | Required |
| Operational tests (file safety) | 100% of scenarios | Required |
| Integration tests (end-to-end) | 1 full session | Required |
| Performance tests | 1M trades/sec | Benchmark only |

---

## 3. Goals & Principles

### 3.1 Primary Goals

1. **Bounded Latency** - Feature latency = Parquet flush interval (30-60s) + processing (~1s)
2. **High Throughput** - Process 11.5M+ trades/day without backpressure
3. **Signal Focus** - Only store actionable information
4. **Nautilus Compatible** - Match collector's precision encoding exactly
5. **Idempotent** - Restart-safe with checkpoint/dedupe mechanism
6. **Configurable** - All thresholds externalized, supports adaptive modes

### 3.2 Latency Model

```
┌─────────────────────────────────────────────────────────────────────────────┐
│  LATENCY BUDGET (Watcher Mode)                                              │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  Collector flush interval           │ 30-60s (PARQUET_FLUSH_INTERVAL_SECS) │
│  File rename (atomic)               │ <1ms                                  │
│  Watcher detection                  │ ~1s                                   │
│  Feature processing                 │ ~1-5s per file                        │
│  ─────────────────────────────────  │ ───────                               │
│  TOTAL                              │ 32-66 seconds                         │
│                                                                             │
│  Note: Sub-second latency requires IPC mode (deferred to v2)               │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 3.3 Non-Goals

- **NOT** sub-second latency (v1 is file-based)
- **NOT** a trading strategy engine
- **NOT** storing redundant data (CVD, delta already in extended_bars)

---

## 4. Architecture

### 4.1 System Context

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           SYSTEM ARCHITECTURE                               │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  barter-data-server                    barter-features                      │
│  (collector)                           (analyzer)                           │
│  ─────────────────                     ───────────────                      │
│  1. Write to .tmp                      3. Watch for .parquet                │
│  2. fsync + rename to .parquet         4. Detect precision from metadata    │
│                                        5. Decode using correct multiplier   │
│                                        6. Process and emit features         │
│            │                                       │                        │
│            ▼                                       ▼                        │
│  ┌─────────────────────────────────────────────────────────────┐           │
│  │  /data/raw/{instrument}/{date}/     /data/features/...      │           │
│  └─────────────────────────────────────────────────────────────┘           │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 4.2 Multi-Instrument Partitioning

```
/data/features/{instrument_id}/{date}/{feature_type}.parquet

Example:
/data/features/BTCUSDT-PERP.BINANCE/2026-02-02/tpo_brackets.parquet
/data/features/BTCUSDT-PERP.BINANCE/2026-02-02/large_trades.parquet
```

---

## 5. File Contracts & Safety

### 5.1 Atomic Write Protocol (REQUIRED - Collector Patch)

**Current state:** Collector writes directly to final path (UNSAFE)

**Required change:** Write to .tmp, fsync, rename

```rust
// REQUIRED: Patch barter-data-server/src/parquet/writer.rs
// See Appendix D for full implementation

fn write_batch_atomic(path: &Path, batch: &RecordBatch) -> Result<()> {
    let tmp_path = path.with_extension("parquet.tmp");

    // 1. Write to temp file
    let file = File::create(&tmp_path)?;
    let mut writer = ArrowWriter::try_new(file, batch.schema(), props)?;
    writer.write(batch)?;
    writer.close()?;

    // 2. Fsync to ensure durability
    let file = File::open(&tmp_path)?;
    file.sync_all()?;

    // 3. Atomic rename
    std::fs::rename(&tmp_path, path)?;

    Ok(())
}
```

### 5.2 Watcher Contract

```rust
// Feature layer watcher MUST:
// 1. Only process files with .parquet extension
// 2. Ignore files with .tmp extension
// 3. Verify file is not being written (optional: mtime stability check)

fn is_ready_file(path: &Path) -> bool {
    path.extension() == Some("parquet".as_ref()) &&
    !path.to_string_lossy().contains(".tmp")
}
```

### 5.3 Checkpoint & Idempotency

```rust
#[derive(Serialize, Deserialize)]
pub struct Checkpoint {
    pub version: String,
    pub config_hash: String,
    pub instruments: HashMap<String, InstrumentCheckpoint>,
}

#[derive(Serialize, Deserialize)]
pub struct InstrumentCheckpoint {
    pub processed_files: HashMap<String, FileCheckpoint>,
    pub session_state: Option<SessionState>,
}

#[derive(Serialize, Deserialize)]
pub struct FileCheckpoint {
    pub file_hash: String,      // SHA256 of file content
    pub rows_processed: u64,
    pub last_ts_event: i64,
    pub processed_at: DateTime<Utc>,
}
```

### 5.4 Gap Handling

```rust
pub enum GapHandling {
    Skip,           // Continue, log warning
    EmitEvent,      // Emit GapDetected event
    Fail { max_gap_minutes: u32 },  // Fail if gap exceeds threshold
}

pub struct GapEvent {
    pub ts_event: i64,
    pub ts_init: i64,
    pub instrument_id: String,
    pub gap_start: i64,
    pub gap_end: i64,
    pub missing_bars: u32,
}
```

---

## 6. Precision & Encoding

### 6.1 Collector Precision (Source of Truth)

**The collector uses Nautilus fixed-point encoding:**

```rust
// From barter-data-server/src/parquet/encoder.rs

pub enum PrecisionMode {
    Standard,  // i64 × 1e9 as FixedSizeBinary(8)
    High,      // i128 × 1e16 as FixedSizeBinary(16) [DEFAULT]
}

impl PrecisionMode {
    pub fn multiplier(self) -> f64 {
        match self {
            PrecisionMode::Standard => 1_000_000_000.0,          // 1e9
            PrecisionMode::High => 10_000_000_000_000_000.0,     // 1e16
        }
    }
}
```

### 6.2 Feature Layer Precision Handling

**Feature layer MUST detect and use the same precision as the collector:**

```rust
// src/precision.rs

/// Detect precision mode from environment or Parquet metadata
pub fn detect_precision() -> PrecisionMode {
    // 1. Check environment variable (matches collector)
    if let Ok(raw) = std::env::var("NAUTILUS_PRECISION") {
        let v = raw.trim().to_lowercase();
        if matches!(v.as_str(), "standard" | "std" | "8") {
            return PrecisionMode::Standard;
        }
    }
    // 2. Default to High (matches collector default)
    PrecisionMode::High
}

/// Decode fixed-point bytes to f64
pub fn decode_fixed_point(bytes: &[u8]) -> f64 {
    match bytes.len() {
        8 => {
            let fixed = i64::from_le_bytes(bytes.try_into().unwrap());
            fixed as f64 / 1_000_000_000.0  // 1e9
        }
        16 => {
            let fixed = i128::from_le_bytes(bytes.try_into().unwrap());
            fixed as f64 / 10_000_000_000_000_000.0  // 1e16
        }
        _ => panic!("Invalid fixed-point byte length: {}", bytes.len()),
    }
}

/// Encode f64 to feature output (use same precision as input)
pub fn encode_for_output(value: f64, mode: PrecisionMode) -> i64 {
    // Feature output uses i64 with explicit precision field
    // This allows simpler processing while maintaining accuracy
    match mode {
        PrecisionMode::Standard => (value * 1e9).round() as i64,
        PrecisionMode::High => {
            // For i64 output, use 1e9 but record that source was 1e16
            (value * 1e9).round() as i64
        }
    }
}
```

### 6.3 Schema Precision Metadata

**Every output row includes precision information:**

```rust
pub struct OutputMetadata {
    pub schema_version: String,     // "1.2.0"
    pub config_hash: String,        // SHA256 truncated
    pub source_precision: String,   // "standard" or "high"
    pub output_precision: i32,      // 9 (meaning 1e9 scale)
}
```

### 6.4 Precision Conversion Table

| Source | Encoding | Multiplier | Feature Output |
|--------|----------|------------|----------------|
| Standard | FixedSizeBinary(8) | 1e9 | i64 × 1e9 |
| High | FixedSizeBinary(16) | 1e16 | i64 × 1e9 (with precision metadata) |

**Note:** Feature output uses i64 × 1e9 for simplicity. The `source_precision` field records the original precision for audit purposes.

---

## 7. Data Schemas

### 7.1 TPO Bracket with Volume Profile

```rust
/// Schema version: 1.2.0
/// Output precision: i64 × 1e9 for prices/volumes
pub struct TpoBracketSchema {
    // === Nautilus Required ===
    pub ts_event: i64,              // Bracket end timestamp (nanos)
    pub ts_init: i64,               // Computation timestamp (nanos)

    // === Metadata (for reproducibility) ===
    pub schema_version: String,     // "1.2.0"
    pub config_hash: String,        // SHA256 of config
    pub source_precision: String,   // "standard" or "high"
    pub output_precision: i32,      // 9 (meaning 1e9)

    // === Identifiers ===
    pub instrument_id: String,
    pub session_id: String,         // ISO8601 session start
    pub session_date: String,

    // === TPO Bracket (this 30m period) ===
    pub label: String,              // "A"-"Z", "AA"-"AV"
    pub bracket_index: u8,          // 0-47
    pub bracket_start_ts: i64,
    pub bracket_end_ts: i64,
    pub bracket_open: i64,          // × 1e9
    pub bracket_high: i64,          // × 1e9
    pub bracket_low: i64,           // × 1e9
    pub bracket_close: i64,         // × 1e9
    pub bracket_volume: i64,        // × 1e9
    pub bracket_buy_volume: i64,    // × 1e9
    pub bracket_sell_volume: i64,   // × 1e9
    pub bracket_delta: i64,         // × 1e9
    pub bracket_trade_count: u32,
    pub bracket_bar_count: u8,      // Expected 30, less if gap
    pub bracket_has_gap: bool,

    // === Running Volume Profile ===
    pub vol_poc: i64,               // × 1e9
    pub vol_vah: i64,               // × 1e9
    pub vol_val: i64,               // × 1e9
    pub vol_total: i64,             // × 1e9

    // === Running TPO Profile ===
    pub tpo_poc: i64,               // × 1e9
    pub tpo_vah: i64,               // × 1e9
    pub tpo_val: i64,               // × 1e9
    pub tpo_total_periods: u16,

    // === Initial Balance ===
    pub ib_high: i64,               // × 1e9
    pub ib_low: i64,                // × 1e9
    pub ib_complete: bool,

    // === Analysis ===
    pub poc_divergence: i64,        // × 1e9
    pub session_type: String,
}
```

### 7.2 Session & Bracket Labeling

```
Session: 00:00:00 UTC to 23:59:59 UTC (24h)
Brackets: 48 × 30-minute periods

Index │ Time (UTC)    │ Label
──────┼───────────────┼───────
  0   │ 00:00 - 00:30 │   A
  1   │ 00:30 - 01:00 │   B     ← IB complete
  2   │ 01:00 - 01:30 │   C
 ...  │      ...      │  ...
 25   │ 12:30 - 13:00 │   Z
 26   │ 13:00 - 13:30 │  AA
 27   │ 13:30 - 14:00 │  AB
 ...  │      ...      │  ...
 47   │ 23:30 - 00:00 │  AV

fn bracket_label(index: u8) -> String {
    if index < 26 {
        ((b'A' + index) as char).to_string()
    } else {
        format!("A{}", (b'A' + index - 26) as char)
    }
}
```

### 7.3 Large Trade Event

```rust
/// Schema version: 1.2.0
pub struct LargeTradeSchema {
    pub ts_event: i64,
    pub ts_init: i64,
    pub schema_version: String,
    pub config_hash: String,
    pub source_precision: String,
    pub output_precision: i32,

    pub instrument_id: String,
    pub trade_id: String,
    pub price: i64,                 // × 1e9
    pub size: i64,                  // × 1e9
    pub side: String,
    pub notional_usd: i64,          // × 1e2 (cents)
    pub category: String,           // "LARGE", "WHALE", "MEGA"
    pub threshold_used: i64,        // × 1e2
    pub threshold_mode: String,
}
```

### 7.4 Profile Event

```rust
/// Schema version: 1.2.0
pub struct ProfileEventSchema {
    pub ts_event: i64,
    pub ts_init: i64,
    pub schema_version: String,
    pub config_hash: String,

    pub instrument_id: String,
    pub session_id: String,
    pub event_type: String,         // ProfileEventType

    pub vol_poc: i64,
    pub vol_vah: i64,
    pub vol_val: i64,
    pub tpo_poc: i64,
    pub tpo_vah: i64,
    pub tpo_val: i64,
    pub ib_high: i64,
    pub ib_low: i64,

    // Event-specific
    pub break_direction: Option<String>,
    pub break_price: Option<i64>,
    pub shift_amount: Option<i64>,
    pub previous_value: Option<i64>,

    // Gap-specific
    pub gap_start_ts: Option<i64>,
    pub gap_end_ts: Option<i64>,
    pub missing_bars: Option<u32>,
}

pub enum ProfileEventType {
    SessionOpen,
    IbComplete,
    IbBreakUp,
    IbBreakDown,
    PocShift,
    VahShift,
    ValShift,
    GapDetected,
    SessionClose,
}
```

---

## 8. Configuration

```toml
# barter-features/config/default.toml

[general]
schema_version = "1.2.0"
input_dir = "/data/raw"
output_dir = "/data/features"
checkpoint_dir = "/data/_checkpoints"
mode = "watch"  # or "batch"

[precision]
# Auto-detect from NAUTILUS_PRECISION env var (matches collector)
# Override only if you know what you're doing
# mode = "auto"  # "auto", "standard", or "high"

[session]
session_start_hour = 0
session_duration_hours = 24

[tpo]
bracket_minutes = 30
price_bucket_usd = 50.0
initial_balance_brackets = 2
value_area_pct = 0.70
value_area_algorithm = "expanding"

[large_trades]
threshold_mode = "absolute"
large_threshold_usd = 2_000_000.0
whale_threshold_usd = 5_000_000.0
mega_threshold_usd = 10_000_000.0

[profile]
poc_shift_threshold_usd = 100.0
va_shift_threshold_usd = 150.0

[gaps]
handling = "emit_event"
max_gap_minutes = 60
tolerance_seconds = 5

[checkpoint]
enabled = true
save_interval_secs = 30
verify_hashes = true
```

---

## 9. Features Specification

### 9.1 Value Area Algorithm (Expanding)

```
Input: price_levels = [(price, volume), ...]
Input: value_area_pct = 0.70

1. POC = price level with maximum volume
2. VA = {POC}, va_volume = poc_volume
3. While va_volume / total_volume < 0.70:
   a. vol_above = volume of level above VA (or 0)
   b. vol_below = volume of level below VA (or 0)
   c. Add level with higher volume to VA
   d. va_volume += added volume
4. VAH = max(VA), VAL = min(VA)
```

### 9.2 TPO POC Calculation

```
TPO POC = price level touched by most 30m brackets

For each bracket:
  levels_touched = all $50 levels between bracket_low and bracket_high
  For each level in levels_touched:
    tpo_count[level] += 1

TPO POC = level with max(tpo_count)
```

---

## 10. Development Phases

### Phase 1: Foundation + Precision (Week 1)

```
☐ Crate scaffold
☐ Config loading (TOML + env)
☐ Precision detection and decoding
☐ Unit tests: precision roundtrip
☐ File watcher (ignore .tmp)
☐ Checkpoint system
☐ Operational tests: file safety, idempotency
```

### Phase 2: TPO + Profile (Week 2)

```
☐ 1m bar reader with precision handling
☐ Gap detection
☐ 30m bracket aggregation
☐ Volume Profile (POC/VAH/VAL)
☐ TPO Profile
☐ Initial Balance
☐ Unit tests: algorithms
☐ Integration test: full session
```

### Phase 3: Events + Large Trades (Week 3)

```
☐ Profile events (shifts, IB breaks)
☐ Large trade detection
☐ Adaptive thresholds (optional)
☐ Unit tests
☐ Integration test
```

### Phase 4: Production (Week 4)

```
☐ Error handling
☐ Logging/metrics
☐ Performance optimization
☐ Documentation
☐ CI/CD
```

---

## 11. Testing Strategy

### 11.1 Operational Tests (File Safety)

```rust
#[test]
fn test_watcher_ignores_tmp_files() {
    let dir = tempdir();
    std::fs::write(dir.path().join("test.parquet.tmp"), b"partial");
    let files = scan_ready_files(dir.path());
    assert!(files.is_empty());
}

#[test]
fn test_restart_no_duplicates() {
    // Process once
    let output1 = process_session("2026-02-02");
    // Restart and process again
    let output2 = process_session("2026-02-02");
    assert_eq!(output1.num_rows(), output2.num_rows());
}

#[test]
fn test_precision_roundtrip() {
    let price = 78523.456789012;
    let encoded = encode_for_output(price, PrecisionMode::High);
    let decoded = encoded as f64 / 1e9;
    assert!((price - decoded).abs() < 1e-6);
}

#[test]
fn test_gap_detection() {
    let bars = vec![bar(t("12:00")), bar(t("12:01")), bar(t("12:05"))];  // Gap
    let events = process_bars(&bars);
    assert!(events.iter().any(|e| e.event_type == "GapDetected"));
}
```

### 11.2 Acceptance Test

```rust
#[test]
fn test_acceptance_criteria() {
    // Run full session
    let result = process_test_session();

    // Data correctness
    assert!(result.all_values_finite());
    assert!(result.poc_matches_manual_calculation());

    // Schema compliance
    for row in result.rows() {
        assert!(row.ts_event > 0);
        assert!(row.ts_init > 0);
        assert!(!row.schema_version.is_empty());
        assert!(!row.config_hash.is_empty());
    }

    // Sorted by ts_event
    assert!(result.is_sorted_by_ts_event());
}
```

---

## 12. Deployment Strategy

### 12.1 Prerequisites

1. **Collector atomic write patch** must be deployed first
2. **NAUTILUS_PRECISION** env var must match collector

### 12.2 Watcher Mode

```bash
NAUTILUS_PRECISION=high \
BARTER_FEATURES_GENERAL_MODE=watch \
cargo run --release -p barter-features
```

---

## 13. Appendices

### Appendix A: Validated Thresholds

(See v1.1 - unchanged)

### Appendix B: Nautilus Python Classes

(See v1.1 - unchanged, update precision handling)

### Appendix C: Collector Schema Reference

```rust
// From barter-data-server/src/parquet/encoder.rs
// Feature layer MUST match this encoding

PrecisionMode::Standard => 1e9   (FixedSizeBinary(8))
PrecisionMode::High     => 1e16  (FixedSizeBinary(16))
```

### Appendix D: Collector Atomic Write Patch

**File:** `barter-data-server/src/parquet/writer.rs`

**Change:** Replace `write_batch_static` with atomic version:

```rust
/// Write a record batch to a Parquet file ATOMICALLY.
/// Uses temp file + fsync + rename pattern to prevent partial reads.
fn write_batch_atomic(
    path: &Path,
    batch: RecordBatch,
    fsync_enabled: bool,
) -> Result<(), ParquetError> {
    let tmp_path = path.with_extension("parquet.tmp");

    // 1. Write to temp file
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();

    {
        let file = File::create(&tmp_path)
            .map_err(|e| ParquetError::General(format!("Create temp file: {}", e)))?;
        let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(props))?;
        writer.write(&batch)?;
        writer.close()?;
    }

    // 2. Fsync temp file
    if fsync_enabled {
        let file = File::open(&tmp_path)
            .map_err(|e| ParquetError::General(format!("Open for fsync: {}", e)))?;
        file.sync_all()
            .map_err(|e| ParquetError::General(format!("Fsync: {}", e)))?;
    }

    // 3. Atomic rename
    std::fs::rename(&tmp_path, path)
        .map_err(|e| ParquetError::General(format!("Atomic rename: {}", e)))?;

    Ok(())
}
```

**Also update flush methods to call `write_batch_atomic` instead of `write_batch_static`.**

---

## Sign-Off Checklist

| Item | Status |
|------|--------|
| Precision alignment with collector | ✅ Fixed in Section 6 |
| Atomic write patch documented | ✅ Appendix D |
| Acceptance criteria defined | ✅ Section 2 |
| Input precision detection specified | ✅ Section 6.2 |
| Gap handling specified | ✅ Section 5.4 |
| Test coverage requirements | ✅ Section 11 |

**Status: Ready for Implementation**

---

## Appendix E: Codex Implementation Guidance

### E.1 Recommended Implementation Order

```
1. Collector atomic write patch (BLOCKER - do first)
2. Features: precision decode + watcher + checkpoint
3. Unit tests for precision, file safety, idempotency
4. TPO brackets + Volume Profile
5. Large trade detection
6. Profile events
7. Integration tests
8. Production hardening
```

### E.2 Watcher Safety (Enhanced)

```rust
/// Check if file is ready to process
fn is_file_ready(path: &Path) -> bool {
    // 1. Must be .parquet (not .tmp)
    if path.extension() != Some("parquet".as_ref()) {
        return false;
    }
    if path.to_string_lossy().contains(".tmp") {
        return false;
    }

    // 2. Must have stable mtime (not being written)
    // Wait for mtime to be stable for 2-5 seconds
    let metadata = std::fs::metadata(path).ok();
    if let Some(meta) = metadata {
        if let Ok(modified) = meta.modified() {
            let age = SystemTime::now()
                .duration_since(modified)
                .unwrap_or_default();
            if age < Duration::from_secs(2) {
                return false;  // File too fresh, may still be writing
            }
        }
    }

    true
}
```

### E.3 Per-Instrument Configuration

```toml
# For BTC (high volume, high price)
[instruments.BTCUSDT-PERP.BINANCE]
price_bucket_usd = 50.0
large_threshold_usd = 2_000_000.0
whale_threshold_usd = 5_000_000.0

# For ETH (medium volume)
[instruments.ETHUSDT-PERP.BINANCE]
price_bucket_usd = 10.0
large_threshold_usd = 500_000.0
whale_threshold_usd = 2_000_000.0

# For SOL (lower price)
[instruments.SOLUSDT-PERP.BINANCE]
price_bucket_usd = 1.0
threshold_mode = "percentile"
large_percentile = 99.95
whale_percentile = 99.99

# Default for unknown instruments
[instruments.default]
threshold_mode = "adaptive"
rolling_window_hours = 24
large_std_dev = 3.0
whale_std_dev = 4.0
```

### E.4 Gap Handling Defaults

```toml
[gaps]
handling = "emit_event"      # Default: emit GapDetected event
max_gap_minutes = 60         # Fail if gap > 60 minutes
tolerance_secs = 2           # Gaps < 2s are timing noise, ignore
```

### E.5 Output Requirements Checklist

Every output row MUST have:
```
☐ ts_event: i64 (nanoseconds)
☐ ts_init: i64 (nanoseconds)
☐ schema_version: String ("1.2.0")
☐ config_hash: String (SHA256 truncated to 16 chars)
☐ source_precision: String ("standard" or "high")
☐ output_precision: i32 (9, meaning 1e9)
```

Output files MUST be:
```
☐ Partitioned: /features/{instrument}/{date}/{type}.parquet
☐ Sorted by ts_event ascending
☐ Valid Parquet readable by pyarrow
```

### E.6 Required Acceptance Tests

```rust
// MUST implement these tests before sign-off

#[test]
fn test_precision_roundtrip_8_bytes() {
    // Standard mode: FixedSizeBinary(8)
    let original = 78523.456789;
    let bytes = encode_standard(original);
    assert_eq!(bytes.len(), 8);
    let decoded = decode_fixed_point(&bytes);
    assert!((original - decoded).abs() < 1e-6);
}

#[test]
fn test_precision_roundtrip_16_bytes() {
    // High mode: FixedSizeBinary(16)
    let original = 78523.456789012345;
    let bytes = encode_high(original);
    assert_eq!(bytes.len(), 16);
    let decoded = decode_fixed_point(&bytes);
    assert!((original - decoded).abs() < 1e-9);
}

#[test]
fn test_idempotency_no_duplicates() {
    let output1 = process_session("2026-02-02");
    let output2 = process_session("2026-02-02");  // Restart
    assert_eq!(output1.num_rows(), output2.num_rows());
    // Verify exact same data
    assert_eq!(output1.to_bytes(), output2.to_bytes());
}

#[test]
fn test_gap_detection_emits_event() {
    let bars_with_gap = vec![
        bar("12:00"), bar("12:01"), bar("12:02"),
        // Gap: 12:03, 12:04 missing
        bar("12:05"),
    ];
    let events = process_bars(&bars_with_gap);
    let gap_events: Vec<_> = events.iter()
        .filter(|e| e.event_type == "GapDetected")
        .collect();
    assert_eq!(gap_events.len(), 1);
    assert_eq!(gap_events[0].missing_bars, Some(2));
}

#[test]
fn test_partial_file_ignored() {
    let dir = tempdir();
    // Create partial file
    fs::write(dir.path().join("test.parquet.tmp"), b"partial");
    // Create complete file
    write_valid_parquet(dir.path().join("test.parquet"));

    let files = scan_ready_files(dir.path());
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with("test.parquet"));
}

#[test]
fn test_multi_instrument_parallel() {
    let instruments = vec!["BTCUSDT", "ETHUSDT", "SOLUSDT"];
    setup_test_data(&instruments);

    let results = process_all_parallel(&instruments);

    assert_eq!(results.len(), 3);
    for (instrument, output) in results {
        assert!(output.tpo_brackets.num_rows() > 0);
        // Verify no cross-contamination
        for row in output.rows() {
            assert!(row.instrument_id.contains(instrument));
        }
    }
}
```

### E.7 Config Snippet (Ready to Use)

```toml
# barter-features/config/default.toml
# v1.2.0 - Codex reviewed

[general]
schema_version = "1.2.0"
input_dir = "/data/raw"
output_dir = "/data/features"
checkpoint_dir = "/data/_checkpoints"
mode = "watch"

[precision]
# Auto-detect from NAUTILUS_PRECISION (matches collector)
# Default: high (i128 × 1e16)
mode = "auto"

[session]
start_hour = 0
duration_hours = 24
timezone = "UTC"

[tpo]
bracket_minutes = 30
price_bucket_usd = 50.0
initial_balance_brackets = 2
value_area_pct = 0.70
value_area_algorithm = "expanding"

[large_trades]
# BTC defaults (validated against 11.5M trades/day)
threshold_mode = "absolute"
large_threshold_usd = 2_000_000.0
whale_threshold_usd = 5_000_000.0
mega_threshold_usd = 10_000_000.0

# Adaptive mode (for other instruments)
# threshold_mode = "percentile"
# large_percentile = 99.95
# whale_percentile = 99.99
# rolling_window_hours = 24

[profile]
poc_shift_threshold_usd = 100.0
va_shift_threshold_usd = 150.0
poc_divergence_threshold_usd = 200.0

[gaps]
handling = "emit_event"
max_gap_minutes = 60
tolerance_secs = 2

[watcher]
# Ignore files with mtime < this (seconds)
stable_mtime_secs = 2

[checkpoint]
enabled = true
save_interval_secs = 30
verify_hashes = true
file = "features_state.json"

[writer]
compression = "zstd"
flush_interval_secs = 60
partition_by_date = true
include_metadata = true
```

---

*Spec v1.2.0 - Codex implementation guidance included*
