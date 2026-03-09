//! Reader for extended bar parquet files from barter-data-server.
//!
//! Extended bars use Int64 × 1e9 encoding for price/volume fields.
//! Timestamps are UInt64 nanoseconds.

use crate::error::{FeatureError, Result};
use crate::precision::decode_extended_bar_value;
use arrow::array::{Array, Int64Array, StringArray, UInt64Array};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::{ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder};
use std::fs::File;
use std::path::Path;
use tracing::debug;

/// Decoded extended bar data.
#[derive(Debug, Clone)]
pub struct ExtendedBar {
    /// Bar close timestamp (nanoseconds)
    pub ts_event: i64,
    /// Processing timestamp (nanoseconds)
    pub ts_init: i64,
    /// Bar open timestamp (nanoseconds)
    pub ts_open: i64,
    /// Instrument identifier
    pub instrument_id: String,
    /// Open price
    pub open: f64,
    /// High price
    pub high: f64,
    /// Low price
    pub low: f64,
    /// Close price
    pub close: f64,
    /// Total volume (base)
    pub volume: f64,
    /// Quote volume
    pub quote_volume: f64,
    /// Number of trades
    pub trade_count: u64,
    /// Buy volume
    pub buy_volume: f64,
    /// Sell volume
    pub sell_volume: f64,
    /// Delta (buy - sell)
    pub delta: f64,
    /// Cumulative volume delta
    pub cvd: f64,
    /// Open interest
    pub open_interest: f64,
    /// Open interest change
    pub oi_change: f64,
    /// Funding rate
    pub funding_rate: f64,
}

impl ExtendedBar {
    /// Notional value in USD (close × volume).
    pub fn notional(&self) -> f64 {
        self.close * self.volume
    }

    /// Bar interval calculated from ts_open to ts_event (seconds).
    pub fn interval_secs(&self) -> u64 {
        if self.ts_event > self.ts_open {
            ((self.ts_event - self.ts_open) / 1_000_000_000) as u64
        } else {
            60 // Default to 1 minute
        }
    }
}

/// Streaming reader for extended bar parquet files.
///
/// Reads one RecordBatch at a time instead of loading the entire file into memory.
pub struct ExtendedBarReader {
    /// Lazy batch iterator — only one batch in memory at a time
    batch_reader: ParquetRecordBatchReader,
    /// Current batch being iterated (replaced as we advance)
    current_batch: Option<RecordBatch>,
    current_row: usize,
}

impl ExtendedBarReader {
    /// Open a parquet file for streaming reads.
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
        let batch_reader = builder.build()?;

        debug!("Opened {:?} for streaming read", path);

        Ok(Self {
            batch_reader,
            current_batch: None,
            current_row: 0,
        })
    }

    /// Read all bars from the file.
    pub fn read_all(&mut self) -> Result<Vec<ExtendedBar>> {
        let mut bars = Vec::new();

        while let Some(bar) = self.next()? {
            bars.push(bar);
        }

        Ok(bars)
    }

    /// Read the next bar, if any.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<ExtendedBar>> {
        loop {
            // Try to read from current batch
            if let Some(batch) = &self.current_batch
                && self.current_row < batch.num_rows()
            {
                let bar = self.read_bar_from_batch(batch, self.current_row)?;
                self.current_row += 1;
                return Ok(Some(bar));
            }

            // Advance to next batch (drops the previous one, freeing memory)
            match self.batch_reader.next() {
                Some(Ok(batch)) => {
                    self.current_batch = Some(batch);
                    self.current_row = 0;
                }
                Some(Err(e)) => return Err(FeatureError::Arrow(e)),
                None => return Ok(None),
            }
        }
    }

    fn read_bar_from_batch(&self, batch: &RecordBatch, row: usize) -> Result<ExtendedBar> {
        // Timestamps are UInt64
        let ts_event = get_u64_column(batch, "ts_event")?.value(row) as i64;
        let ts_init = get_u64_column(batch, "ts_init")?.value(row) as i64;
        let ts_open = get_u64_column_opt(batch, "ts_open")
            .map(|arr| arr.value(row) as i64)
            .unwrap_or(ts_event - 60_000_000_000); // Default: 1 minute before close

        // Get instrument_id
        let instrument_id = get_string_column_opt(batch, "instrument_id")
            .map(|arr| arr.value(row).to_string())
            .unwrap_or_default();

        // OHLCV - decode from Int64 × 1e9
        let open = decode_i64_price(batch, "open", row)?;
        let high = decode_i64_price(batch, "high", row)?;
        let low = decode_i64_price(batch, "low", row)?;
        let close = decode_i64_price(batch, "close", row)?;
        let volume = decode_i64_price(batch, "volume", row)?;
        let quote_volume = decode_i64_price_opt(batch, "quote_volume", row).unwrap_or(0.0);

        // Trade count is UInt64
        let trade_count = get_u64_column_opt(batch, "trade_count")
            .map(|arr| arr.value(row))
            .unwrap_or(0);

        // Extended fields
        let buy_volume = decode_i64_price_opt(batch, "buy_volume", row).unwrap_or(0.0);
        let sell_volume = decode_i64_price_opt(batch, "sell_volume", row).unwrap_or(0.0);
        let delta = decode_i64_price_opt(batch, "delta", row).unwrap_or(buy_volume - sell_volume);
        let cvd = decode_i64_price_opt(batch, "cvd", row).unwrap_or(0.0);

        // Derivatives
        let open_interest = decode_i64_price_opt(batch, "open_interest", row).unwrap_or(0.0);
        let oi_change = decode_i64_price_opt(batch, "oi_change", row).unwrap_or(0.0);
        let funding_rate = get_f64_column_opt(batch, "funding_rate")
            .map(|arr| arr.value(row))
            .unwrap_or(0.0);

        Ok(ExtendedBar {
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
            funding_rate,
        })
    }
}

