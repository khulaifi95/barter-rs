//! Parquet verification utilities.
//!
//! Validates data integrity and checks for gaps.

use arrow::array::{Array, UInt64Array};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use std::collections::{BTreeSet, HashSet};
use std::fs::File;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),

    #[error("Arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    #[error("Verification failed: {0}")]
    VerificationFailed(String),
}

/// Result of verifying a Parquet file.
#[derive(Debug, Default, Clone)]
pub struct VerifyResult {
    pub rows: u64,
    pub duplicates: u64,
    pub nulls: u64,
    pub out_of_order: u64,
    pub min_ts_ns: Option<u64>,
    pub max_ts_ns: Option<u64>,
    pub errors: Vec<String>,
}

impl VerifyResult {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty() && self.duplicates == 0 && self.out_of_order == 0
    }
}

impl std::fmt::Display for VerifyResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Rows: {}, Duplicates: {}, OutOfOrder: {}, Nulls: {}",
            self.rows, self.duplicates, self.out_of_order, self.nulls
        )?;
        if let (Some(min), Some(max)) = (self.min_ts_ns, self.max_ts_ns) {
            write!(f, ", TimeRange: {} - {}", min, max)?;
        }
        Ok(())
    }
}

/// Verify a bar Parquet file.
///
/// Checks:
/// - Schema has expected columns (open, high, low, close, volume, ts_event, ts_init)
/// - No null values in required columns
/// - Timestamps are in order (ts_event ascending)
/// - No duplicate timestamps
pub fn verify_bar_file(path: &Path) -> Result<VerifyResult, VerifyError> {
    let file = File::open(path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
    let schema = builder.schema();

    let mut result = VerifyResult::default();

    // Verify schema
    let expected_cols = [
        "open", "high", "low", "close", "volume", "ts_event", "ts_init",
    ];
    for col in &expected_cols {
        if schema.field_with_name(col).is_err() {
            result.errors.push(format!("Missing column: {}", col));
        }
    }

    // Find ts_event column index
    let ts_event_idx = schema
        .fields()
        .iter()
        .position(|f| f.name() == "ts_event")
        .unwrap_or(5); // Default to position 5

    let reader = builder.build()?;
    let mut prev_ts: Option<u64> = None;
    let mut seen_ts: HashSet<u64> = HashSet::new();

    for batch_result in reader {
        let batch = batch_result?;
        result.rows += batch.num_rows() as u64;

        // Check ts_event column
        let ts_col = batch.column(ts_event_idx);
        let ts_array = ts_col
            .as_any()
            .downcast_ref::<UInt64Array>()
            .ok_or_else(|| VerifyError::VerificationFailed("ts_event not UInt64".to_string()))?;

        for i in 0..ts_array.len() {
            if ts_array.is_null(i) {
                result.nulls += 1;
                continue;
            }

            let ts = ts_array.value(i);

            // Update min/max
            match result.min_ts_ns {
                None => result.min_ts_ns = Some(ts),
                Some(min) if ts < min => result.min_ts_ns = Some(ts),
                _ => {}
            }
            match result.max_ts_ns {
                None => result.max_ts_ns = Some(ts),
                Some(max) if ts > max => result.max_ts_ns = Some(ts),
                _ => {}
            }

            // Check ordering
            if let Some(prev) = prev_ts {
                if ts < prev {
                    result.out_of_order += 1;
                }
            }
            prev_ts = Some(ts);

            // Check duplicates
            if !seen_ts.insert(ts) {
                result.duplicates += 1;
            }
        }
    }

    Ok(result)
}

/// Verify a trade Parquet file.
///
/// Checks:
/// - Schema has expected columns (price, size, aggressor_side, trade_id, ts_event, ts_init)
/// - No null values in required columns
/// - Timestamps are in order (ts_event ascending)
/// - No duplicate trade_ids
pub fn verify_trade_file(path: &Path) -> Result<VerifyResult, VerifyError> {
    let file = File::open(path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
    let schema = builder.schema();

    let mut result = VerifyResult::default();

    // Verify schema
    let expected_cols = [
        "price",
        "size",
        "aggressor_side",
        "trade_id",
        "ts_event",
        "ts_init",
    ];
    for col in &expected_cols {
        if schema.field_with_name(col).is_err() {
            result.errors.push(format!("Missing column: {}", col));
        }
    }

    // Find column indices
    let ts_event_idx = schema
        .fields()
        .iter()
        .position(|f| f.name() == "ts_event")
        .unwrap_or(4);

    let reader = builder.build()?;
    let mut prev_ts: Option<u64> = None;

    for batch_result in reader {
        let batch = batch_result?;
        result.rows += batch.num_rows() as u64;

        // Check ts_event column
        let ts_col = batch.column(ts_event_idx);
        let ts_array = ts_col
            .as_any()
            .downcast_ref::<UInt64Array>()
            .ok_or_else(|| VerifyError::VerificationFailed("ts_event not UInt64".to_string()))?;

        for i in 0..ts_array.len() {
            if ts_array.is_null(i) {
                result.nulls += 1;
                continue;
            }

            let ts = ts_array.value(i);

            // Update min/max
            match result.min_ts_ns {
                None => result.min_ts_ns = Some(ts),
                Some(min) if ts < min => result.min_ts_ns = Some(ts),
                _ => {}
            }
            match result.max_ts_ns {
                None => result.max_ts_ns = Some(ts),
                Some(max) if ts > max => result.max_ts_ns = Some(ts),
                _ => {}
            }

            // Check ordering
            if let Some(prev) = prev_ts {
                if ts < prev {
                    result.out_of_order += 1;
                }
            }
            prev_ts = Some(ts);
        }
    }

    Ok(result)
}

/// A detected gap in data coverage.
#[derive(Debug, Clone)]
pub struct Gap {
    pub start_ns: u64,
    pub end_ns: u64,
    pub missing_intervals: u64,
}

impl Gap {
    /// Duration of the gap in nanoseconds.
    pub fn duration_ns(&self) -> u64 {
        self.end_ns.saturating_sub(self.start_ns)
    }

    /// Duration of the gap in seconds.
    pub fn duration_secs(&self) -> f64 {
        self.duration_ns() as f64 / 1_000_000_000.0
    }
}

impl std::fmt::Display for Gap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Convert nanoseconds to human readable format
        let start_secs = self.start_ns / 1_000_000_000;
        let end_secs = self.end_ns / 1_000_000_000;
        let duration_mins = self.duration_secs() / 60.0;

        write!(
            f,
            "Gap: {} - {} ({:.1} mins, {} missing intervals)",
            start_secs, end_secs, duration_mins, self.missing_intervals
        )
    }
}

