//! Integration tests for barter-features pipeline.
//!
//! These tests verify end-to-end functionality of the feature computation pipeline,
//! including reading input parquet files, processing through TPO, and writing output.

use arrow::array::{
    ArrayRef, FixedSizeBinaryArray, Int64Array, StringArray, StringBuilder, UInt8Array, UInt64Array,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

// ============================================================================
// Test Data Structures
// ============================================================================

/// Test bar data for creating extended_bars parquet files.
#[derive(Clone)]
pub struct TestBar {
    pub ts_event: u64,
    pub ts_init: u64,
    pub ts_open: u64,
    pub instrument_id: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub quote_volume: f64,
    pub trade_count: u64,
    pub buy_volume: f64,
    pub sell_volume: f64,
    pub delta: f64,
    pub cvd: f64,
    pub open_interest: f64,
    pub oi_change: f64,
    pub funding_rate: f64,
}

impl TestBar {
    /// Create a simple test bar with default values.
    pub fn new(ts_event: u64, price: f64, volume: f64) -> Self {
        Self {
            ts_event,
            ts_init: ts_event + 1_000_000,      // 1ms after event
            ts_open: ts_event - 60_000_000_000, // 1 minute before close
            instrument_id: "BTCUSDT-PERP.BINANCE".to_string(),
            open: price,
            high: price + 50.0,
            low: price - 50.0,
            close: price,
            volume,
            quote_volume: price * volume,
            trade_count: 100,
            buy_volume: volume * 0.6,
            sell_volume: volume * 0.4,
            delta: volume * 0.2,
            cvd: 0.0,
            open_interest: 50000.0,
            oi_change: 10.0,
            funding_rate: 0.0001,
        }
    }

    /// Create a bar with a specific instrument.
    pub fn with_instrument(mut self, instrument_id: &str) -> Self {
        self.instrument_id = instrument_id.to_string();
        self
    }
}

/// Test trade data for creating trades parquet files.
#[derive(Clone)]
pub struct TestTrade {
    pub ts_event: u64,
    pub ts_init: u64,
    pub price: f64,
    pub size: f64,
    pub aggressor_side: u8, // 1=BUY, 2=SELL
    pub trade_id: String,
}

impl TestTrade {
    pub fn new(ts_event: u64, price: f64, size: f64, side: u8, trade_id: &str) -> Self {
        Self {
            ts_event,
            ts_init: ts_event + 1_000_000,
            price,
            size,
            aggressor_side: side,
            trade_id: trade_id.to_string(),
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Encode a f64 value as Int64 x 1e9 (standard precision).
fn encode_i64(value: f64) -> i64 {
    (value * 1_000_000_000.0).round() as i64
}

// Note: encode_fixed_binary_8 removed as FixedSizeBinaryArray construction
// is done inline in create_test_trades for proper memory handling.

/// Create the schema for extended_bars parquet files.
fn extended_bars_schema() -> Schema {
    Schema::new(vec![
        Field::new("ts_event", DataType::UInt64, false),
        Field::new("ts_init", DataType::UInt64, false),
        Field::new("ts_open", DataType::UInt64, false),
        Field::new("instrument_id", DataType::Utf8, false),
        Field::new("open", DataType::Int64, false),
        Field::new("high", DataType::Int64, false),
        Field::new("low", DataType::Int64, false),
        Field::new("close", DataType::Int64, false),
        Field::new("volume", DataType::Int64, false),
        Field::new("quote_volume", DataType::Int64, false),
        Field::new("trade_count", DataType::UInt64, false),
        Field::new("buy_volume", DataType::Int64, false),
        Field::new("sell_volume", DataType::Int64, false),
        Field::new("delta", DataType::Int64, false),
        Field::new("cvd", DataType::Int64, false),
        Field::new("open_interest", DataType::Int64, false),
        Field::new("oi_change", DataType::Int64, false),
    ])
}

/// Create the schema for trades parquet files.
fn trades_schema() -> Schema {
    Schema::new(vec![
        Field::new("ts_event", DataType::UInt64, false),
        Field::new("ts_init", DataType::UInt64, false),
        Field::new("price", DataType::FixedSizeBinary(8), false),
        Field::new("size", DataType::FixedSizeBinary(8), false),
        Field::new("aggressor_side", DataType::UInt8, false),
        Field::new("trade_id", DataType::Utf8, false),
    ])
}

/// Create a test extended_bars parquet file.
///
/// # Arguments
/// * `dir` - Directory to create the file in
/// * `instrument` - Instrument ID (e.g., "BTCUSDT-PERP.BINANCE")
/// * `date` - Date string (e.g., "2024-02-02")
/// * `bars` - Vector of test bars to write
///
/// # Returns
/// Path to the created parquet file
pub fn create_test_extended_bars(
    dir: &Path,
    instrument: &str,
    date: &str,
    bars: &[TestBar],
) -> std::path::PathBuf {
    // Create directory structure: dir/instrument/date/extended_bars_1m.parquet
    let file_dir = dir.join(instrument).join(date);
    std::fs::create_dir_all(&file_dir).expect("Failed to create directory structure");

    let file_path = file_dir.join("extended_bars_1m.parquet");

    // Build arrays
    let len = bars.len();

    let ts_event: ArrayRef = Arc::new(UInt64Array::from(
        bars.iter().map(|b| b.ts_event).collect::<Vec<_>>(),
    ));
    let ts_init: ArrayRef = Arc::new(UInt64Array::from(
        bars.iter().map(|b| b.ts_init).collect::<Vec<_>>(),
    ));
    let ts_open: ArrayRef = Arc::new(UInt64Array::from(
        bars.iter().map(|b| b.ts_open).collect::<Vec<_>>(),
    ));

    let mut instrument_id_builder = StringBuilder::with_capacity(len, len * 32);
    for bar in bars {
        instrument_id_builder.append_value(&bar.instrument_id);
    }
    let instrument_id: ArrayRef = Arc::new(instrument_id_builder.finish());

    let open: ArrayRef = Arc::new(Int64Array::from(
        bars.iter().map(|b| encode_i64(b.open)).collect::<Vec<_>>(),
    ));
    let high: ArrayRef = Arc::new(Int64Array::from(
        bars.iter().map(|b| encode_i64(b.high)).collect::<Vec<_>>(),
    ));
    let low: ArrayRef = Arc::new(Int64Array::from(
        bars.iter().map(|b| encode_i64(b.low)).collect::<Vec<_>>(),
    ));
    let close: ArrayRef = Arc::new(Int64Array::from(
        bars.iter().map(|b| encode_i64(b.close)).collect::<Vec<_>>(),
    ));
    let volume: ArrayRef = Arc::new(Int64Array::from(
        bars.iter()
            .map(|b| encode_i64(b.volume))
            .collect::<Vec<_>>(),
    ));
    let quote_volume: ArrayRef = Arc::new(Int64Array::from(
        bars.iter()
            .map(|b| encode_i64(b.quote_volume))
            .collect::<Vec<_>>(),
    ));
    let trade_count: ArrayRef = Arc::new(UInt64Array::from(
        bars.iter().map(|b| b.trade_count).collect::<Vec<_>>(),
    ));
    let buy_volume: ArrayRef = Arc::new(Int64Array::from(
        bars.iter()
            .map(|b| encode_i64(b.buy_volume))
            .collect::<Vec<_>>(),
    ));
    let sell_volume: ArrayRef = Arc::new(Int64Array::from(
        bars.iter()
            .map(|b| encode_i64(b.sell_volume))
            .collect::<Vec<_>>(),
    ));
    let delta: ArrayRef = Arc::new(Int64Array::from(
        bars.iter().map(|b| encode_i64(b.delta)).collect::<Vec<_>>(),
    ));
    let cvd: ArrayRef = Arc::new(Int64Array::from(
        bars.iter().map(|b| encode_i64(b.cvd)).collect::<Vec<_>>(),
    ));
    let open_interest: ArrayRef = Arc::new(Int64Array::from(
        bars.iter()
            .map(|b| encode_i64(b.open_interest))
            .collect::<Vec<_>>(),
    ));
    let oi_change: ArrayRef = Arc::new(Int64Array::from(
        bars.iter()
            .map(|b| encode_i64(b.oi_change))
            .collect::<Vec<_>>(),
    ));

    let schema = Arc::new(extended_bars_schema());
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            ts_event,
            ts_init,
            ts_open,
            instrument_id,
            open,
            high,
            low,
            close,
            volume,
            quote_volume,
            trade_count,
            buy_volume,
            sell_volume,
            delta,
            cvd,
            open_interest,
            oi_change,
        ],
    )
    .expect("Failed to create record batch");

    // Write parquet file
    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(Default::default()))
        .build();

    let file = File::create(&file_path).expect("Failed to create parquet file");
    let mut writer =
        ArrowWriter::try_new(file, schema, Some(props)).expect("Failed to create ArrowWriter");
    writer.write(&batch).expect("Failed to write batch");
    writer.close().expect("Failed to close writer");

    file_path
}

/// Create a test trades parquet file.
///
/// # Arguments
/// * `dir` - Directory to create the file in
/// * `instrument` - Instrument ID
/// * `date` - Date string
/// * `trades` - Vector of test trades to write
///
/// # Returns
/// Path to the created parquet file
pub fn create_test_trades(
    dir: &Path,
    instrument: &str,
    date: &str,
    trades: &[TestTrade],
) -> std::path::PathBuf {
    let file_dir = dir.join(instrument).join(date);
    std::fs::create_dir_all(&file_dir).expect("Failed to create directory structure");

    let file_path = file_dir.join("trades.parquet");

    let len = trades.len();

    let ts_event: ArrayRef = Arc::new(UInt64Array::from(
        trades.iter().map(|t| t.ts_event).collect::<Vec<_>>(),
    ));
    let ts_init: ArrayRef = Arc::new(UInt64Array::from(
        trades.iter().map(|t| t.ts_init).collect::<Vec<_>>(),
    ));

    // Price and size as FixedSizeBinary(8)
    // Build fixed-size binary arrays from encoded values
    let price_data: Vec<[u8; 8]> = trades
        .iter()
        .map(|t| {
            let fixed = (t.price * 1_000_000_000.0).round() as i64;
            fixed.to_le_bytes()
        })
        .collect();

    let size_data: Vec<[u8; 8]> = trades
        .iter()
        .map(|t| {
            let fixed = (t.size * 1_000_000_000.0).round() as i64;
            fixed.to_le_bytes()
        })
        .collect();

    let price: ArrayRef = Arc::new(
        FixedSizeBinaryArray::try_from_iter(price_data.iter().map(|b| b.as_slice()))
            .expect("Failed to create price array"),
    );
    let size: ArrayRef = Arc::new(
        FixedSizeBinaryArray::try_from_iter(size_data.iter().map(|b| b.as_slice()))
            .expect("Failed to create size array"),
    );

    let aggressor_side: ArrayRef = Arc::new(UInt8Array::from(
        trades.iter().map(|t| t.aggressor_side).collect::<Vec<_>>(),
    ));

    let mut trade_id_builder = StringBuilder::with_capacity(len, len * 32);
    for trade in trades {
        trade_id_builder.append_value(&trade.trade_id);
    }
    let trade_id: ArrayRef = Arc::new(trade_id_builder.finish());

    let schema = Arc::new(trades_schema());
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![ts_event, ts_init, price, size, aggressor_side, trade_id],
    )
    .expect("Failed to create record batch");

    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(Default::default()))
        .build();

    let file = File::create(&file_path).expect("Failed to create parquet file");
    let mut writer =
        ArrowWriter::try_new(file, schema, Some(props)).expect("Failed to create ArrowWriter");
    writer.write(&batch).expect("Failed to write batch");
    writer.close().expect("Failed to close writer");

    file_path
}

