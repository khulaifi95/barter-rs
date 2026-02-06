//! Sample Parquet file generator for Nautilus compatibility testing.
//!
//! Generates test files:
//! - `/tmp/test_bars.parquet` - 10 sample 1-minute bars
//! - `/tmp/test_trades.parquet` - 100 sample trades
//!
//! Run with: `cargo run -p barter-data-server --bin generate-sample-parquet`
//!
//! **NOTE**: Uses `NAUTILUS_PRECISION` (high or standard) to select 16-byte or 8-byte encoding.

use arrow::array::{
    ArrayRef, FixedSizeBinaryBuilder, Float64Builder, Int64Builder, StringBuilder, UInt8Builder,
    UInt64Builder,
};
use arrow::record_batch::RecordBatch;
use barter_data_server::parquet::encoder::{encode_fixed_point, encode_fixed_point_i64};
use barter_data_server::parquet::{
    BarMetadata, ExtendedBarMetadata, PrecisionMode, TradeMetadata, bar_schema,
    extended_bar_schema, trade_schema,
};
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use std::fs::File;
use std::sync::Arc;

fn generate_bars(
    path: &str,
    precision_mode: PrecisionMode,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Generating bars to {}...", path);

    // Nautilus bar_type format: {instrument_id}-{step}-{aggregation}-{price_type}-EXTERNAL
    let instrument_id = "BTCUSDT-PERP.BINANCE";
    let bar_type = format!("{}-1-MINUTE-LAST-EXTERNAL", instrument_id);
    let meta = BarMetadata {
        bar_type,
        instrument_id: instrument_id.to_string(),
        price_precision: 2,
        size_precision: 3,
    };
    let schema = bar_schema(&meta, precision_mode);

    // Generate 10 sample 1-minute bars
    let num_bars = 10;
    let base_price = 100_000.0;
    let base_time_ns: u64 = 1_706_540_400_000_000_000; // 2024-01-29 15:00:00 UTC in nanos

    let bytes_len = precision_mode.bytes_len();
    let mut opens = FixedSizeBinaryBuilder::with_capacity(num_bars, bytes_len);
    let mut highs = FixedSizeBinaryBuilder::with_capacity(num_bars, bytes_len);
    let mut lows = FixedSizeBinaryBuilder::with_capacity(num_bars, bytes_len);
    let mut closes = FixedSizeBinaryBuilder::with_capacity(num_bars, bytes_len);
    let mut volumes = FixedSizeBinaryBuilder::with_capacity(num_bars, bytes_len);
    let mut ts_events = UInt64Builder::with_capacity(num_bars);
    let mut ts_inits = UInt64Builder::with_capacity(num_bars);

    for i in 0..num_bars {
        let offset = (i as f64) * 10.0;
        let open = base_price + offset;
        let high = open + 50.0;
        let low = open - 30.0;
        let close = open + 20.0;
        let volume = 100.0 + (i as f64) * 5.0;

        // ts_event = bar CLOSE time (end of minute)
        let ts_event_ns = base_time_ns + ((i as u64 + 1) * 60_000_000_000);
        // ts_init = ingest time (slightly after close)
        let ts_init_ns = ts_event_ns + 100_000; // 100 microseconds later

        let open_bytes = encode_fixed_point(open, precision_mode);
        let high_bytes = encode_fixed_point(high, precision_mode);
        let low_bytes = encode_fixed_point(low, precision_mode);
        let close_bytes = encode_fixed_point(close, precision_mode);
        let volume_bytes = encode_fixed_point(volume, precision_mode);

        opens.append_value(open_bytes.as_slice())?;
        highs.append_value(high_bytes.as_slice())?;
        lows.append_value(low_bytes.as_slice())?;
        closes.append_value(close_bytes.as_slice())?;
        volumes.append_value(volume_bytes.as_slice())?;
        ts_events.append_value(ts_event_ns);
        ts_inits.append_value(ts_init_ns);
    }

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

    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();

    let file = File::create(path)?;
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
    writer.write(&batch)?;
    writer.close()?;

    println!("  Wrote {} bars with bar_type={}", num_bars, meta.bar_type);
    println!("  Schema metadata: {:?}", batch.schema().metadata());
    Ok(())
}