/// Result of gap checking.
#[derive(Debug, Default)]
pub struct GapCheckResult {
    pub files_checked: u64,
    pub total_rows: u64,
    pub min_ts_ns: Option<u64>,
    pub max_ts_ns: Option<u64>,
    pub gaps: Vec<Gap>,
    pub coverage_pct: f64,
}

impl std::fmt::Display for GapCheckResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Files: {}, Rows: {}, Gaps: {}, Coverage: {:.2}%",
            self.files_checked,
            self.total_rows,
            self.gaps.len(),
            self.coverage_pct
        )
    }
}

/// Check for gaps in a directory of Parquet files.
///
/// Reads all Parquet files, extracts ts_event timestamps, and identifies
/// gaps larger than the expected interval.
pub fn check_gaps(dir: &Path, expected_interval_ns: u64) -> Result<GapCheckResult, VerifyError> {
    let mut result = GapCheckResult::default();

    // Collect all timestamps from all files
    let mut all_timestamps: BTreeSet<u64> = BTreeSet::new();

    // Find all Parquet files recursively
    let parquet_files = find_parquet_files(dir)?;

    for path in parquet_files {
        result.files_checked += 1;

        let file = match File::open(&path) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("Failed to open {:?}: {}", path, e);
                continue;
            }
        };

        let builder = match ParquetRecordBatchReaderBuilder::try_new(file) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("Failed to read {:?}: {}", path, e);
                continue;
            }
        };

        let schema = builder.schema();
        let ts_event_idx = schema
            .fields()
            .iter()
            .position(|f| f.name() == "ts_event")
            .unwrap_or(5);

        let reader = builder.build()?;

        for batch_result in reader {
            let batch = batch_result?;
            result.total_rows += batch.num_rows() as u64;

            let ts_col = batch.column(ts_event_idx);
            if let Some(ts_array) = ts_col.as_any().downcast_ref::<UInt64Array>() {
                for i in 0..ts_array.len() {
                    if !ts_array.is_null(i) {
                        all_timestamps.insert(ts_array.value(i));
                    }
                }
            }
        }
    }

    if all_timestamps.is_empty() {
        return Ok(result);
    }

    // Find min/max
    result.min_ts_ns = all_timestamps.iter().next().copied();
    result.max_ts_ns = all_timestamps.iter().next_back().copied();

    // Detect gaps
    let timestamps: Vec<u64> = all_timestamps.into_iter().collect();
    let tolerance = expected_interval_ns + (expected_interval_ns / 10); // 10% tolerance

    for i in 1..timestamps.len() {
        let delta = timestamps[i] - timestamps[i - 1];
        if delta > tolerance {
            let missing = delta / expected_interval_ns;
            result.gaps.push(Gap {
                start_ns: timestamps[i - 1],
                end_ns: timestamps[i],
                missing_intervals: missing.saturating_sub(1),
            });
        }
    }

    // Calculate coverage percentage
    if let (Some(min), Some(max)) = (result.min_ts_ns, result.max_ts_ns) {
        let total_range = max - min;
        if total_range > 0 {
            let expected_intervals = total_range / expected_interval_ns;
            let gap_intervals: u64 = result.gaps.iter().map(|g| g.missing_intervals).sum();
            let actual_intervals = expected_intervals.saturating_sub(gap_intervals);
            result.coverage_pct = (actual_intervals as f64 / expected_intervals as f64) * 100.0;
        }
    }

    Ok(result)
}