/// Generate test bars for a specific time range.
///
/// Creates one bar per minute starting from `start_ts`.
fn generate_test_bars(start_ts: u64, count: usize, base_price: f64) -> Vec<TestBar> {
    (0..count)
        .map(|i| {
            let ts = start_ts + (i as u64 * 60_000_000_000); // 1 minute intervals
            let price = base_price + (i as f64 * 10.0); // Slight price increase
            TestBar::new(ts, price, 10.0 + (i as f64 * 0.1))
        })
        .collect()
}

/// Read a parquet file and return the record batches.
fn read_parquet_batches(path: &Path) -> Vec<RecordBatch> {
    let file = File::open(path).expect("Failed to open parquet file");
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).expect("Failed to create reader builder");
    let reader = builder.build().expect("Failed to build reader");
    reader
        .collect::<Result<Vec<_>, _>>()
        .expect("Failed to read batches")
}

/// Get column names from a parquet file.
fn get_parquet_columns(path: &Path) -> Vec<String> {
    let file = File::open(path).expect("Failed to open parquet file");
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).expect("Failed to create reader builder");
    builder
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().to_string())
        .collect()
}

/// Get row count from a parquet file.
fn get_parquet_row_count(path: &Path) -> usize {
    let batches = read_parquet_batches(path);
    batches.iter().map(|b| b.num_rows()).sum()
}

