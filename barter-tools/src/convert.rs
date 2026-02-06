//! CSV to Parquet conversion utilities.
//!
//! Converts Binance CSV files to Nautilus-compatible Parquet format.
//!
//! Binance Klines CSV columns:
//! 0. Open time (ms), 1. Open, 2. High, 3. Low, 4. Close, 5. Volume,
//! 6. Close time (ms), 7. Quote volume, 8. Trade count, 9. Taker buy base,
//! 10. Taker buy quote, 11. Ignore
//!
//! Binance AggTrades CSV columns:
//! 0. Agg trade ID, 1. Price, 2. Quantity, 3. First trade ID, 4. Last trade ID,
//! 5. Timestamp (ms), 6. Is buyer maker (true = seller aggressor)

use arrow::array::{ArrayRef, FixedSizeBinaryBuilder, StringBuilder, UInt64Builder, UInt8Builder};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;
use zip::ZipArchive;

use crate::{encode_fixed_point, PrecisionMode};

#[derive(Debug, Error)]
pub enum ConvertError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),

    #[error("Arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    #[error("Parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),

    #[error("ZIP error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("Invalid data format: {0}")]
    InvalidFormat(String),

    #[error("Parse error: {0}")]
    Parse(String),
}

/// Statistics from a conversion operation.
#[derive(Debug, Default)]
pub struct ConvertStats {
    pub rows_read: u64,
    pub rows_written: u64,
    pub files_processed: u64,
    pub errors: Vec<String>,
}

impl std::fmt::Display for ConvertStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Files: {}, Rows read: {}, Rows written: {}, Errors: {}",
            self.files_processed,
            self.rows_read,
            self.rows_written,
            self.errors.len()
        )
    }
}

/// Create Nautilus bar schema.
fn bar_schema(metadata: HashMap<String, String>, precision_bytes: i32) -> Arc<Schema> {
    Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("open", DataType::FixedSizeBinary(precision_bytes), false),
            Field::new("high", DataType::FixedSizeBinary(precision_bytes), false),
            Field::new("low", DataType::FixedSizeBinary(precision_bytes), false),
            Field::new("close", DataType::FixedSizeBinary(precision_bytes), false),
            Field::new("volume", DataType::FixedSizeBinary(precision_bytes), false),
            Field::new("ts_event", DataType::UInt64, false),
            Field::new("ts_init", DataType::UInt64, false),
        ],
        metadata,
    ))
}

/// Create Nautilus trade schema.
fn trade_schema(metadata: HashMap<String, String>, precision_bytes: i32) -> Arc<Schema> {
    Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("price", DataType::FixedSizeBinary(precision_bytes), false),
            Field::new("size", DataType::FixedSizeBinary(precision_bytes), false),
            Field::new("aggressor_side", DataType::UInt8, false),
            Field::new("trade_id", DataType::Utf8, false),
            Field::new("ts_event", DataType::UInt64, false),
            Field::new("ts_init", DataType::UInt64, false),
        ],
        metadata,
    ))
}

/// Parse a float from a CSV field.
fn parse_f64(s: &str) -> Result<f64, ConvertError> {
    s.trim()
        .parse::<f64>()
        .map_err(|e| ConvertError::Parse(format!("Invalid float '{}': {}", s, e)))
}

/// Parse a u64 from a CSV field.
fn parse_u64(s: &str) -> Result<u64, ConvertError> {
    s.trim()
        .parse::<u64>()
        .map_err(|e| ConvertError::Parse(format!("Invalid u64 '{}': {}", s, e)))
}

/// Parse a boolean from a CSV field.
fn parse_bool(s: &str) -> Result<bool, ConvertError> {
    match s.trim().to_lowercase().as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(ConvertError::Parse(format!("Invalid bool '{}'", s))),
    }
}

/// Read CSV content from a file (handles both plain CSV and ZIP).
pub fn read_csv_content(path: &Path) -> Result<String, ConvertError> {
    if path.extension().and_then(|e| e.to_str()) == Some("zip") {
        // Extract first CSV from ZIP
        let file = File::open(path)?;
        let mut archive = ZipArchive::new(file)?;

        if archive.is_empty() {
            return Err(ConvertError::InvalidFormat("Empty ZIP file".to_string()));
        }

        let mut zip_file = archive.by_index(0)?;
        let mut content = String::new();
        zip_file.read_to_string(&mut content)?;
        Ok(content)
    } else {
        // Plain CSV
        std::fs::read_to_string(path).map_err(ConvertError::from)
    }
}