/// Read all extended bars from a parquet file.
pub fn read_extended_bars(path: &Path) -> Result<Vec<ExtendedBar>> {
    let mut reader = ExtendedBarReader::open(path)?;
    reader.read_all()
}

// Helper functions for column extraction

fn get_i64_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int64Array> {
    let col = batch
        .column_by_name(name)
        .ok_or_else(|| FeatureError::MissingColumn(name.to_string()))?;
    col.as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| FeatureError::InvalidData(format!("Column {} is not Int64", name)))
}

fn get_u64_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a UInt64Array> {
    let col = batch
        .column_by_name(name)
        .ok_or_else(|| FeatureError::MissingColumn(name.to_string()))?;
    col.as_any()
        .downcast_ref::<UInt64Array>()
        .ok_or_else(|| FeatureError::InvalidData(format!("Column {} is not UInt64", name)))
}

fn get_u64_column_opt<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a UInt64Array> {
    batch
        .column_by_name(name)
        .and_then(|col| col.as_any().downcast_ref::<UInt64Array>())
}

fn get_string_column_opt<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a StringArray> {
    batch
        .column_by_name(name)
        .and_then(|col| col.as_any().downcast_ref::<StringArray>())
}

fn get_f64_column_opt<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Option<&'a arrow::array::Float64Array> {
    batch
        .column_by_name(name)
        .and_then(|col| col.as_any().downcast_ref::<arrow::array::Float64Array>())
}

fn decode_i64_price(batch: &RecordBatch, name: &str, row: usize) -> Result<f64> {
    let col = get_i64_column(batch, name)?;
    Ok(decode_extended_bar_value(col.value(row)))
}

fn decode_i64_price_opt(batch: &RecordBatch, name: &str, row: usize) -> Option<f64> {
    batch
        .column_by_name(name)
        .and_then(|col| col.as_any().downcast_ref::<Int64Array>())
        .map(|arr| decode_extended_bar_value(arr.value(row)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extended_bar_notional() {
        let bar = ExtendedBar {
            ts_event: 0,
            ts_init: 0,
            ts_open: 0,
            instrument_id: "TEST".to_string(),
            open: 100.0,
            high: 105.0,
            low: 95.0,
            close: 102.0,
            volume: 10.0,
            quote_volume: 1020.0,
            trade_count: 100,
            buy_volume: 6.0,
            sell_volume: 4.0,
            delta: 2.0,
            cvd: 100.0,
            open_interest: 50000.0,
            oi_change: 100.0,
            funding_rate: 0.0001,
        };

        assert_eq!(bar.notional(), 1020.0);
    }

    #[test]
    fn test_decode_extended_bar_value() {
        // Encode a value as Int64 × 1e9
        let original: f64 = 78523.456789;
        let encoded = (original * 1e9).round() as i64;
        let decoded = decode_extended_bar_value(encoded);

        assert!((decoded - original).abs() < 1e-6);
    }

    #[test]
    fn test_interval_secs() {
        let bar = ExtendedBar {
            ts_event: 1_706_832_060_000_000_000, // 1 minute after open
            ts_init: 0,
            ts_open: 1_706_832_000_000_000_000,
            instrument_id: "TEST".to_string(),
            open: 100.0,
            high: 105.0,
            low: 95.0,
            close: 102.0,
            volume: 10.0,
            quote_volume: 1020.0,
            trade_count: 100,
            buy_volume: 6.0,
            sell_volume: 4.0,
            delta: 2.0,
            cvd: 100.0,
            open_interest: 50000.0,
            oi_change: 100.0,
            funding_rate: 0.0001,
        };

        assert_eq!(bar.interval_secs(), 60);
    }
}