fn generate_trades(
    path: &str,
    precision_mode: PrecisionMode,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Generating trades to {}...", path);

    let instrument_id = "BTCUSDT-PERP.BINANCE";
    let meta = TradeMetadata {
        instrument_id: instrument_id.to_string(),
        price_precision: 2,
        size_precision: 3,
    };
    let schema = trade_schema(&meta, precision_mode);

    // Generate 100 sample trades
    let num_trades = 100;
    let base_price = 100_000.0;
    let base_time_ns: u64 = 1_706_540_400_000_000_000; // 2024-01-29 15:00:00 UTC in nanos

    let bytes_len = precision_mode.bytes_len();
    let mut prices = FixedSizeBinaryBuilder::with_capacity(num_trades, bytes_len);
    let mut sizes = FixedSizeBinaryBuilder::with_capacity(num_trades, bytes_len);
    let mut sides = UInt8Builder::with_capacity(num_trades);
    let mut trade_ids = StringBuilder::with_capacity(num_trades, num_trades * 20);
    let mut ts_events = UInt64Builder::with_capacity(num_trades);
    let mut ts_inits = UInt64Builder::with_capacity(num_trades);

    for i in 0..num_trades {
        let price_offset = ((i as f64) * 0.1).sin() * 50.0;
        let price = base_price + price_offset;
        let size = 0.001 + (i as f64) * 0.0001;

        // Alternate buy/sell: 1=BUYER, 2=SELLER
        let side: u8 = if i % 2 == 0 { 1 } else { 2 };

        let trade_id = format!("{}", 1000000 + i);

        // ts_event = exchange timestamp
        let ts_event_ns = base_time_ns + (i as u64 * 500_000_000); // 500ms apart
        // ts_init = ingest time
        let ts_init_ns = ts_event_ns + 50_000; // 50 microseconds later

        let price_bytes = encode_fixed_point(price, precision_mode);
        let size_bytes = encode_fixed_point(size, precision_mode);
        prices.append_value(price_bytes.as_slice())?;
        sizes.append_value(size_bytes.as_slice())?;
        sides.append_value(side);
        trade_ids.append_value(&trade_id);
        ts_events.append_value(ts_event_ns);
        ts_inits.append_value(ts_init_ns);
    }

    let arrays: Vec<ArrayRef> = vec![
        Arc::new(prices.finish()),
        Arc::new(sizes.finish()),
        Arc::new(sides.finish()),
        Arc::new(trade_ids.finish()),
        Arc::new(ts_events.finish()),
        Arc::new(ts_inits.finish()),
    ];

    let batch = RecordBatch::try_new(schema.clone(), arrays)?;

    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();

    let file = File::create(path)?;
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
    writer.write(&batch)?;
    writer.close()?;

    println!(
        "  Wrote {} trades with instrument_id={}",
        num_trades, instrument_id
    );
    println!("  Schema metadata: {:?}", batch.schema().metadata());
    Ok(())
}

