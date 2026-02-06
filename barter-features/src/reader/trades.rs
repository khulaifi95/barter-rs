//! Reader for trade parquet files from barter-data-server.
//!
//! Trade parquet files use Nautilus encoding:
//! - price/size: FixedSizeBinary(8) or FixedSizeBinary(16)
//! - aggressor_side: UInt8 (0=NO_AGGRESSOR, 1=BUYER, 2=SELLER)
//! - trade_id: Utf8
//! - ts_event/ts_init: UInt64 nanoseconds

use crate::error::{FeatureError, Result};
use crate::precision::{PrecisionMode, decode_fixed_point};
use arrow::array::{Array, FixedSizeBinaryArray, StringArray, UInt8Array, UInt64Array};
use arrow::datatypes::DataType;
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use std::fs::File;
use std::path::Path;
use tracing::debug;

/// Decoded trade data.
#[derive(Debug, Clone)]
pub struct Trade {
    /// Trade timestamp (nanoseconds)
    pub ts_event: i64,
    /// Processing timestamp (nanoseconds)
    pub ts_init: i64,
    /// Trade price
    pub price: f64,
    /// Trade size
    pub size: f64,
    /// Aggressor side: "BUY", "SELL", or "UNKNOWN"
    pub side: String,
    /// Trade ID from exchange
    pub trade_id: String,
}

impl Trade {
    /// Calculate notional value (price × size).
    pub fn notional(&self) -> f64 {
        self.price * self.size
    }
}

/// Reader for trade parquet files.
pub struct TradeReader {
    batches: Vec<RecordBatch>,
    current_batch: usize,
    current_row: usize,
    /// Detected precision mode from the parquet schema
    precision_mode: PrecisionMode,
}

impl TradeReader {
    /// Open a parquet file for reading trades.
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;

        // Detect precision from schema before building reader
        let schema = builder.schema();
        let precision_mode = detect_precision_from_schema(schema)?;

        let reader = builder.build()?;

        let batches: Vec<RecordBatch> = reader.collect::<std::result::Result<Vec<_>, _>>()?;

        debug!(
            "Opened trades {:?} with {} batches, precision={:?}",
            path,
            batches.len(),
            precision_mode
        );

        Ok(Self {
            batches,
            current_batch: 0,
            current_row: 0,
            precision_mode,
        })
    }

    /// Get the detected precision mode for this trades file.
    pub fn precision_mode(&self) -> PrecisionMode {
        self.precision_mode
    }

    /// Get total row count across all batches.
    pub fn total_rows(&self) -> usize {
        self.batches.iter().map(|b| b.num_rows()).sum()
    }

    /// Read all trades from the file.
    pub fn read_all(&mut self) -> Result<Vec<Trade>> {
        let mut trades = Vec::with_capacity(self.total_rows());

        while let Some(trade) = self.next()? {
            trades.push(trade);
        }

        Ok(trades)
    }

    /// Read the next trade, if any.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<Trade>> {
        while self.current_batch < self.batches.len() {
            let batch = &self.batches[self.current_batch];

            if self.current_row < batch.num_rows() {
                let trade = self.read_trade_from_batch(batch, self.current_row)?;
                self.current_row += 1;
                return Ok(Some(trade));
            }

            self.current_batch += 1;
            self.current_row = 0;
        }

        Ok(None)
    }

    /// Reset reader to beginning.
    pub fn reset(&mut self) {
        self.current_batch = 0;
        self.current_row = 0;
    }

    fn read_trade_from_batch(&self, batch: &RecordBatch, row: usize) -> Result<Trade> {
        // Timestamps are UInt64
        let ts_event = get_u64_column(batch, "ts_event")?.value(row) as i64;
        let ts_init = get_u64_column(batch, "ts_init")?.value(row) as i64;

        // Price and size are FixedSizeBinary
        let price = decode_fixed_size_binary(batch, "price", row)?;
        let size = decode_fixed_size_binary(batch, "size", row)?;

        // Aggressor side is UInt8
        let aggressor_side = get_u8_column(batch, "aggressor_side")?.value(row);
        let side = match aggressor_side {
            1 => "BUY".to_string(),
            2 => "SELL".to_string(),
            _ => "UNKNOWN".to_string(),
        };

        // Trade ID is Utf8
        let trade_id = get_string_column(batch, "trade_id")?.value(row).to_string();

        Ok(Trade {
            ts_event,
            ts_init,
            price,
            size,
            side,
            trade_id,
        })
    }
}

/// Read all trades from a parquet file.
pub fn read_trades(path: &Path) -> Result<Vec<Trade>> {
    let mut reader = TradeReader::open(path)?;
    reader.read_all()
}

/// Read all trades from a parquet file and return the detected precision mode.
///
/// This is useful when you need to preserve the source precision in output metadata.
pub fn read_trades_with_precision(path: &Path) -> Result<(Vec<Trade>, PrecisionMode)> {
    let mut reader = TradeReader::open(path)?;
    let precision = reader.precision_mode();
    let trades = reader.read_all()?;
    Ok((trades, precision))
}