/// Convert Binance klines CSV to Nautilus bar Parquet.
///
/// Input: CSV with columns: open_time, open, high, low, close, volume, close_time, ...
/// Output: Parquet with Nautilus bar schema
pub fn convert_klines_to_parquet(
    input_csv: &Path,
    output_parquet: &Path,
    symbol: &str,
    price_precision: u8,
    size_precision: u8,
) -> Result<u64, ConvertError> {
    let precision_mode = PrecisionMode::from_env();
    let bytes_len = match precision_mode {
        PrecisionMode::Standard => 8,
        PrecisionMode::High => 16,
    };

    // Read CSV content
    let content = read_csv_content(input_csv)?;
    let lines: Vec<&str> = content.lines().collect();

    if lines.is_empty() {
        return Ok(0);
    }

    // Pre-allocate builders
    let capacity = lines.len();
    let mut opens = FixedSizeBinaryBuilder::with_capacity(capacity, bytes_len);
    let mut highs = FixedSizeBinaryBuilder::with_capacity(capacity, bytes_len);
    let mut lows = FixedSizeBinaryBuilder::with_capacity(capacity, bytes_len);
    let mut closes = FixedSizeBinaryBuilder::with_capacity(capacity, bytes_len);
    let mut volumes = FixedSizeBinaryBuilder::with_capacity(capacity, bytes_len);
    let mut ts_events = UInt64Builder::with_capacity(capacity);
    let mut ts_inits = UInt64Builder::with_capacity(capacity);

    let mut rows_written = 0u64;

    for line in lines {
        let fields: Vec<&str> = line.split(',').collect();
        if fields.len() < 7 {
            continue; // Skip malformed lines
        }

        // Parse fields
        let _open_time_ms = match parse_u64(fields[0]) {
            Ok(v) => v,
            Err(_) => continue, // Skip header or malformed
        };
        let open = parse_f64(fields[1])?;
        let high = parse_f64(fields[2])?;
        let low = parse_f64(fields[3])?;
        let close = parse_f64(fields[4])?;
        let volume = parse_f64(fields[5])?;
        let close_time_ms = parse_u64(fields[6])?;

        // ts_event = bar CLOSE time in nanoseconds
        let ts_event_ns = close_time_ms * 1_000_000;
        // ts_init = slightly after ts_event (simulating ingest delay)
        let ts_init_ns = ts_event_ns + 100_000; // 100 microseconds

        // Encode prices
        let open_bytes = encode_fixed_point(open, precision_mode);
        let high_bytes = encode_fixed_point(high, precision_mode);
        let low_bytes = encode_fixed_point(low, precision_mode);
        let close_bytes = encode_fixed_point(close, precision_mode);
        let vol_bytes = encode_fixed_point(volume, precision_mode);

        opens.append_value(open_bytes.as_slice())?;
        highs.append_value(high_bytes.as_slice())?;
        lows.append_value(low_bytes.as_slice())?;
        closes.append_value(close_bytes.as_slice())?;
        volumes.append_value(vol_bytes.as_slice())?;
        ts_events.append_value(ts_event_ns);
        ts_inits.append_value(ts_init_ns);

        rows_written += 1;
    }

    if rows_written == 0 {
        return Ok(0);
    }

    // Build schema metadata
    let instrument_id = format!("{}-PERP.BINANCE", symbol);
    let bar_type = format!("{}-1-MINUTE-LAST-EXTERNAL", instrument_id);
    let mut metadata = HashMap::new();
    metadata.insert("bar_type".to_string(), bar_type);
    metadata.insert("instrument_id".to_string(), instrument_id);
    metadata.insert("price_precision".to_string(), price_precision.to_string());
    metadata.insert("size_precision".to_string(), size_precision.to_string());

    let schema = bar_schema(metadata, bytes_len);

    // Build record batch
    let arrays: Vec<ArrayRef> = vec![
        Arc::new(opens.finish()),
        Arc::new(highs.finish()),
        Arc::new(lows.finish()),
        Arc::new(closes.finish()),
        Arc::new(volumes.finish()),
        Arc::new(ts_events.finish()),
        Arc::new(ts_inits.finish()),
    ];
    let batch = RecordBatch::try_new(schema.clone(), arrays)?;

    // Write Parquet file
    if let Some(parent) = output_parquet.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();

    let file = File::create(output_parquet)?;
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
    writer.write(&batch)?;
    writer.close()?;

    Ok(rows_written)
}