fn generate_extended_bars(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("Generating extended bars to {}...", path);

    let instrument_id = "BTCUSDT-PERP.BINANCE";
    let meta = ExtendedBarMetadata::new(instrument_id, 2, 3);
    let schema = extended_bar_schema(&meta);

    let num_bars = 10;
    let base_time_ns: u64 = 1_706_540_400_000_000_000; // 2024-01-29 15:00:00 UTC in nanos

    let mut ts_events = UInt64Builder::with_capacity(num_bars);
    let mut ts_inits = UInt64Builder::with_capacity(num_bars);
    let mut ts_opens = UInt64Builder::with_capacity(num_bars);
    let mut instrument_ids = StringBuilder::with_capacity(num_bars, num_bars * 30);

    let mut opens = Int64Builder::with_capacity(num_bars);
    let mut highs = Int64Builder::with_capacity(num_bars);
    let mut lows = Int64Builder::with_capacity(num_bars);
    let mut closes = Int64Builder::with_capacity(num_bars);
    let mut volumes = Int64Builder::with_capacity(num_bars);
    let mut quote_volumes = Int64Builder::with_capacity(num_bars);
    let mut trade_counts = UInt64Builder::with_capacity(num_bars);

    let mut buy_volumes = Int64Builder::with_capacity(num_bars);
    let mut sell_volumes = Int64Builder::with_capacity(num_bars);
    let mut deltas = Int64Builder::with_capacity(num_bars);
    let mut cvds = Int64Builder::with_capacity(num_bars);

    let mut open_interests = Int64Builder::with_capacity(num_bars);
    let mut oi_changes = Int64Builder::with_capacity(num_bars);
    let mut funding_rates = Float64Builder::with_capacity(num_bars);

    let mut bid_prices = Int64Builder::with_capacity(num_bars);
    let mut bid_sizes = Int64Builder::with_capacity(num_bars);
    let mut ask_prices = Int64Builder::with_capacity(num_bars);
    let mut ask_sizes = Int64Builder::with_capacity(num_bars);
    let mut spread_bps = Float64Builder::with_capacity(num_bars);
    let mut book_imbalances = Float64Builder::with_capacity(num_bars);

    let mut liq_buy_usd = Int64Builder::with_capacity(num_bars);
    let mut liq_sell_usd = Int64Builder::with_capacity(num_bars);
    let mut liq_total_usd = Int64Builder::with_capacity(num_bars);
    let mut liq_count = UInt64Builder::with_capacity(num_bars);

    let mut bid_depth_10bps_base = Int64Builder::with_capacity(num_bars);
    let mut ask_depth_10bps_base = Int64Builder::with_capacity(num_bars);
    let mut bid_depth_10bps_usd = Int64Builder::with_capacity(num_bars);
    let mut ask_depth_10bps_usd = Int64Builder::with_capacity(num_bars);
    let mut depth_imb_10bps = Float64Builder::with_capacity(num_bars);

    let mut bid_depth_50bps_base = Int64Builder::with_capacity(num_bars);
    let mut ask_depth_50bps_base = Int64Builder::with_capacity(num_bars);
    let mut bid_depth_50bps_usd = Int64Builder::with_capacity(num_bars);
    let mut ask_depth_50bps_usd = Int64Builder::with_capacity(num_bars);
    let mut depth_imb_50bps = Float64Builder::with_capacity(num_bars);

    let mut bid_depth_100bps_base = Int64Builder::with_capacity(num_bars);
    let mut ask_depth_100bps_base = Int64Builder::with_capacity(num_bars);
    let mut bid_depth_100bps_usd = Int64Builder::with_capacity(num_bars);
    let mut ask_depth_100bps_usd = Int64Builder::with_capacity(num_bars);
    let mut depth_imb_100bps = Float64Builder::with_capacity(num_bars);

    for i in 0..num_bars {
        let open = 100_000.0 + (i as f64) * 10.0;
        let high = open + 50.0;
        let low = open - 30.0;
        let close = open + 20.0;
        let volume = 100.0 + (i as f64) * 5.0;
        let quote_volume = volume * close;

        let ts_open_ns = base_time_ns + (i as u64 * 60_000_000_000);
        let ts_event_ns = ts_open_ns + 60_000_000_000;
        let ts_init_ns = ts_event_ns + 100_000;

        let buy_vol = volume * 0.6;
        let sell_vol = volume * 0.4;
        let delta = buy_vol - sell_vol;
        let cvd = (i as f64 + 1.0) * delta;

        let oi = 1_000_000.0 + (i as f64) * 10_000.0;
        let oi_change = 10_000.0;
        let funding = 0.0001;

        let bid_price = close - 5.0;
        let ask_price = close + 5.0;
        let bid_size = 2.0 + (i as f64) * 0.1;
        let ask_size = 1.8 + (i as f64) * 0.1;
        let spread = ((ask_price - bid_price) / close) * 10_000.0;
        let imbalance = (bid_size - ask_size) / (bid_size + ask_size);

        let liq_buy = 10_000.0 + (i as f64) * 500.0;
        let liq_sell = 5_000.0 + (i as f64) * 250.0;
        let liq_total = liq_buy + liq_sell;
        let liq_cnt = 3 + i as u64;

        let depth_10_base = 5.0 + (i as f64);
        let depth_10_usd = depth_10_base * close;
        let depth_50_base = 15.0 + (i as f64);
        let depth_50_usd = depth_50_base * close;
        let depth_100_base = 30.0 + (i as f64);
        let depth_100_usd = depth_100_base * close;

        ts_events.append_value(ts_event_ns);
        ts_inits.append_value(ts_init_ns);
        ts_opens.append_value(ts_open_ns);
        instrument_ids.append_value(instrument_id);

        opens.append_value(encode_fixed_point_i64(open));
        highs.append_value(encode_fixed_point_i64(high));
        lows.append_value(encode_fixed_point_i64(low));
        closes.append_value(encode_fixed_point_i64(close));
        volumes.append_value(encode_fixed_point_i64(volume));
        quote_volumes.append_value(encode_fixed_point_i64(quote_volume));
        trade_counts.append_value(50 + i as u64);

        buy_volumes.append_value(encode_fixed_point_i64(buy_vol));
        sell_volumes.append_value(encode_fixed_point_i64(sell_vol));
        deltas.append_value(encode_fixed_point_i64(delta));
        cvds.append_value(encode_fixed_point_i64(cvd));

        open_interests.append_value(encode_fixed_point_i64(oi));
        oi_changes.append_value(encode_fixed_point_i64(oi_change));
        funding_rates.append_value(funding);

        bid_prices.append_value(encode_fixed_point_i64(bid_price));
        bid_sizes.append_value(encode_fixed_point_i64(bid_size));
        ask_prices.append_value(encode_fixed_point_i64(ask_price));
        ask_sizes.append_value(encode_fixed_point_i64(ask_size));
        spread_bps.append_value(spread);
        book_imbalances.append_value(imbalance);

        liq_buy_usd.append_value(encode_fixed_point_i64(liq_buy));
        liq_sell_usd.append_value(encode_fixed_point_i64(liq_sell));
        liq_total_usd.append_value(encode_fixed_point_i64(liq_total));
        liq_count.append_value(liq_cnt);

        bid_depth_10bps_base.append_value(encode_fixed_point_i64(depth_10_base));
        ask_depth_10bps_base.append_value(encode_fixed_point_i64(depth_10_base * 0.9));
        bid_depth_10bps_usd.append_value(encode_fixed_point_i64(depth_10_usd));
        ask_depth_10bps_usd.append_value(encode_fixed_point_i64(depth_10_usd * 0.95));
        depth_imb_10bps.append_value(0.05);

        bid_depth_50bps_base.append_value(encode_fixed_point_i64(depth_50_base));
        ask_depth_50bps_base.append_value(encode_fixed_point_i64(depth_50_base * 0.95));
        bid_depth_50bps_usd.append_value(encode_fixed_point_i64(depth_50_usd));
        ask_depth_50bps_usd.append_value(encode_fixed_point_i64(depth_50_usd * 0.97));
        depth_imb_50bps.append_value(0.03);

        bid_depth_100bps_base.append_value(encode_fixed_point_i64(depth_100_base));
        ask_depth_100bps_base.append_value(encode_fixed_point_i64(depth_100_base * 0.97));
        bid_depth_100bps_usd.append_value(encode_fixed_point_i64(depth_100_usd));
        ask_depth_100bps_usd.append_value(encode_fixed_point_i64(depth_100_usd * 0.98));
        depth_imb_100bps.append_value(0.02);
    }

    let arrays: Vec<ArrayRef> = vec![
        Arc::new(ts_events.finish()),
        Arc::new(ts_inits.finish()),
        Arc::new(ts_opens.finish()),
        Arc::new(instrument_ids.finish()),
        Arc::new(opens.finish()),
        Arc::new(highs.finish()),
        Arc::new(lows.finish()),
        Arc::new(closes.finish()),
        Arc::new(volumes.finish()),
        Arc::new(quote_volumes.finish()),
        Arc::new(trade_counts.finish()),
        Arc::new(buy_volumes.finish()),
        Arc::new(sell_volumes.finish()),
        Arc::new(deltas.finish()),
        Arc::new(cvds.finish()),
        Arc::new(open_interests.finish()),
        Arc::new(oi_changes.finish()),
        Arc::new(funding_rates.finish()),
        Arc::new(bid_prices.finish()),
        Arc::new(bid_sizes.finish()),
        Arc::new(ask_prices.finish()),
        Arc::new(ask_sizes.finish()),
        Arc::new(spread_bps.finish()),
        Arc::new(book_imbalances.finish()),
        Arc::new(liq_buy_usd.finish()),
        Arc::new(liq_sell_usd.finish()),
        Arc::new(liq_total_usd.finish()),
        Arc::new(liq_count.finish()),
        Arc::new(bid_depth_10bps_base.finish()),
        Arc::new(ask_depth_10bps_base.finish()),
        Arc::new(bid_depth_10bps_usd.finish()),
        Arc::new(ask_depth_10bps_usd.finish()),
        Arc::new(depth_imb_10bps.finish()),
        Arc::new(bid_depth_50bps_base.finish()),
        Arc::new(ask_depth_50bps_base.finish()),
        Arc::new(bid_depth_50bps_usd.finish()),
        Arc::new(ask_depth_50bps_usd.finish()),
        Arc::new(depth_imb_50bps.finish()),
        Arc::new(bid_depth_100bps_base.finish()),
        Arc::new(ask_depth_100bps_base.finish()),
        Arc::new(bid_depth_100bps_usd.finish()),
        Arc::new(ask_depth_100bps_usd.finish()),
        Arc::new(depth_imb_100bps.finish()),
    ];

    let batch = RecordBatch::try_new(schema.clone(), arrays)?;

    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();

    let file = File::create(path)?;
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
    writer.write(&batch)?;
    writer.close()?;

    println!(
        "  Wrote {} extended bars with instrument_id={}",
        num_bars, instrument_id
    );
    println!("  Schema metadata: {:?}", batch.schema().metadata());
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Nautilus Sample Parquet Generator ===\n");

    let precision_mode = PrecisionMode::from_env();
    generate_bars("/tmp/test_bars.parquet", precision_mode)?;
    println!();
    generate_trades("/tmp/test_trades.parquet", precision_mode)?;
    println!();
    generate_extended_bars("/tmp/test_extended_bars.parquet")?;

    println!("\n=== Generation Complete ===");
    println!("Files created:");
    println!("  /tmp/test_bars.parquet   - 10 sample 1-minute bars");
    println!("  /tmp/test_trades.parquet - 100 sample trades");
    println!("  /tmp/test_extended_bars.parquet - 10 sample extended bars");
    println!("\nValidate with: python scripts/validation/test_nautilus_load.py");

    Ok(())
}