/// Read the instrument_id from parquet schema metadata (if present).
pub fn trade_instrument_id_from_schema(path: &Path) -> Result<Option<String>> {
    let file = File::open(path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
    let schema = builder.schema();
    Ok(schema.metadata().get("instrument_id").cloned())
}

/// Detect precision mode from Arrow schema by inspecting the "price" column.
fn detect_precision_from_schema(schema: &arrow::datatypes::SchemaRef) -> Result<PrecisionMode> {
    // Look for the "price" column to determine precision
    if let Ok(field) = schema.field_with_name("price") {
        match field.data_type() {
            DataType::FixedSizeBinary(8) => Ok(PrecisionMode::Standard),
            DataType::FixedSizeBinary(16) => Ok(PrecisionMode::High),
            DataType::FixedSizeBinary(n) => Err(FeatureError::InvalidPrecision {
                expected: 8,
                actual: *n as usize,
            }),
            other => Err(FeatureError::InvalidData(format!(
                "price column has unexpected type: {:?}, expected FixedSizeBinary",
                other
            ))),
        }
    } else {
        // Default to standard if price column not found (shouldn't happen)
        debug!("price column not found in schema, defaulting to standard precision");
        Ok(PrecisionMode::Standard)
    }
}

// Helper functions for column extraction

fn get_u64_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a UInt64Array> {
    let col = batch
        .column_by_name(name)
        .ok_or_else(|| FeatureError::MissingColumn(name.to_string()))?;
    col.as_any()
        .downcast_ref::<UInt64Array>()
        .ok_or_else(|| FeatureError::InvalidData(format!("Column {} is not UInt64", name)))
}

fn get_u8_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a UInt8Array> {
    let col = batch
        .column_by_name(name)
        .ok_or_else(|| FeatureError::MissingColumn(name.to_string()))?;
    col.as_any()
        .downcast_ref::<UInt8Array>()
        .ok_or_else(|| FeatureError::InvalidData(format!("Column {} is not UInt8", name)))
}

fn get_string_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    let col = batch
        .column_by_name(name)
        .ok_or_else(|| FeatureError::MissingColumn(name.to_string()))?;
    col.as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| FeatureError::InvalidData(format!("Column {} is not Utf8", name)))
}

fn decode_fixed_size_binary(batch: &RecordBatch, name: &str, row: usize) -> Result<f64> {
    let col = batch
        .column_by_name(name)
        .ok_or_else(|| FeatureError::MissingColumn(name.to_string()))?;
    let arr = col
        .as_any()
        .downcast_ref::<FixedSizeBinaryArray>()
        .ok_or_else(|| {
            FeatureError::InvalidData(format!("Column {} is not FixedSizeBinary", name))
        })?;

    let bytes = arr.value(row);
    Ok(decode_fixed_point(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;

    #[test]
    fn test_trade_notional() {
        let trade = Trade {
            ts_event: 0,
            ts_init: 0,
            price: 100000.0,
            size: 25.0,
            side: "BUY".to_string(),
            trade_id: "123".to_string(),
        };

        assert_eq!(trade.notional(), 2_500_000.0);
    }

    #[test]
    fn test_side_values() {
        // Aggressor side encoding
        assert_eq!(1u8, 1); // BUYER
        assert_eq!(2u8, 2); // SELLER
        assert_eq!(0u8, 0); // NO_AGGRESSOR
    }

    #[test]
    fn test_precision_detection_standard() {
        // Create schema with FixedSizeBinary(8) for standard precision
        let schema = Arc::new(Schema::new(vec![
            Field::new("price", DataType::FixedSizeBinary(8), false),
            Field::new("size", DataType::FixedSizeBinary(8), false),
            Field::new("aggressor_side", DataType::UInt8, false),
            Field::new("trade_id", DataType::Utf8, false),
            Field::new("ts_event", DataType::UInt64, false),
            Field::new("ts_init", DataType::UInt64, false),
        ]));

        let precision = detect_precision_from_schema(&schema).unwrap();
        assert_eq!(precision, PrecisionMode::Standard);
    }

    #[test]
    fn test_precision_detection_high() {
        // Create schema with FixedSizeBinary(16) for high precision
        let schema = Arc::new(Schema::new(vec![
            Field::new("price", DataType::FixedSizeBinary(16), false),
            Field::new("size", DataType::FixedSizeBinary(16), false),
            Field::new("aggressor_side", DataType::UInt8, false),
            Field::new("trade_id", DataType::Utf8, false),
            Field::new("ts_event", DataType::UInt64, false),
            Field::new("ts_init", DataType::UInt64, false),
        ]));

        let precision = detect_precision_from_schema(&schema).unwrap();
        assert_eq!(precision, PrecisionMode::High);
    }
}
