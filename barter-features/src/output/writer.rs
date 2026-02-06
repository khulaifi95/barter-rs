//! Atomic parquet file writer.

use crate::error::Result;
use crate::features::{LargeTrade, ProfileEvent, TpoBracket};
use crate::output::schema::{
    OutputMetadata, large_trade_schema, profile_event_schema, tpo_bracket_schema,
};
use crate::precision::encode_for_output;
use arrow::array::{
    ArrayRef, BooleanBuilder, Int32Builder, Int64Builder, StringBuilder, UInt8Builder,
    UInt16Builder, UInt32Builder,
};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use tracing::debug;

/// Write a record batch to parquet atomically.
///
/// Uses .tmp + fsync + rename pattern for safety.
pub fn write_parquet_atomic(path: &Path, batch: &RecordBatch) -> Result<()> {
    let tmp_path = path.with_extension("parquet.tmp");

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(Default::default()))
        .build();

    // 1. Write to temp file
    {
        let file = File::create(&tmp_path)?;
        let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(props))?;
        writer.write(batch)?;
        writer.close()?;
    }

    // 2. Fsync temp file
    {
        let file = File::open(&tmp_path)?;
        file.sync_all()?;
    }

    // 3. Atomic rename
    std::fs::rename(&tmp_path, path)?;

    debug!("Wrote {} rows to {:?}", batch.num_rows(), path);
    Ok(())
}

/// Feature output writer.
pub struct FeatureWriter {
    metadata: OutputMetadata,
}

impl FeatureWriter {
    /// Create a new writer with the given metadata.
    pub fn new(metadata: OutputMetadata) -> Self {
        Self { metadata }
    }

    /// Write TPO brackets to parquet.
    pub fn write_tpo_brackets(&self, path: &Path, brackets: &[TpoBracket]) -> Result<()> {
        if brackets.is_empty() {
            return Ok(());
        }

        let batch = self.build_tpo_batch(brackets)?;
        write_parquet_atomic(path, &batch)
    }

    /// Write large trades to parquet.
    pub fn write_large_trades(&self, path: &Path, trades: &[LargeTrade]) -> Result<()> {
        self.write_large_trades_with_precision(path, trades, &self.metadata.source_precision)
    }

    /// Write large trades to parquet with explicit source precision.
    ///
    /// Use this when the trades came from a file with known precision (e.g., high-precision trades).
    pub fn write_large_trades_with_precision(
        &self,
        path: &Path,
        trades: &[LargeTrade],
        source_precision: &str,
    ) -> Result<()> {
        if trades.is_empty() {
            return Ok(());
        }

        let batch = self.build_large_trade_batch_with_precision(trades, source_precision)?;
        write_parquet_atomic(path, &batch)
    }

    /// Write profile events to parquet.
    pub fn write_profile_events(&self, path: &Path, events: &[ProfileEvent]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }

        let batch = self.build_profile_event_batch(events)?;
        write_parquet_atomic(path, &batch)
    }

    fn build_tpo_batch(&self, brackets: &[TpoBracket]) -> Result<RecordBatch> {
        let len = brackets.len();

        // Timestamps
        let mut ts_event = Int64Builder::with_capacity(len);
        let mut ts_init = Int64Builder::with_capacity(len);

        // Metadata
        let mut schema_version = StringBuilder::with_capacity(len, len * 8);
        let mut config_hash = StringBuilder::with_capacity(len, len * 16);
        let mut source_precision = StringBuilder::with_capacity(len, len * 8);
        let mut output_precision = Int32Builder::with_capacity(len);

        // Identifiers
        let mut instrument_id = StringBuilder::with_capacity(len, len * 32);
        let mut session_id = StringBuilder::with_capacity(len, len * 32);
        let mut session_date = StringBuilder::with_capacity(len, len * 10);

        // Bracket info
        let mut label = StringBuilder::with_capacity(len, len * 2);
        let mut bracket_index = UInt8Builder::with_capacity(len);
        let mut bracket_start_ts = Int64Builder::with_capacity(len);
        let mut bracket_end_ts = Int64Builder::with_capacity(len);

        // OHLCV
        let mut bracket_open = Int64Builder::with_capacity(len);
        let mut bracket_high = Int64Builder::with_capacity(len);
        let mut bracket_low = Int64Builder::with_capacity(len);
        let mut bracket_close = Int64Builder::with_capacity(len);
        let mut bracket_volume = Int64Builder::with_capacity(len);
        let mut bracket_buy_volume = Int64Builder::with_capacity(len);
        let mut bracket_sell_volume = Int64Builder::with_capacity(len);
        let mut bracket_delta = Int64Builder::with_capacity(len);
        let mut bracket_trade_count = UInt32Builder::with_capacity(len);
        let mut bracket_bar_count = UInt8Builder::with_capacity(len);
        let mut bracket_has_gap = BooleanBuilder::with_capacity(len);

        // Volume Profile
        let mut vol_poc = Int64Builder::with_capacity(len);
        let mut vol_vah = Int64Builder::with_capacity(len);
        let mut vol_val = Int64Builder::with_capacity(len);
        let mut vol_total = Int64Builder::with_capacity(len);

        // TPO Profile
        let mut tpo_poc = Int64Builder::with_capacity(len);
        let mut tpo_vah = Int64Builder::with_capacity(len);
        let mut tpo_val = Int64Builder::with_capacity(len);
        let mut tpo_total_periods = UInt16Builder::with_capacity(len);

        // Initial Balance
        let mut ib_high = Int64Builder::with_capacity(len);
        let mut ib_low = Int64Builder::with_capacity(len);
        let mut ib_complete = BooleanBuilder::with_capacity(len);

        // Analysis
        let mut poc_divergence = Int64Builder::with_capacity(len);
        let mut session_type = StringBuilder::with_capacity(len, len * 8);

        for b in brackets {
            ts_event.append_value(b.ts_event);
            ts_init.append_value(b.ts_init);

            schema_version.append_value(&self.metadata.schema_version);
            config_hash.append_value(&self.metadata.config_hash);
            source_precision.append_value(&self.metadata.source_precision);
            output_precision.append_value(self.metadata.output_precision);

            instrument_id.append_value(&b.instrument_id);
            session_id.append_value(&b.session_id);
            session_date.append_value(&b.session_date);

            label.append_value(&b.label);
            bracket_index.append_value(b.bracket_index);
            bracket_start_ts.append_value(b.bracket_start_ts);
            bracket_end_ts.append_value(b.bracket_end_ts);

            bracket_open.append_value(encode_for_output(b.bracket_open));
            bracket_high.append_value(encode_for_output(b.bracket_high));
            bracket_low.append_value(encode_for_output(b.bracket_low));
            bracket_close.append_value(encode_for_output(b.bracket_close));
            bracket_volume.append_value(encode_for_output(b.bracket_volume));
            bracket_buy_volume.append_value(encode_for_output(b.bracket_buy_volume));
            bracket_sell_volume.append_value(encode_for_output(b.bracket_sell_volume));
            bracket_delta.append_value(encode_for_output(b.bracket_delta));
            bracket_trade_count.append_value(b.bracket_trade_count);
            bracket_bar_count.append_value(b.bracket_bar_count);
            bracket_has_gap.append_value(b.bracket_has_gap);

            vol_poc.append_value(encode_for_output(b.vol_poc));
            vol_vah.append_value(encode_for_output(b.vol_vah));
            vol_val.append_value(encode_for_output(b.vol_val));
            vol_total.append_value(encode_for_output(b.vol_total));

            tpo_poc.append_value(encode_for_output(b.tpo_poc));
            tpo_vah.append_value(encode_for_output(b.tpo_vah));
            tpo_val.append_value(encode_for_output(b.tpo_val));
            tpo_total_periods.append_value(b.tpo_total_periods);

            ib_high.append_value(encode_for_output(b.ib_high));
            ib_low.append_value(encode_for_output(b.ib_low));
            ib_complete.append_value(b.ib_complete);

            poc_divergence.append_value(encode_for_output(b.poc_divergence));
            session_type.append_value(&b.session_type);
        }

        let schema = Arc::new(tpo_bracket_schema());
        let columns: Vec<ArrayRef> = vec![
            Arc::new(ts_event.finish()),
            Arc::new(ts_init.finish()),
            Arc::new(schema_version.finish()),
            Arc::new(config_hash.finish()),
            Arc::new(source_precision.finish()),
            Arc::new(output_precision.finish()),
            Arc::new(instrument_id.finish()),
            Arc::new(session_id.finish()),
            Arc::new(session_date.finish()),
            Arc::new(label.finish()),
            Arc::new(bracket_index.finish()),
            Arc::new(bracket_start_ts.finish()),
            Arc::new(bracket_end_ts.finish()),
            Arc::new(bracket_open.finish()),
            Arc::new(bracket_high.finish()),
            Arc::new(bracket_low.finish()),
            Arc::new(bracket_close.finish()),
            Arc::new(bracket_volume.finish()),
            Arc::new(bracket_buy_volume.finish()),
            Arc::new(bracket_sell_volume.finish()),
            Arc::new(bracket_delta.finish()),
            Arc::new(bracket_trade_count.finish()),
            Arc::new(bracket_bar_count.finish()),
            Arc::new(bracket_has_gap.finish()),
            Arc::new(vol_poc.finish()),
            Arc::new(vol_vah.finish()),
            Arc::new(vol_val.finish()),
            Arc::new(vol_total.finish()),
            Arc::new(tpo_poc.finish()),
            Arc::new(tpo_vah.finish()),
            Arc::new(tpo_val.finish()),
            Arc::new(tpo_total_periods.finish()),
            Arc::new(ib_high.finish()),
            Arc::new(ib_low.finish()),
            Arc::new(ib_complete.finish()),
            Arc::new(poc_divergence.finish()),
            Arc::new(session_type.finish()),
        ];

        Ok(RecordBatch::try_new(schema, columns)?)
    }

    #[allow(dead_code)]
    fn build_large_trade_batch(&self, trades: &[LargeTrade]) -> Result<RecordBatch> {
        self.build_large_trade_batch_with_precision(trades, &self.metadata.source_precision)
    }

    fn build_large_trade_batch_with_precision(
        &self,
        trades: &[LargeTrade],
        source_prec: &str,
    ) -> Result<RecordBatch> {
        let len = trades.len();

        let mut ts_event = Int64Builder::with_capacity(len);
        let mut ts_init = Int64Builder::with_capacity(len);
        let mut schema_version = StringBuilder::with_capacity(len, len * 8);
        let mut config_hash = StringBuilder::with_capacity(len, len * 16);
        let mut source_precision = StringBuilder::with_capacity(len, len * 8);
        let mut output_precision = Int32Builder::with_capacity(len);
        let mut instrument_id = StringBuilder::with_capacity(len, len * 32);
        let mut trade_id = StringBuilder::with_capacity(len, len * 32);
        let mut price = Int64Builder::with_capacity(len);
        let mut size = Int64Builder::with_capacity(len);
        let mut side = StringBuilder::with_capacity(len, len * 4);
        let mut notional_usd = Int64Builder::with_capacity(len);
        let mut category = StringBuilder::with_capacity(len, len * 8);
        let mut threshold_used = Int64Builder::with_capacity(len);
        let mut threshold_mode = StringBuilder::with_capacity(len, len * 16);

        for t in trades {
            ts_event.append_value(t.ts_event);
            ts_init.append_value(t.ts_init);
            schema_version.append_value(&self.metadata.schema_version);
            config_hash.append_value(&self.metadata.config_hash);
            source_precision.append_value(source_prec);
            output_precision.append_value(self.metadata.output_precision);
            instrument_id.append_value(&t.instrument_id);
            trade_id.append_value(&t.trade_id);
            price.append_value(encode_for_output(t.price));
            size.append_value(encode_for_output(t.size));
            side.append_value(&t.side);
            // Use consistent 1e9 scaling for all monetary values
            notional_usd.append_value(encode_for_output(t.notional_usd));
            category.append_value(t.category.as_str());
            threshold_used.append_value(encode_for_output(t.threshold_used));
            threshold_mode.append_value(&t.threshold_mode);
        }

        let schema = Arc::new(large_trade_schema());
        let columns: Vec<ArrayRef> = vec![
            Arc::new(ts_event.finish()),
            Arc::new(ts_init.finish()),
            Arc::new(schema_version.finish()),
            Arc::new(config_hash.finish()),
            Arc::new(source_precision.finish()),
            Arc::new(output_precision.finish()),
            Arc::new(instrument_id.finish()),
            Arc::new(trade_id.finish()),
            Arc::new(price.finish()),
            Arc::new(size.finish()),
            Arc::new(side.finish()),
            Arc::new(notional_usd.finish()),
            Arc::new(category.finish()),
            Arc::new(threshold_used.finish()),
            Arc::new(threshold_mode.finish()),
        ];

        Ok(RecordBatch::try_new(schema, columns)?)
    }

    fn build_profile_event_batch(&self, events: &[ProfileEvent]) -> Result<RecordBatch> {
        let len = events.len();

        let mut ts_event = Int64Builder::with_capacity(len);
        let mut ts_init = Int64Builder::with_capacity(len);
        let mut schema_version = StringBuilder::with_capacity(len, len * 8);
        let mut config_hash = StringBuilder::with_capacity(len, len * 16);
        let mut source_precision = StringBuilder::with_capacity(len, len * 8);
        let mut output_precision = Int32Builder::with_capacity(len);
        let mut instrument_id = StringBuilder::with_capacity(len, len * 32);
        let mut session_id = StringBuilder::with_capacity(len, len * 32);
        let mut event_type = StringBuilder::with_capacity(len, len * 16);
        let mut vol_poc = Int64Builder::with_capacity(len);
        let mut vol_vah = Int64Builder::with_capacity(len);
        let mut vol_val = Int64Builder::with_capacity(len);
        let mut tpo_poc = Int64Builder::with_capacity(len);
        let mut tpo_vah = Int64Builder::with_capacity(len);
        let mut tpo_val = Int64Builder::with_capacity(len);
        let mut ib_high = Int64Builder::with_capacity(len);
        let mut ib_low = Int64Builder::with_capacity(len);
        let mut break_direction = StringBuilder::with_capacity(len, len * 4);
        let mut break_price = Int64Builder::with_capacity(len);
        let mut shift_amount = Int64Builder::with_capacity(len);
        let mut previous_value = Int64Builder::with_capacity(len);
        let mut gap_start_ts = Int64Builder::with_capacity(len);
        let mut gap_end_ts = Int64Builder::with_capacity(len);
        let mut missing_bars = UInt32Builder::with_capacity(len);

        for e in events {
            ts_event.append_value(e.ts_event);
            ts_init.append_value(e.ts_init);
            schema_version.append_value(&self.metadata.schema_version);
            config_hash.append_value(&self.metadata.config_hash);
            source_precision.append_value(&self.metadata.source_precision);
            output_precision.append_value(self.metadata.output_precision);
            instrument_id.append_value(&e.instrument_id);
            session_id.append_value(&e.session_id);
            event_type.append_value(e.event_type.as_str());
            vol_poc.append_value(encode_for_output(e.vol_poc));
            vol_vah.append_value(encode_for_output(e.vol_vah));
            vol_val.append_value(encode_for_output(e.vol_val));
            tpo_poc.append_value(encode_for_output(e.tpo_poc));
            tpo_vah.append_value(encode_for_output(e.tpo_vah));
            tpo_val.append_value(encode_for_output(e.tpo_val));
            ib_high.append_value(encode_for_output(e.ib_high));
            ib_low.append_value(encode_for_output(e.ib_low));

            // Nullable fields
            match &e.break_direction {
                Some(v) => break_direction.append_value(v),
                None => break_direction.append_null(),
            }
            match e.break_price {
                Some(v) => break_price.append_value(encode_for_output(v)),
                None => break_price.append_null(),
            }
            match e.shift_amount {
                Some(v) => shift_amount.append_value(encode_for_output(v)),
                None => shift_amount.append_null(),
            }
            match e.previous_value {
                Some(v) => previous_value.append_value(encode_for_output(v)),
                None => previous_value.append_null(),
            }
            match e.gap_start_ts {
                Some(v) => gap_start_ts.append_value(v),
                None => gap_start_ts.append_null(),
            }
            match e.gap_end_ts {
                Some(v) => gap_end_ts.append_value(v),
                None => gap_end_ts.append_null(),
            }
            match e.missing_bars {
                Some(v) => missing_bars.append_value(v),
                None => missing_bars.append_null(),
            }
        }

        let schema = Arc::new(profile_event_schema());
        let columns: Vec<ArrayRef> = vec![
            Arc::new(ts_event.finish()),
            Arc::new(ts_init.finish()),
            Arc::new(schema_version.finish()),
            Arc::new(config_hash.finish()),
            Arc::new(source_precision.finish()),
            Arc::new(output_precision.finish()),
            Arc::new(instrument_id.finish()),
            Arc::new(session_id.finish()),
            Arc::new(event_type.finish()),
            Arc::new(vol_poc.finish()),
            Arc::new(vol_vah.finish()),
            Arc::new(vol_val.finish()),
            Arc::new(tpo_poc.finish()),
            Arc::new(tpo_vah.finish()),
            Arc::new(tpo_val.finish()),
            Arc::new(ib_high.finish()),
            Arc::new(ib_low.finish()),
            Arc::new(break_direction.finish()),
            Arc::new(break_price.finish()),
            Arc::new(shift_amount.finish()),
            Arc::new(previous_value.finish()),
            Arc::new(gap_start_ts.finish()),
            Arc::new(gap_end_ts.finish()),
            Arc::new(missing_bars.finish()),
        ];

        Ok(RecordBatch::try_new(schema, columns)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_test_bracket() -> TpoBracket {
        TpoBracket {
            ts_event: 1_706_832_000_000_000_000,
            ts_init: 1_706_832_001_000_000_000,
            instrument_id: "BTCUSDT".to_string(),
            session_id: "2024-02-02T00:00:00Z".to_string(),
            session_date: "2024-02-02".to_string(),
            label: "A".to_string(),
            bracket_index: 0,
            bracket_start_ts: 1_706_832_000_000_000_000,
            bracket_end_ts: 1_706_833_800_000_000_000,
            bracket_open: 100000.0,
            bracket_high: 100500.0,
            bracket_low: 99500.0,
            bracket_close: 100200.0,
            bracket_volume: 1000.0,
            bracket_buy_volume: 600.0,
            bracket_sell_volume: 400.0,
            bracket_delta: 200.0,
            bracket_trade_count: 1000,
            bracket_bar_count: 30,
            bracket_has_gap: false,
            vol_poc: 100000.0,
            vol_vah: 100300.0,
            vol_val: 99700.0,
            vol_total: 5000.0,
            tpo_poc: 100050.0,
            tpo_vah: 100350.0,
            tpo_val: 99750.0,
            tpo_total_periods: 1,
            ib_high: 100500.0,
            ib_low: 99500.0,
            ib_complete: false,
            poc_divergence: 50.0,
            session_type: "normal".to_string(),
        }
    }

    #[test]
    fn test_write_tpo_brackets() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tpo_brackets.parquet");

        let metadata = OutputMetadata {
            schema_version: "1.2.0".to_string(),
            config_hash: "abc123".to_string(),
            source_precision: "high".to_string(),
            output_precision: 9,
        };

        let writer = FeatureWriter::new(metadata);
        let brackets = vec![make_test_bracket()];

        writer.write_tpo_brackets(&path, &brackets).unwrap();

        assert!(path.exists());
    }

    #[test]
    fn test_atomic_write_creates_no_tmp() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.parquet");

        let metadata = OutputMetadata::default();
        let writer = FeatureWriter::new(metadata);
        let brackets = vec![make_test_bracket()];

        writer.write_tpo_brackets(&path, &brackets).unwrap();

        // Tmp file should not exist after write
        let tmp_path = path.with_extension("parquet.tmp");
        assert!(!tmp_path.exists());
    }
}