/// Convert Binance aggregated trades CSV to Nautilus trade tick Parquet.
///
/// Input: CSV with columns: agg_trade_id, price, quantity, first_trade_id,
///        last_trade_id, timestamp, is_buyer_maker
/// Output: Parquet with Nautilus trade schema
pub fn convert_trades_to_parquet(
    input_csv: &Path,
    output_parquet: &Path,
    symbol: &str,
    price_precision: u8,
    size_precision: u8,
) -> Result<u64, ConvertError> {
    let precision_mode = PrecisionMode::from_env();
    let bytes_len = match precision_mode {
        PrecisionMode::Standard => 8,
        PrecisionMode::High => 16,
    };

    // Read CSV content
    let content = read_csv_content(input_csv)?;
    let lines: Vec<&str> = content.lines().collect();

    if lines.is_empty() {
        return Ok(0);
    }

    // Pre-allocate builders
    let capacity = lines.len();
    let mut prices = FixedSizeBinaryBuilder::with_capacity(capacity, bytes_len);
    let mut sizes = FixedSizeBinaryBuilder::with_capacity(capacity, bytes_len);
    let mut sides = UInt8Builder::with_capacity(capacity);
    let mut trade_ids = StringBuilder::with_capacity(capacity, capacity * 20);
    let mut ts_events = UInt64Builder::with_capacity(capacity);
    let mut ts_inits = UInt64Builder::with_capacity(capacity);

    let mut rows_written = 0u64;

    for line in lines {
        let fields: Vec<&str> = line.split(',').collect();
        if fields.len() < 7 {
            continue; // Skip malformed lines
        }

        // Parse fields
        let trade_id = match parse_u64(fields[0]) {
            Ok(v) => v.to_string(),
            Err(_) => continue, // Skip header or malformed
        };
        let price = parse_f64(fields[1])?;
        let quantity = parse_f64(fields[2])?;
        let timestamp_ms = parse_u64(fields[5])?;
        let is_buyer_maker = parse_bool(fields[6])?;

        // Aggressor side: is_buyer_maker=true means SELLER aggressor (2)
        // is_buyer_maker=false means BUYER aggressor (1)
        let aggressor_side: u8 = if is_buyer_maker { 2 } else { 1 };

        // Convert to nanoseconds
        let ts_event_ns = timestamp_ms * 1_000_000;
        let ts_init_ns = ts_event_ns + 50_000; // 50 microseconds

        // Encode prices
        let price_bytes = encode_fixed_point(price, precision_mode);
        let size_bytes = encode_fixed_point(quantity, precision_mode);

        prices.append_value(price_bytes.as_slice())?;
        sizes.append_value(size_bytes.as_slice())?;
        sides.append_value(aggressor_side);
        trade_ids.append_value(&trade_id);
        ts_events.append_value(ts_event_ns);
        ts_inits.append_value(ts_init_ns);

        rows_written += 1;
    }

    if rows_written == 0 {
        return Ok(0);
    }

    // Build schema metadata
    let instrument_id = format!("{}-PERP.BINANCE", symbol);
    let mut metadata = HashMap::new();
    metadata.insert("instrument_id".to_string(), instrument_id);
    metadata.insert("price_precision".to_string(), price_precision.to_string());
    metadata.insert("size_precision".to_string(), size_precision.to_string());

    let schema = trade_schema(metadata, bytes_len);

    // Build record batch
    let arrays: Vec<ArrayRef> = vec![
        Arc::new(prices.finish()),
        Arc::new(sizes.finish()),
        Arc::new(sides.finish()),
        Arc::new(trade_ids.finish()),
        Arc::new(ts_events.finish()),
        Arc::new(ts_inits.finish()),
    ];
    let batch = RecordBatch::try_new(schema.clone(), arrays)?;

    // Write Parquet file
    if let Some(parent) = output_parquet.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();

    let file = File::create(output_parquet)?;
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
    writer.write(&batch)?;
    writer.close()?;

    Ok(rows_written)
}