// ============================================================================
// Integration Tests
// ============================================================================

/// Create a test configuration pointing to temp directories.
fn create_test_config(input_dir: &Path, output_dir: &Path, checkpoint_dir: &Path) -> String {
    format!(
        r#"
[general]
schema_version = "1.2.0"
input_dir = "{}"
output_dir = "{}"
checkpoint_dir = "{}"
mode = "batch"

[precision]
mode = "standard"

[tpo]
bracket_minutes = 30
price_bucket_usd = 50.0
initial_balance_brackets = 2
value_area_pct = 0.70

[checkpoint]
enabled = false

[watcher]
stable_mtime_secs = 0
"#,
        input_dir.display(),
        output_dir.display(),
        checkpoint_dir.display()
    )
}

#[tokio::test]
async fn test_batch_mode_produces_tpo_output() {
    // Setup temp directories
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let input_dir = temp_dir.path().join("input");
    let output_dir = temp_dir.path().join("output");
    let checkpoint_dir = temp_dir.path().join("checkpoint");

    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&checkpoint_dir).unwrap();

    // Create test input: 60 bars (2 full 30-min brackets)
    let instrument = "BTCUSDT-PERP.BINANCE";
    let date = "2024-02-02";
    // Timestamp for 2024-02-02 00:00:00 UTC
    let base_ts: u64 = 1_706_832_000_000_000_000;

    let bars = generate_test_bars(base_ts, 60, 45000.0);
    create_test_extended_bars(&input_dir, instrument, date, &bars);

    // Write config to temp file
    let config_content = create_test_config(&input_dir, &output_dir, &checkpoint_dir);
    let config_path = temp_dir.path().join("config.toml");
    std::fs::write(&config_path, &config_content).unwrap();

    // Load config and run pipeline
    let config = barter_features::Config::from_file(&config_path).expect("Failed to load config");
    let mut pipeline = barter_features::Pipeline::new(config).expect("Failed to create pipeline");
    pipeline.run_batch().await.expect("Pipeline failed");

    // Verify output exists
    let tpo_output_path = output_dir
        .join(instrument)
        .join(date)
        .join("tpo_brackets.parquet");

    assert!(
        tpo_output_path.exists(),
        "TPO output file should exist at {:?}",
        tpo_output_path
    );

    // Verify output has content
    let row_count = get_parquet_row_count(&tpo_output_path);
    assert!(
        row_count >= 1,
        "TPO output should have at least 1 bracket, got {}",
        row_count
    );
}

