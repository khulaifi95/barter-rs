//! Validate extended bars Parquet schema and sample data.
//!
//! Usage:
//!   cargo run -p barter-data-server --bin check-extended-bars -- /path/to/file.parquet

use arrow::array::{Array, Float64Array, Int64Array};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use std::collections::HashSet;
use std::env;
use std::fs::File;

const REQUIRED_COLUMNS: &[&str] = &[
    "ts_event",
    "ts_init",
    "ts_open",
    "instrument_id",
    "open",
    "high",
    "low",
    "close",
    "volume",
    "quote_volume",
    "trade_count",
    "buy_volume",
    "sell_volume",
    "delta",
    "cvd",
    "open_interest",
    "oi_change",
    "funding_rate",
    "bid_price",
    "bid_size",
    "ask_price",
    "ask_size",
    "spread_bps",
    "book_imbalance",
    "liq_buy_usd",
    "liq_sell_usd",
    "liq_total_usd",
    "liq_count",
    "bid_depth_10bps_base",
    "ask_depth_10bps_base",
    "bid_depth_10bps_usd",
    "ask_depth_10bps_usd",
    "depth_imb_10bps",
    "bid_depth_50bps_base",
    "ask_depth_50bps_base",
    "bid_depth_50bps_usd",
    "ask_depth_50bps_usd",
    "depth_imb_50bps",
    "bid_depth_100bps_base",
    "ask_depth_100bps_base",
    "bid_depth_100bps_usd",
    "ask_depth_100bps_usd",
    "depth_imb_100bps",
];

fn any_non_zero_i64(array: &Int64Array) -> bool {
    (0..array.len()).any(|i| array.value(i) != 0)
}

fn any_non_zero_f64(array: &Float64Array) -> bool {
    (0..array.len()).any(|i| array.value(i).abs() > 0.0)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args().nth(1).ok_or("missing parquet path argument")?;
    let file = File::open(&path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
    let schema = builder.schema().clone();

    let schema_meta = schema.metadata();
    let schema_version = schema_meta
        .get("schema_version")
        .map(|v| v.as_str())
        .unwrap_or("");
    if schema_version != "2" {
        return Err(format!("schema_version expected 2, got {}", schema_version).into());
    }

    let present: HashSet<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    let missing: Vec<&str> = REQUIRED_COLUMNS
        .iter()
        .copied()
        .filter(|c| !present.contains(c))
        .collect();
    if !missing.is_empty() {
        return Err(format!("missing columns: {:?}", missing).into());
    }

    let mut reader = builder.build()?;
    let mut checked = false;
    while let Some(batch) = reader.next() {
        let batch = batch?;
        let liq_total = batch
            .column_by_name("liq_total_usd")
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .ok_or("liq_total_usd column not int64")?;
        let depth_usd = batch
            .column_by_name("bid_depth_10bps_usd")
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .ok_or("bid_depth_10bps_usd column not int64")?;
        let depth_imb = batch
            .column_by_name("depth_imb_10bps")
            .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
            .ok_or("depth_imb_10bps column not float64")?;

        if any_non_zero_i64(liq_total) && any_non_zero_i64(depth_usd) && any_non_zero_f64(depth_imb)
        {
            checked = true;
            break;
        }
    }

    if !checked {
        return Err("extended bars appear all-zero for liq/depth fields".into());
    }

    println!("Extended bars schema + sample data OK: {}", path);
    Ok(())
}