/// Find all Parquet files in a directory recursively.
fn find_parquet_files(dir: &Path) -> Result<Vec<std::path::PathBuf>, VerifyError> {
    let mut files = Vec::new();

    if dir.is_file() {
        if dir.extension().and_then(|e| e.to_str()) == Some("parquet") {
            files.push(dir.to_path_buf());
        }
        return Ok(files);
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            files.extend(find_parquet_files(&path)?);
        } else if path.extension().and_then(|e| e.to_str()) == Some("parquet") {
            files.push(path);
        }
    }

    Ok(files)
}

/// Verify a directory of Parquet files.
pub fn verify_directory(
    dir: &Path,
    data_type: &str,
) -> Result<Vec<(std::path::PathBuf, VerifyResult)>, VerifyError> {
    let files = find_parquet_files(dir)?;
    let mut results = Vec::new();

    for path in files {
        let result = match data_type {
            "bars" | "klines" => verify_bar_file(&path),
            "trades" | "aggTrades" => verify_trade_file(&path),
            _ => verify_bar_file(&path), // Default to bars
        };

        match result {
            Ok(r) => results.push((path, r)),
            Err(e) => {
                let mut error_result = VerifyResult::default();
                error_result.errors.push(format!("Failed to verify: {}", e));
                results.push((path, error_result));
            }
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::FixedSizeBinaryBuilder;
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use parquet::arrow::ArrowWriter;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn create_test_bar_parquet(path: &Path, timestamps: &[u64]) -> Result<(), VerifyError> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("open", DataType::FixedSizeBinary(16), false),
            Field::new("high", DataType::FixedSizeBinary(16), false),
            Field::new("low", DataType::FixedSizeBinary(16), false),
            Field::new("close", DataType::FixedSizeBinary(16), false),
            Field::new("volume", DataType::FixedSizeBinary(16), false),
            Field::new("ts_event", DataType::UInt64, false),
            Field::new("ts_init", DataType::UInt64, false),
        ]));

        let capacity = timestamps.len();
        let mut opens = FixedSizeBinaryBuilder::with_capacity(capacity, 16);
        let mut highs = FixedSizeBinaryBuilder::with_capacity(capacity, 16);
        let mut lows = FixedSizeBinaryBuilder::with_capacity(capacity, 16);
        let mut closes = FixedSizeBinaryBuilder::with_capacity(capacity, 16);
        let mut volumes = FixedSizeBinaryBuilder::with_capacity(capacity, 16);
        let mut ts_events = arrow::array::UInt64Builder::with_capacity(capacity);
        let mut ts_inits = arrow::array::UInt64Builder::with_capacity(capacity);

        for &ts in timestamps {
            opens.append_value([0u8; 16])?;
            highs.append_value([0u8; 16])?;
            lows.append_value([0u8; 16])?;
            closes.append_value([0u8; 16])?;
            volumes.append_value([0u8; 16])?;
            ts_events.append_value(ts);
            ts_inits.append_value(ts + 100);
        }

        let arrays: Vec<arrow::array::ArrayRef> = vec![
            Arc::new(opens.finish()),
            Arc::new(highs.finish()),
            Arc::new(lows.finish()),
            Arc::new(closes.finish()),
            Arc::new(volumes.finish()),
            Arc::new(ts_events.finish()),
            Arc::new(ts_inits.finish()),
        ];

        let batch = RecordBatch::try_new(schema.clone(), arrays)?;

        let file = File::create(path)?;
        let mut writer = ArrowWriter::try_new(file, schema, None)?;
        writer.write(&batch)?;
        writer.close()?;

        Ok(())
    }

    #[test]
    fn test_verify_bar_file() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("test_bars.parquet");

        // Create test file with ordered timestamps (60 second intervals in nanos)
        let timestamps: Vec<u64> = (0..10)
            .map(|i| 1_706_540_400_000_000_000 + (i as u64 * 60_000_000_000))
            .collect();

        create_test_bar_parquet(&path, &timestamps).unwrap();

        let result = verify_bar_file(&path).unwrap();
        assert_eq!(result.rows, 10);
        assert_eq!(result.duplicates, 0);
        assert_eq!(result.out_of_order, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_detects_duplicates() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("test_dups.parquet");

        // Create test file with duplicate timestamp
        let timestamps = vec![
            1_706_540_400_000_000_000,
            1_706_540_460_000_000_000,
            1_706_540_460_000_000_000, // Duplicate!
            1_706_540_520_000_000_000,
        ];

        create_test_bar_parquet(&path, &timestamps).unwrap();

        let result = verify_bar_file(&path).unwrap();
        assert_eq!(result.rows, 4);
        assert_eq!(result.duplicates, 1);
        assert!(!result.is_ok());
    }

    #[test]
    fn test_verify_detects_out_of_order() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("test_order.parquet");

        // Create test file with out-of-order timestamp
        let timestamps = vec![
            1_706_540_400_000_000_000,
            1_706_540_520_000_000_000,
            1_706_540_460_000_000_000, // Out of order!
            1_706_540_580_000_000_000,
        ];

        create_test_bar_parquet(&path, &timestamps).unwrap();

        let result = verify_bar_file(&path).unwrap();
        assert_eq!(result.rows, 4);
        assert_eq!(result.out_of_order, 1);
        assert!(!result.is_ok());
    }

    #[test]
    fn test_check_gaps() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("test_gaps.parquet");

        // Create file with a gap (missing 2 minutes)
        let timestamps = vec![
            1_706_540_400_000_000_000, // 0
            1_706_540_460_000_000_000, // +1 min
            1_706_540_520_000_000_000, // +2 min
            // Gap here: 3 minutes missing
            1_706_540_760_000_000_000, // +6 min
            1_706_540_820_000_000_000, // +7 min
        ];

        create_test_bar_parquet(&path, &timestamps).unwrap();

        let result = check_gaps(temp_dir.path(), 60_000_000_000).unwrap();
        assert_eq!(result.files_checked, 1);
        assert_eq!(result.total_rows, 5);
        assert_eq!(result.gaps.len(), 1);
        assert_eq!(result.gaps[0].missing_intervals, 3); // 3 minutes missing
    }

    #[test]
    fn test_gap_display() {
        let gap = Gap {
            start_ns: 1_706_540_400_000_000_000,
            end_ns: 1_706_540_640_000_000_000,
            missing_intervals: 3,
        };

        assert_eq!(gap.duration_secs(), 240.0);
        let display = format!("{}", gap);
        assert!(display.contains("4.0 mins"));
    }
}