#[tokio::test]
async fn test_batch_mode_handles_empty_directory() {
    // Setup temp directories
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let input_dir = temp_dir.path().join("input");
    let output_dir = temp_dir.path().join("output");
    let checkpoint_dir = temp_dir.path().join("checkpoint");

    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&checkpoint_dir).unwrap();

    // No input files created - empty input directory

    let config_content = create_test_config(&input_dir, &output_dir, &checkpoint_dir);
    let config_path = temp_dir.path().join("config.toml");
    std::fs::write(&config_path, &config_content).unwrap();

    let config = barter_features::Config::from_file(&config_path).expect("Failed to load config");
    let mut pipeline = barter_features::Pipeline::new(config).expect("Failed to create pipeline");

    // Should complete without error even with no input
    let result = pipeline.run_batch().await;
    assert!(
        result.is_ok(),
        "Pipeline should handle empty directory gracefully: {:?}",
        result.err()
    );

    // Output directory should exist but be empty (or have no instrument subdirs)
    let output_contents: Vec<_> = std::fs::read_dir(&output_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        output_contents.is_empty(),
        "Output directory should be empty when no input provided"
    );
}

#[tokio::test]
async fn test_session_accumulation_across_files() {
    // Setup temp directories
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let input_dir = temp_dir.path().join("input");
    let output_dir = temp_dir.path().join("output");
    let checkpoint_dir = temp_dir.path().join("checkpoint");

    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&checkpoint_dir).unwrap();

    let instrument = "BTCUSDT-PERP.BINANCE";
    let date = "2024-02-02";
    let base_ts: u64 = 1_706_832_000_000_000_000;

    // Create two files for the same session with different bar ranges
    // File 1: First 30 bars (bracket A)
    let bars1 = generate_test_bars(base_ts, 30, 45000.0);

    // File 2: Next 30 bars (bracket B)
    let start_ts2 = base_ts + (30 * 60_000_000_000);
    let bars2 = generate_test_bars(start_ts2, 30, 45300.0);

    // Create separate directories for each file to simulate multiple input files
    let file_dir = input_dir.join(instrument).join(date);
    std::fs::create_dir_all(&file_dir).unwrap();

    // Write first file
    let file1_path = file_dir.join("extended_bars_1m_part1.parquet");
    write_test_bars_to_path(&file1_path, &bars1);

    // Write second file
    let file2_path = file_dir.join("extended_bars_1m_part2.parquet");
    write_test_bars_to_path(&file2_path, &bars2);

    let config_content = create_test_config(&input_dir, &output_dir, &checkpoint_dir);
    let config_path = temp_dir.path().join("config.toml");
    std::fs::write(&config_path, &config_content).unwrap();

    let config = barter_features::Config::from_file(&config_path).expect("Failed to load config");
    let mut pipeline = barter_features::Pipeline::new(config).expect("Failed to create pipeline");
    pipeline.run_batch().await.expect("Pipeline failed");

    // Verify output
    let tpo_output_path = output_dir
        .join(instrument)
        .join(date)
        .join("tpo_brackets.parquet");

    assert!(tpo_output_path.exists(), "TPO output should exist");

    let row_count = get_parquet_row_count(&tpo_output_path);
    // Should have at least 2 brackets (one for each 30-minute period)
    assert!(
        row_count >= 2,
        "Should have at least 2 brackets from merged files, got {}",
        row_count
    );
}