/// Convert all CSV/ZIP files in a directory.
pub fn convert_directory(
    input_dir: &Path,
    output_dir: &Path,
    symbol: &str,
    data_type: &str,
    price_precision: u8,
    size_precision: u8,
) -> Result<ConvertStats, ConvertError> {
    let mut stats = ConvertStats::default();

    // Find all CSV and ZIP files
    let entries: Vec<_> = std::fs::read_dir(input_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let path = e.path();
            path.extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext == "csv" || ext == "zip")
                .unwrap_or(false)
        })
        .collect();

    for entry in entries {
        let input_path = entry.path();
        let file_stem = input_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        // Output path: output_dir/SYMBOL/bars_1m/DATE.parquet
        let output_path = if data_type == "klines" {
            output_dir
                .join("bars_1m")
                .join(symbol)
                .join(format!("{}.parquet", file_stem))
        } else {
            output_dir
                .join("trades")
                .join(symbol)
                .join(format!("{}.parquet", file_stem))
        };

        let result = match data_type {
            "klines" => convert_klines_to_parquet(
                &input_path,
                &output_path,
                symbol,
                price_precision,
                size_precision,
            ),
            "trades" | "aggTrades" => convert_trades_to_parquet(
                &input_path,
                &output_path,
                symbol,
                price_precision,
                size_precision,
            ),
            _ => Err(ConvertError::InvalidFormat(format!(
                "Unknown data type: {}",
                data_type
            ))),
        };

        match result {
            Ok(rows) => {
                stats.rows_written += rows;
                stats.rows_read += rows; // For CSV, rows_read ~ rows_written
                stats.files_processed += 1;
            }
            Err(e) => {
                stats.errors.push(format!("{:?}: {}", input_path, e));
            }
        }
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_convert_klines_csv() {
        let temp_dir = TempDir::new().unwrap();
        let csv_path = temp_dir.path().join("test_klines.csv");
        let parquet_path = temp_dir.path().join("test_klines.parquet");

        // Create test CSV (Binance klines format)
        let csv_content = r#"1706540400000,100000.00,100050.00,99950.00,100025.00,150.5,1706540459999,15050000.00,1000,75.25,7525000.00,0
1706540460000,100025.00,100100.00,100000.00,100075.00,200.3,1706540519999,20030000.00,1500,100.15,10015000.00,0
"#;
        std::fs::write(&csv_path, csv_content).unwrap();

        let rows = convert_klines_to_parquet(&csv_path, &parquet_path, "BTCUSDT", 2, 3).unwrap();

        assert_eq!(rows, 2);
        assert!(parquet_path.exists());

        // Verify Parquet file
        let file = File::open(&parquet_path).unwrap();
        let reader =
            parquet::arrow::arrow_reader::ParquetRecordBatchReader::try_new(file, 1024).unwrap();
        let batches: Vec<_> = reader.collect();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].as_ref().unwrap().num_rows(), 2);
    }

    #[test]
    fn test_convert_trades_csv() {
        let temp_dir = TempDir::new().unwrap();
        let csv_path = temp_dir.path().join("test_trades.csv");
        let parquet_path = temp_dir.path().join("test_trades.parquet");

        // Create test CSV (Binance aggTrades format)
        let csv_content = r#"1000001,100000.50,0.001,999999,1000000,1706540400123,false,true
1000002,100001.25,0.002,1000001,1000001,1706540400456,true,true
"#;
        std::fs::write(&csv_path, csv_content).unwrap();

        let rows = convert_trades_to_parquet(&csv_path, &parquet_path, "BTCUSDT", 2, 3).unwrap();

        assert_eq!(rows, 2);
        assert!(parquet_path.exists());
    }

    #[test]
    fn test_parse_helpers() {
        assert_eq!(parse_f64("100.5").unwrap(), 100.5);
        assert_eq!(parse_u64("12345").unwrap(), 12345);
        assert!(parse_bool("true").unwrap());
        assert!(!parse_bool("false").unwrap());
    }
}