/// Helper to write test bars directly to a specific path.
fn write_test_bars_to_path(path: &Path, bars: &[TestBar]) {
    let len = bars.len();

    let ts_event: ArrayRef = Arc::new(UInt64Array::from(
        bars.iter().map(|b| b.ts_event).collect::<Vec<_>>(),
    ));
    let ts_init: ArrayRef = Arc::new(UInt64Array::from(
        bars.iter().map(|b| b.ts_init).collect::<Vec<_>>(),
    ));
    let ts_open: ArrayRef = Arc::new(UInt64Array::from(
        bars.iter().map(|b| b.ts_open).collect::<Vec<_>>(),
    ));

    let mut instrument_id_builder = StringBuilder::with_capacity(len, len * 32);
    for bar in bars {
        instrument_id_builder.append_value(&bar.instrument_id);
    }
    let instrument_id: ArrayRef = Arc::new(instrument_id_builder.finish());

    let open: ArrayRef = Arc::new(Int64Array::from(
        bars.iter().map(|b| encode_i64(b.open)).collect::<Vec<_>>(),
    ));
    let high: ArrayRef = Arc::new(Int64Array::from(
        bars.iter().map(|b| encode_i64(b.high)).collect::<Vec<_>>(),
    ));
    let low: ArrayRef = Arc::new(Int64Array::from(
        bars.iter().map(|b| encode_i64(b.low)).collect::<Vec<_>>(),
    ));
    let close: ArrayRef = Arc::new(Int64Array::from(
        bars.iter().map(|b| encode_i64(b.close)).collect::<Vec<_>>(),
    ));
    let volume: ArrayRef = Arc::new(Int64Array::from(
        bars.iter()
            .map(|b| encode_i64(b.volume))
            .collect::<Vec<_>>(),
    ));
    let quote_volume: ArrayRef = Arc::new(Int64Array::from(
        bars.iter()
            .map(|b| encode_i64(b.quote_volume))
            .collect::<Vec<_>>(),
    ));
    let trade_count: ArrayRef = Arc::new(UInt64Array::from(
        bars.iter().map(|b| b.trade_count).collect::<Vec<_>>(),
    ));
    let buy_volume: ArrayRef = Arc::new(Int64Array::from(
        bars.iter()
            .map(|b| encode_i64(b.buy_volume))
            .collect::<Vec<_>>(),
    ));
    let sell_volume: ArrayRef = Arc::new(Int64Array::from(
        bars.iter()
            .map(|b| encode_i64(b.sell_volume))
            .collect::<Vec<_>>(),
    ));
    let delta: ArrayRef = Arc::new(Int64Array::from(
        bars.iter().map(|b| encode_i64(b.delta)).collect::<Vec<_>>(),
    ));
    let cvd: ArrayRef = Arc::new(Int64Array::from(
        bars.iter().map(|b| encode_i64(b.cvd)).collect::<Vec<_>>(),
    ));
    let open_interest: ArrayRef = Arc::new(Int64Array::from(
        bars.iter()
            .map(|b| encode_i64(b.open_interest))
            .collect::<Vec<_>>(),
    ));
    let oi_change: ArrayRef = Arc::new(Int64Array::from(
        bars.iter()
            .map(|b| encode_i64(b.oi_change))
            .collect::<Vec<_>>(),
    ));

    let schema = Arc::new(extended_bars_schema());
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            ts_event,
            ts_init,
            ts_open,
            instrument_id,
            open,
            high,
            low,
            close,
            volume,
            quote_volume,
            trade_count,
            buy_volume,
            sell_volume,
            delta,
            cvd,
            open_interest,
            oi_change,
        ],
    )
    .expect("Failed to create record batch");

    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(Default::default()))
        .build();

    let file = File::create(path).expect("Failed to create parquet file");
    let mut writer =
        ArrowWriter::try_new(file, schema, Some(props)).expect("Failed to create ArrowWriter");
    writer.write(&batch).expect("Failed to write batch");
    writer.close().expect("Failed to close writer");
}

#[tokio::test]
async fn test_output_schema_correctness() {
    // Setup temp directories
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let input_dir = temp_dir.path().join("input");
    let output_dir = temp_dir.path().join("output");
    let checkpoint_dir = temp_dir.path().join("checkpoint");

    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&checkpoint_dir).unwrap();

    let instrument = "BTCUSDT-PERP.BINANCE";
    let date = "2024-02-02";
    let base_ts: u64 = 1_706_832_000_000_000_000;

    let bars = generate_test_bars(base_ts, 60, 45000.0);
    create_test_extended_bars(&input_dir, instrument, date, &bars);

    let config_content = create_test_config(&input_dir, &output_dir, &checkpoint_dir);
    let config_path = temp_dir.path().join("config.toml");
    std::fs::write(&config_path, &config_content).unwrap();

    let config = barter_features::Config::from_file(&config_path).expect("Failed to load config");
    let mut pipeline = barter_features::Pipeline::new(config).expect("Failed to create pipeline");
    pipeline.run_batch().await.expect("Pipeline failed");

    let tpo_output_path = output_dir
        .join(instrument)
        .join(date)
        .join("tpo_brackets.parquet");

    assert!(tpo_output_path.exists(), "TPO output should exist");

    // Get columns and verify schema
    let columns = get_parquet_columns(&tpo_output_path);

    // Expected columns from tpo_bracket_schema
    let expected_columns = vec![
        "ts_event",
        "ts_init",
        "schema_version",
        "config_hash",
        "source_precision",
        "output_precision",
        "instrument_id",
        "session_id",
        "session_date",
        "label",
        "bracket_index",
        "bracket_start_ts",
        "bracket_end_ts",
        "bracket_open",
        "bracket_high",
        "bracket_low",
        "bracket_close",
        "bracket_volume",
        "bracket_buy_volume",
        "bracket_sell_volume",
        "bracket_delta",
        "bracket_trade_count",
        "bracket_bar_count",
        "bracket_has_gap",
        "vol_poc",
        "vol_vah",
        "vol_val",
        "vol_total",
        "tpo_poc",
        "tpo_vah",
        "tpo_val",
        "tpo_total_periods",
        "ib_high",
        "ib_low",
        "ib_complete",
        "poc_divergence",
        "session_type",
    ];

    // Verify all expected columns exist
    for expected in &expected_columns {
        assert!(
            columns.contains(&expected.to_string()),
            "Missing expected column: {}. Got columns: {:?}",
            expected,
            columns
        );
    }

    // Verify column count matches
    assert_eq!(
        columns.len(),
        expected_columns.len(),
        "Column count mismatch. Expected {}, got {}. Columns: {:?}",
        expected_columns.len(),
        columns.len(),
        columns
    );

    // Read and verify some data values
    let batches = read_parquet_batches(&tpo_output_path);
    assert!(!batches.is_empty(), "Should have at least one batch");

    let batch = &batches[0];

    // Verify instrument_id column has correct value
    let instrument_col = batch
        .column_by_name("instrument_id")
        .expect("instrument_id column should exist");
    let instrument_arr = instrument_col
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("instrument_id should be StringArray");
    assert_eq!(
        instrument_arr.value(0),
        instrument,
        "Instrument ID should match"
    );

    // Verify session_date column
    let date_col = batch
        .column_by_name("session_date")
        .expect("session_date column should exist");
    let date_arr = date_col
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("session_date should be StringArray");
    assert_eq!(date_arr.value(0), date, "Session date should match");

    // Verify label column has valid bracket labels
    let label_col = batch
        .column_by_name("label")
        .expect("label column should exist");
    let label_arr = label_col
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("label should be StringArray");
    let label = label_arr.value(0);
    assert!(
        label.len() <= 2 && label.chars().all(|c| c.is_ascii_uppercase()),
        "Label should be valid bracket label (A-Z or AA-AV), got: {}",
        label
    );

    // Verify schema_version is set
    let version_col = batch
        .column_by_name("schema_version")
        .expect("schema_version column should exist");
    let version_arr = version_col
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("schema_version should be StringArray");
    assert_eq!(
        version_arr.value(0),
        "1.2.0",
        "Schema version should be 1.2.0"
    );
}

#[tokio::test]
async fn test_helper_functions_create_valid_parquet() {
    // Test that our helper functions create valid, readable parquet files
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    let instrument = "ETHUSDT-PERP.BINANCE";
    let date = "2024-01-15";

    // Test extended bars creation
    let bars = vec![
        TestBar::new(1_705_276_800_000_000_000, 2500.0, 100.0),
        TestBar::new(1_705_276_860_000_000_000, 2505.0, 150.0),
        TestBar::new(1_705_276_920_000_000_000, 2510.0, 120.0),
    ];

    let bars_path = create_test_extended_bars(temp_dir.path(), instrument, date, &bars);
    assert!(bars_path.exists(), "Extended bars file should exist");

    // Verify file is readable
    let batches = read_parquet_batches(&bars_path);
    assert_eq!(batches.len(), 1, "Should have exactly one batch");
    assert_eq!(batches[0].num_rows(), 3, "Should have 3 rows");

    // Verify columns
    let columns = get_parquet_columns(&bars_path);
    assert!(columns.contains(&"ts_event".to_string()));
    assert!(columns.contains(&"open".to_string()));
    assert!(columns.contains(&"volume".to_string()));

    // Test trades creation
    let trades = vec![
        TestTrade::new(1_705_276_800_000_000_000, 2500.0, 5.0, 1, "trade_001"),
        TestTrade::new(1_705_276_801_000_000_000, 2501.0, 3.0, 2, "trade_002"),
    ];

    let trades_path = create_test_trades(temp_dir.path(), instrument, date, &trades);
    assert!(trades_path.exists(), "Trades file should exist");

    let trade_batches = read_parquet_batches(&trades_path);
    assert_eq!(trade_batches.len(), 1, "Should have exactly one batch");
    assert_eq!(trade_batches[0].num_rows(), 2, "Should have 2 rows");

    let trade_columns = get_parquet_columns(&trades_path);
    assert!(trade_columns.contains(&"price".to_string()));
    assert!(trade_columns.contains(&"size".to_string()));
    assert!(trade_columns.contains(&"trade_id".to_string()));
}

#[tokio::test]
async fn test_multiple_instruments_processed_independently() {
    // Setup temp directories
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let input_dir = temp_dir.path().join("input");
    let output_dir = temp_dir.path().join("output");
    let checkpoint_dir = temp_dir.path().join("checkpoint");

    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&checkpoint_dir).unwrap();

    let date = "2024-02-02";
    let base_ts: u64 = 1_706_832_000_000_000_000;

    // Create data for two different instruments
    let btc_bars: Vec<TestBar> = generate_test_bars(base_ts, 60, 45000.0)
        .into_iter()
        .map(|b| b.with_instrument("BTCUSDT-PERP.BINANCE"))
        .collect();

    let eth_bars: Vec<TestBar> = generate_test_bars(base_ts, 60, 2500.0)
        .into_iter()
        .map(|b| b.with_instrument("ETHUSDT-PERP.BINANCE"))
        .collect();

    create_test_extended_bars(&input_dir, "BTCUSDT-PERP.BINANCE", date, &btc_bars);
    create_test_extended_bars(&input_dir, "ETHUSDT-PERP.BINANCE", date, &eth_bars);

    let config_content = create_test_config(&input_dir, &output_dir, &checkpoint_dir);
    let config_path = temp_dir.path().join("config.toml");
    std::fs::write(&config_path, &config_content).unwrap();

    let config = barter_features::Config::from_file(&config_path).expect("Failed to load config");
    let mut pipeline = barter_features::Pipeline::new(config).expect("Failed to create pipeline");
    pipeline.run_batch().await.expect("Pipeline failed");

    // Verify both instruments have output
    let btc_output = output_dir
        .join("BTCUSDT-PERP.BINANCE")
        .join(date)
        .join("tpo_brackets.parquet");
    let eth_output = output_dir
        .join("ETHUSDT-PERP.BINANCE")
        .join(date)
        .join("tpo_brackets.parquet");

    assert!(btc_output.exists(), "BTC output should exist");
    assert!(eth_output.exists(), "ETH output should exist");

    // Verify data is instrument-specific
    let btc_batches = read_parquet_batches(&btc_output);
    let eth_batches = read_parquet_batches(&eth_output);

    let btc_instrument = btc_batches[0]
        .column_by_name("instrument_id")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap()
        .value(0);
    assert_eq!(btc_instrument, "BTCUSDT-PERP.BINANCE");

    let eth_instrument = eth_batches[0]
        .column_by_name("instrument_id")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap()
        .value(0);
    assert_eq!(eth_instrument, "ETHUSDT-PERP.BINANCE");
}
