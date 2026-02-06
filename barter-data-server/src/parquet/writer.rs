//! Buffered Parquet writer for market data.
//!
//! Receives events via channel and writes them to Parquet files with periodic flush.

use crate::parquet::encoder::{PrecisionMode, encode_fixed_point, encode_fixed_point_i64};
use crate::parquet::schema::{
    AggressorSide, BarMetadata, ExtendedBarMetadata, OrderBookDeltaMetadata, TradeMetadata,
    bar_schema, extended_bar_schema, order_book_delta_schema, trade_schema,
};
use arrow::array::{
    ArrayRef, FixedSizeBinaryBuilder, Float64Builder, Int64Builder, StringBuilder, UInt8Builder,
    UInt64Builder,
};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::time::{Duration, interval};
use tracing::{debug, error, info, warn};

fn parse_bool_env(var: &str, default: bool) -> bool {
    std::env::var(var)
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes"))
        .unwrap_or(default)
}

fn parse_usize_env(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

/// Errors that can occur during Parquet writing.
#[derive(Error, Debug)]
pub enum ParquetError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),
    #[error("Parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),
    #[error("Invalid data: {0}")]
    #[allow(dead_code)]
    InvalidData(String),
}

/// Configuration for Parquet writer (from environment variables).
#[derive(Debug, Clone)]
pub struct ParquetConfig {
    /// Output directory for Parquet files
    pub output_dir: PathBuf,
    /// Flush interval in seconds
    pub flush_interval_secs: u64,
    /// Buffer size for event channel
    #[allow(dead_code)]
    pub buffer_size: usize,
    /// Enable S3 upload after local write
    pub s3_enabled: bool,
    /// S3 bucket name
    pub s3_bucket: Option<String>,
    /// S3 region
    pub s3_region: Option<String>,
    /// Nautilus precision mode (standard or high)
    pub precision_mode: PrecisionMode,
    /// Fsync after each file write (durability vs throughput)
    pub fsync: bool,
    /// Retry count for process_event failures (transient errors)
    pub event_retry_max: usize,
}

impl ParquetConfig {
    /// Load configuration from environment variables.
    pub fn from_env() -> Self {
        let output_dir = std::env::var("PARQUET_OUTPUT_DIR")
            .unwrap_or_else(|_| "/data/parquet".to_string())
            .into();
        let flush_interval_secs = std::env::var("PARQUET_FLUSH_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(60);
        let buffer_size = std::env::var("PARQUET_BUFFER")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(50_000);
        let s3_enabled = std::env::var("S3_ENABLED")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false);
        let s3_bucket = std::env::var("S3_BUCKET").ok();
        let s3_region = std::env::var("S3_REGION").ok();
        let precision_mode = PrecisionMode::from_env();
        let fsync = parse_bool_env("PARQUET_FSYNC", false);
        let event_retry_max = parse_usize_env("PARQUET_EVENT_RETRY_MAX", 3);

        Self {
            output_dir,
            flush_interval_secs,
            buffer_size,
            s3_enabled,
            s3_bucket,
            s3_region,
            precision_mode,
            fsync,
            event_retry_max,
        }
    }
}

/// A trade event for Parquet writing.
#[derive(Debug, Clone)]
pub struct TradeEvent {
    pub instrument_id: String,
    pub price: f64,
    pub size: f64,
    pub side: Option<barter_instrument::Side>,
    pub trade_id: String,
    pub ts_event_ns: u64,
    pub ts_init_ns: u64,
    pub price_precision: u8,
    pub size_precision: u8,
}

/// A bar event for Parquet writing.
#[derive(Debug, Clone)]
pub struct BarEvent {
    pub bar_type: String,
    pub instrument_id: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub ts_event_ns: u64,
    pub ts_init_ns: u64,
    pub price_precision: u8,
    pub size_precision: u8,
}

/// An extended bar event for Parquet writing (Barter-only, includes delta/CVD/OI).
#[derive(Debug, Clone)]
pub struct ExtendedBarEvent {
    pub instrument_id: String,
    pub ts_event_ns: u64,
    pub ts_init_ns: u64,
    pub ts_open_ns: u64,
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
    pub bid_price: f64,
    pub bid_size: f64,
    pub ask_price: f64,
    pub ask_size: f64,
    pub spread_bps: f64,
    pub book_imbalance: f64,
    pub liq_buy_usd: f64,
    pub liq_sell_usd: f64,
    pub liq_total_usd: f64,
    pub liq_count: u64,
    pub bid_depth_10bps_base: f64,
    pub ask_depth_10bps_base: f64,
    pub bid_depth_10bps_usd: f64,
    pub ask_depth_10bps_usd: f64,
    pub depth_imb_10bps: f64,
    pub bid_depth_50bps_base: f64,
    pub ask_depth_50bps_base: f64,
    pub bid_depth_50bps_usd: f64,
    pub ask_depth_50bps_usd: f64,
    pub depth_imb_50bps: f64,
    pub bid_depth_100bps_base: f64,
    pub ask_depth_100bps_base: f64,
    pub bid_depth_100bps_usd: f64,
    pub ask_depth_100bps_usd: f64,
    pub depth_imb_100bps: f64,
    pub price_precision: u8,
    pub size_precision: u8,
}

/// An order book delta event for Parquet writing (Nautilus-compatible).
#[derive(Debug, Clone)]
pub struct OrderBookDeltaEvent {
    pub instrument_id: String,
    pub action: u8,
    pub side: u8,
    pub price: f64,
    pub size: f64,
    pub order_id: u64,
    pub flags: u8,
    pub sequence: u64,
    pub ts_event_ns: u64,
    pub ts_init_ns: u64,
    pub price_precision: u8,
    pub size_precision: u8,
}

/// Event types that can be written to Parquet.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum ParquetEvent {
    Trade(TradeEvent),
    Bar(BarEvent),
    ExtendedBar(ExtendedBarEvent),
    OrderBookDelta(OrderBookDeltaEvent),
}

/// Buffered trade data for batch writing.
struct TradeBuffer {
    prices: FixedSizeBinaryBuilder,
    sizes: FixedSizeBinaryBuilder,
    sides: UInt8Builder,
    trade_ids: StringBuilder,
    ts_events: UInt64Builder,
    ts_inits: UInt64Builder,
    metadata: TradeMetadata,
    count: usize,
    precision_mode: PrecisionMode,
}

impl TradeBuffer {
    fn new(metadata: TradeMetadata, capacity: usize, precision_mode: PrecisionMode) -> Self {
        let bytes_len = precision_mode.bytes_len();
        Self {
            prices: FixedSizeBinaryBuilder::with_capacity(capacity, bytes_len),
            sizes: FixedSizeBinaryBuilder::with_capacity(capacity, bytes_len),
            sides: UInt8Builder::with_capacity(capacity),
            trade_ids: StringBuilder::with_capacity(capacity, capacity * 20),
            ts_events: UInt64Builder::with_capacity(capacity),
            ts_inits: UInt64Builder::with_capacity(capacity),
            metadata,
            count: 0,
            precision_mode,
        }
    }

    fn append(&mut self, trade: &TradeEvent) -> Result<(), ParquetError> {
        let price_bytes = encode_fixed_point(trade.price, self.precision_mode);
        let size_bytes = encode_fixed_point(trade.size, self.precision_mode);
        self.prices.append_value(price_bytes.as_slice())?;
        self.sizes.append_value(size_bytes.as_slice())?;
        self.sides
            .append_value(AggressorSide::from_side(trade.side) as u8);
        self.trade_ids.append_value(&trade.trade_id);
        self.ts_events.append_value(trade.ts_event_ns);
        self.ts_inits.append_value(trade.ts_init_ns);
        self.count += 1;
        Ok(())
    }

    #[allow(clippy::wrong_self_convention)]
    fn to_record_batch(&mut self) -> Result<RecordBatch, ParquetError> {
        let schema = trade_schema(&self.metadata, self.precision_mode);
        let arrays: Vec<ArrayRef> = vec![
            Arc::new(self.prices.finish()),
            Arc::new(self.sizes.finish()),
            Arc::new(self.sides.finish()),
            Arc::new(self.trade_ids.finish()),
            Arc::new(self.ts_events.finish()),
            Arc::new(self.ts_inits.finish()),
        ];
        self.count = 0;
        Ok(RecordBatch::try_new(schema, arrays)?)
    }

    fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn len(&self) -> usize {
        self.count
    }
}

/// Buffered order book delta data for batch writing.
struct OrderBookDeltaBuffer {
    actions: UInt8Builder,
    sides: UInt8Builder,
    prices: FixedSizeBinaryBuilder,
    sizes: FixedSizeBinaryBuilder,
    order_ids: UInt64Builder,
    flags: UInt8Builder,
    sequences: UInt64Builder,
    ts_events: UInt64Builder,
    ts_inits: UInt64Builder,
    metadata: OrderBookDeltaMetadata,
    count: usize,
    precision_mode: PrecisionMode,
}

impl OrderBookDeltaBuffer {
    fn new(
        metadata: OrderBookDeltaMetadata,
        capacity: usize,
        precision_mode: PrecisionMode,
    ) -> Self {
        let bytes_len = precision_mode.bytes_len();
        Self {
            actions: UInt8Builder::with_capacity(capacity),
            sides: UInt8Builder::with_capacity(capacity),
            prices: FixedSizeBinaryBuilder::with_capacity(capacity, bytes_len),
            sizes: FixedSizeBinaryBuilder::with_capacity(capacity, bytes_len),
            order_ids: UInt64Builder::with_capacity(capacity),
            flags: UInt8Builder::with_capacity(capacity),
            sequences: UInt64Builder::with_capacity(capacity),
            ts_events: UInt64Builder::with_capacity(capacity),
            ts_inits: UInt64Builder::with_capacity(capacity),
            metadata,
            count: 0,
            precision_mode,
        }
    }

    fn append(&mut self, delta: &OrderBookDeltaEvent) -> Result<(), ParquetError> {
        let price_bytes = encode_fixed_point(delta.price, self.precision_mode);
        let size_bytes = encode_fixed_point(delta.size, self.precision_mode);
        self.actions.append_value(delta.action);
        self.sides.append_value(delta.side);
        self.prices.append_value(price_bytes.as_slice())?;
        self.sizes.append_value(size_bytes.as_slice())?;
        self.order_ids.append_value(delta.order_id);
        self.flags.append_value(delta.flags);
        self.sequences.append_value(delta.sequence);
        self.ts_events.append_value(delta.ts_event_ns);
        self.ts_inits.append_value(delta.ts_init_ns);
        self.count += 1;
        Ok(())
    }

    #[allow(clippy::wrong_self_convention)]
    fn to_record_batch(&mut self) -> Result<RecordBatch, ParquetError> {
        let schema = order_book_delta_schema(&self.metadata, self.precision_mode);
        let arrays: Vec<ArrayRef> = vec![
            Arc::new(self.actions.finish()),
            Arc::new(self.sides.finish()),
            Arc::new(self.prices.finish()),
            Arc::new(self.sizes.finish()),
            Arc::new(self.order_ids.finish()),
            Arc::new(self.flags.finish()),
            Arc::new(self.sequences.finish()),
            Arc::new(self.ts_events.finish()),
            Arc::new(self.ts_inits.finish()),
        ];
        self.count = 0;
        Ok(RecordBatch::try_new(schema, arrays)?)
    }

    fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn len(&self) -> usize {
        self.count
    }
}

/// Buffered bar data for batch writing.
struct BarBuffer {
    opens: FixedSizeBinaryBuilder,
    highs: FixedSizeBinaryBuilder,
    lows: FixedSizeBinaryBuilder,
    closes: FixedSizeBinaryBuilder,
    volumes: FixedSizeBinaryBuilder,
    ts_events: UInt64Builder,
    ts_inits: UInt64Builder,
    metadata: BarMetadata,
    count: usize,
    precision_mode: PrecisionMode,
}

impl BarBuffer {
    fn new(metadata: BarMetadata, capacity: usize, precision_mode: PrecisionMode) -> Self {
        let bytes_len = precision_mode.bytes_len();
        Self {
            opens: FixedSizeBinaryBuilder::with_capacity(capacity, bytes_len),
            highs: FixedSizeBinaryBuilder::with_capacity(capacity, bytes_len),
            lows: FixedSizeBinaryBuilder::with_capacity(capacity, bytes_len),
            closes: FixedSizeBinaryBuilder::with_capacity(capacity, bytes_len),
            volumes: FixedSizeBinaryBuilder::with_capacity(capacity, bytes_len),
            ts_events: UInt64Builder::with_capacity(capacity),
            ts_inits: UInt64Builder::with_capacity(capacity),
            metadata,
            count: 0,
            precision_mode,
        }
    }

    fn append(&mut self, bar: &BarEvent) -> Result<(), ParquetError> {
        let open_bytes = encode_fixed_point(bar.open, self.precision_mode);
        let high_bytes = encode_fixed_point(bar.high, self.precision_mode);
        let low_bytes = encode_fixed_point(bar.low, self.precision_mode);
        let close_bytes = encode_fixed_point(bar.close, self.precision_mode);
        let vol_bytes = encode_fixed_point(bar.volume, self.precision_mode);
        self.opens.append_value(open_bytes.as_slice())?;
        self.highs.append_value(high_bytes.as_slice())?;
        self.lows.append_value(low_bytes.as_slice())?;
        self.closes.append_value(close_bytes.as_slice())?;
        self.volumes.append_value(vol_bytes.as_slice())?;
        self.ts_events.append_value(bar.ts_event_ns);
        self.ts_inits.append_value(bar.ts_init_ns);
        self.count += 1;
        Ok(())
    }

    #[allow(clippy::wrong_self_convention)]
    fn to_record_batch(&mut self) -> Result<RecordBatch, ParquetError> {
        let schema = bar_schema(&self.metadata, self.precision_mode);
        let arrays: Vec<ArrayRef> = vec![
            Arc::new(self.opens.finish()),
            Arc::new(self.highs.finish()),
            Arc::new(self.lows.finish()),
            Arc::new(self.closes.finish()),
            Arc::new(self.volumes.finish()),
            Arc::new(self.ts_events.finish()),
            Arc::new(self.ts_inits.finish()),
        ];
        self.count = 0;
        Ok(RecordBatch::try_new(schema, arrays)?)
    }

    fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn len(&self) -> usize {
        self.count
    }
}

/// Buffered extended bar data for batch writing (Barter-only).
struct ExtendedBarBuffer {
    ts_events: UInt64Builder,
    ts_inits: UInt64Builder,
    ts_opens: UInt64Builder,
    instrument_ids: StringBuilder,
    opens: Int64Builder,
    highs: Int64Builder,
    lows: Int64Builder,
    closes: Int64Builder,
    volumes: Int64Builder,
    quote_volumes: Int64Builder,
    trade_counts: UInt64Builder,
    buy_volumes: Int64Builder,
    sell_volumes: Int64Builder,
    deltas: Int64Builder,
    cvds: Int64Builder,
    open_interests: Int64Builder,
    oi_changes: Int64Builder,
    funding_rates: Float64Builder,
    bid_prices: Int64Builder,
    bid_sizes: Int64Builder,
    ask_prices: Int64Builder,
    ask_sizes: Int64Builder,
    spread_bps: Float64Builder,
    book_imbalances: Float64Builder,
    liq_buy_usd: Int64Builder,
    liq_sell_usd: Int64Builder,
    liq_total_usd: Int64Builder,
    liq_count: UInt64Builder,
    bid_depth_10bps_base: Int64Builder,
    ask_depth_10bps_base: Int64Builder,
    bid_depth_10bps_usd: Int64Builder,
    ask_depth_10bps_usd: Int64Builder,
    depth_imb_10bps: Float64Builder,
    bid_depth_50bps_base: Int64Builder,
    ask_depth_50bps_base: Int64Builder,
    bid_depth_50bps_usd: Int64Builder,
    ask_depth_50bps_usd: Int64Builder,
    depth_imb_50bps: Float64Builder,
    bid_depth_100bps_base: Int64Builder,
    ask_depth_100bps_base: Int64Builder,
    bid_depth_100bps_usd: Int64Builder,
    ask_depth_100bps_usd: Int64Builder,
    depth_imb_100bps: Float64Builder,
    metadata: ExtendedBarMetadata,
    count: usize,
}

impl ExtendedBarBuffer {
    fn new(metadata: ExtendedBarMetadata, capacity: usize) -> Self {
        Self {
            ts_events: UInt64Builder::with_capacity(capacity),
            ts_inits: UInt64Builder::with_capacity(capacity),
            ts_opens: UInt64Builder::with_capacity(capacity),
            instrument_ids: StringBuilder::with_capacity(capacity, capacity * 30),
            opens: Int64Builder::with_capacity(capacity),
            highs: Int64Builder::with_capacity(capacity),
            lows: Int64Builder::with_capacity(capacity),
            closes: Int64Builder::with_capacity(capacity),
            volumes: Int64Builder::with_capacity(capacity),
            quote_volumes: Int64Builder::with_capacity(capacity),
            trade_counts: UInt64Builder::with_capacity(capacity),
            buy_volumes: Int64Builder::with_capacity(capacity),
            sell_volumes: Int64Builder::with_capacity(capacity),
            deltas: Int64Builder::with_capacity(capacity),
            cvds: Int64Builder::with_capacity(capacity),
            open_interests: Int64Builder::with_capacity(capacity),
            oi_changes: Int64Builder::with_capacity(capacity),
            funding_rates: Float64Builder::with_capacity(capacity),
            bid_prices: Int64Builder::with_capacity(capacity),
            bid_sizes: Int64Builder::with_capacity(capacity),
            ask_prices: Int64Builder::with_capacity(capacity),
            ask_sizes: Int64Builder::with_capacity(capacity),
            spread_bps: Float64Builder::with_capacity(capacity),
            book_imbalances: Float64Builder::with_capacity(capacity),
            liq_buy_usd: Int64Builder::with_capacity(capacity),
            liq_sell_usd: Int64Builder::with_capacity(capacity),
            liq_total_usd: Int64Builder::with_capacity(capacity),
            liq_count: UInt64Builder::with_capacity(capacity),
            bid_depth_10bps_base: Int64Builder::with_capacity(capacity),
            ask_depth_10bps_base: Int64Builder::with_capacity(capacity),
            bid_depth_10bps_usd: Int64Builder::with_capacity(capacity),
            ask_depth_10bps_usd: Int64Builder::with_capacity(capacity),
            depth_imb_10bps: Float64Builder::with_capacity(capacity),
            bid_depth_50bps_base: Int64Builder::with_capacity(capacity),
            ask_depth_50bps_base: Int64Builder::with_capacity(capacity),
            bid_depth_50bps_usd: Int64Builder::with_capacity(capacity),
            ask_depth_50bps_usd: Int64Builder::with_capacity(capacity),
            depth_imb_50bps: Float64Builder::with_capacity(capacity),
            bid_depth_100bps_base: Int64Builder::with_capacity(capacity),
            ask_depth_100bps_base: Int64Builder::with_capacity(capacity),
            bid_depth_100bps_usd: Int64Builder::with_capacity(capacity),
            ask_depth_100bps_usd: Int64Builder::with_capacity(capacity),
            depth_imb_100bps: Float64Builder::with_capacity(capacity),
            metadata,
            count: 0,
        }
    }

    fn append(&mut self, bar: &ExtendedBarEvent) -> Result<(), ParquetError> {
        self.ts_events.append_value(bar.ts_event_ns);
        self.ts_inits.append_value(bar.ts_init_ns);
        self.ts_opens.append_value(bar.ts_open_ns);
        self.instrument_ids.append_value(&bar.instrument_id);
        self.opens.append_value(encode_fixed_point_i64(bar.open));
        self.highs.append_value(encode_fixed_point_i64(bar.high));
        self.lows.append_value(encode_fixed_point_i64(bar.low));
        self.closes.append_value(encode_fixed_point_i64(bar.close));
        self.volumes
            .append_value(encode_fixed_point_i64(bar.volume));
        self.quote_volumes
            .append_value(encode_fixed_point_i64(bar.quote_volume));
        self.trade_counts.append_value(bar.trade_count);
        self.buy_volumes
            .append_value(encode_fixed_point_i64(bar.buy_volume));
        self.sell_volumes
            .append_value(encode_fixed_point_i64(bar.sell_volume));
        self.deltas.append_value(encode_fixed_point_i64(bar.delta));
        self.cvds.append_value(encode_fixed_point_i64(bar.cvd));
        self.open_interests
            .append_value(encode_fixed_point_i64(bar.open_interest));
        self.oi_changes
            .append_value(encode_fixed_point_i64(bar.oi_change));
        self.funding_rates.append_value(bar.funding_rate);
        self.bid_prices
            .append_value(encode_fixed_point_i64(bar.bid_price));
        self.bid_sizes
            .append_value(encode_fixed_point_i64(bar.bid_size));
        self.ask_prices
            .append_value(encode_fixed_point_i64(bar.ask_price));
        self.ask_sizes
            .append_value(encode_fixed_point_i64(bar.ask_size));
        self.spread_bps.append_value(bar.spread_bps);
        self.book_imbalances.append_value(bar.book_imbalance);
        self.liq_buy_usd
            .append_value(encode_fixed_point_i64(bar.liq_buy_usd));
        self.liq_sell_usd
            .append_value(encode_fixed_point_i64(bar.liq_sell_usd));
        self.liq_total_usd
            .append_value(encode_fixed_point_i64(bar.liq_total_usd));
        self.liq_count.append_value(bar.liq_count);
        self.bid_depth_10bps_base
            .append_value(encode_fixed_point_i64(bar.bid_depth_10bps_base));
        self.ask_depth_10bps_base
            .append_value(encode_fixed_point_i64(bar.ask_depth_10bps_base));
        self.bid_depth_10bps_usd
            .append_value(encode_fixed_point_i64(bar.bid_depth_10bps_usd));
        self.ask_depth_10bps_usd
            .append_value(encode_fixed_point_i64(bar.ask_depth_10bps_usd));
        self.depth_imb_10bps.append_value(bar.depth_imb_10bps);
        self.bid_depth_50bps_base
            .append_value(encode_fixed_point_i64(bar.bid_depth_50bps_base));
        self.ask_depth_50bps_base
            .append_value(encode_fixed_point_i64(bar.ask_depth_50bps_base));
        self.bid_depth_50bps_usd
            .append_value(encode_fixed_point_i64(bar.bid_depth_50bps_usd));
        self.ask_depth_50bps_usd
            .append_value(encode_fixed_point_i64(bar.ask_depth_50bps_usd));
        self.depth_imb_50bps.append_value(bar.depth_imb_50bps);
        self.bid_depth_100bps_base
            .append_value(encode_fixed_point_i64(bar.bid_depth_100bps_base));
        self.ask_depth_100bps_base
            .append_value(encode_fixed_point_i64(bar.ask_depth_100bps_base));
        self.bid_depth_100bps_usd
            .append_value(encode_fixed_point_i64(bar.bid_depth_100bps_usd));
        self.ask_depth_100bps_usd
            .append_value(encode_fixed_point_i64(bar.ask_depth_100bps_usd));
        self.depth_imb_100bps.append_value(bar.depth_imb_100bps);
        self.count += 1;
        Ok(())
    }

    #[allow(clippy::wrong_self_convention)]
    fn to_record_batch(&mut self) -> Result<RecordBatch, ParquetError> {
        let schema = extended_bar_schema(&self.metadata);
        let arrays: Vec<ArrayRef> = vec![
            Arc::new(self.ts_events.finish()),
            Arc::new(self.ts_inits.finish()),
            Arc::new(self.ts_opens.finish()),
            Arc::new(self.instrument_ids.finish()),
            Arc::new(self.opens.finish()),
            Arc::new(self.highs.finish()),
            Arc::new(self.lows.finish()),
            Arc::new(self.closes.finish()),
            Arc::new(self.volumes.finish()),
            Arc::new(self.quote_volumes.finish()),
            Arc::new(self.trade_counts.finish()),
            Arc::new(self.buy_volumes.finish()),
            Arc::new(self.sell_volumes.finish()),
            Arc::new(self.deltas.finish()),
            Arc::new(self.cvds.finish()),
            Arc::new(self.open_interests.finish()),
            Arc::new(self.oi_changes.finish()),
            Arc::new(self.funding_rates.finish()),
            Arc::new(self.bid_prices.finish()),
            Arc::new(self.bid_sizes.finish()),
            Arc::new(self.ask_prices.finish()),
            Arc::new(self.ask_sizes.finish()),
            Arc::new(self.spread_bps.finish()),
            Arc::new(self.book_imbalances.finish()),
            Arc::new(self.liq_buy_usd.finish()),
            Arc::new(self.liq_sell_usd.finish()),
            Arc::new(self.liq_total_usd.finish()),
            Arc::new(self.liq_count.finish()),
            Arc::new(self.bid_depth_10bps_base.finish()),
            Arc::new(self.ask_depth_10bps_base.finish()),
            Arc::new(self.bid_depth_10bps_usd.finish()),
            Arc::new(self.ask_depth_10bps_usd.finish()),
            Arc::new(self.depth_imb_10bps.finish()),
            Arc::new(self.bid_depth_50bps_base.finish()),
            Arc::new(self.ask_depth_50bps_base.finish()),
            Arc::new(self.bid_depth_50bps_usd.finish()),
            Arc::new(self.ask_depth_50bps_usd.finish()),
            Arc::new(self.depth_imb_50bps.finish()),
            Arc::new(self.bid_depth_100bps_base.finish()),
            Arc::new(self.ask_depth_100bps_base.finish()),
            Arc::new(self.bid_depth_100bps_usd.finish()),
            Arc::new(self.ask_depth_100bps_usd.finish()),
            Arc::new(self.depth_imb_100bps.finish()),
        ];
        self.count = 0;
        Ok(RecordBatch::try_new(schema, arrays)?)
    }

    fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn len(&self) -> usize {
        self.count
    }
}

/// Parquet writer that buffers events and writes to files.
pub struct ParquetWriter {
    config: ParquetConfig,
    trade_buffers: HashMap<Arc<str>, TradeBuffer>,
    bar_buffers: HashMap<Arc<str>, BarBuffer>,
    extended_bar_buffers: HashMap<Arc<str>, ExtendedBarBuffer>,
    order_book_delta_buffers: HashMap<Arc<str>, OrderBookDeltaBuffer>,
    trades_written: AtomicU64,
    bars_written: AtomicU64,
    order_book_deltas_written: AtomicU64,
    flush_seq: AtomicU64,
}

impl ParquetWriter {
    /// Create a new Parquet writer.
    pub fn new(config: ParquetConfig) -> Self {
        Self {
            config,
            trade_buffers: HashMap::new(),
            bar_buffers: HashMap::new(),
            extended_bar_buffers: HashMap::new(),
            order_book_delta_buffers: HashMap::new(),
            trades_written: AtomicU64::new(0),
            bars_written: AtomicU64::new(0),
            order_book_deltas_written: AtomicU64::new(0),
            flush_seq: AtomicU64::new(0),
        }
    }

    /// Process a single event.
    pub fn process_event(&mut self, event: ParquetEvent) -> Result<(), ParquetError> {
        match event {
            ParquetEvent::Trade(trade) => {
                if let Some(buffer) = self.trade_buffers.get_mut(trade.instrument_id.as_str()) {
                    buffer.append(&trade)?;
                } else {
                    let key: Arc<str> = Arc::from(trade.instrument_id.as_str());
                    let buffer = TradeBuffer::new(
                        TradeMetadata {
                            instrument_id: key.to_string(),
                            price_precision: trade.price_precision,
                            size_precision: trade.size_precision,
                        },
                        10_000,
                        self.config.precision_mode,
                    );
                    self.trade_buffers.insert(key, buffer);
                    let buffer = self
                        .trade_buffers
                        .get_mut(trade.instrument_id.as_str())
                        .expect("inserted trade buffer missing");
                    buffer.append(&trade)?;
                }
            }
            ParquetEvent::Bar(bar) => {
                if let Some(buffer) = self.bar_buffers.get_mut(bar.bar_type.as_str()) {
                    buffer.append(&bar)?;
                } else {
                    let key: Arc<str> = Arc::from(bar.bar_type.as_str());
                    let buffer = BarBuffer::new(
                        BarMetadata {
                            bar_type: key.to_string(),
                            instrument_id: bar.instrument_id.clone(),
                            price_precision: bar.price_precision,
                            size_precision: bar.size_precision,
                        },
                        1_000,
                        self.config.precision_mode,
                    );
                    self.bar_buffers.insert(key, buffer);
                    let buffer = self
                        .bar_buffers
                        .get_mut(bar.bar_type.as_str())
                        .expect("inserted bar buffer missing");
                    buffer.append(&bar)?;
                }
            }
            ParquetEvent::ExtendedBar(ext_bar) => {
                if let Some(buffer) = self
                    .extended_bar_buffers
                    .get_mut(ext_bar.instrument_id.as_str())
                {
                    buffer.append(&ext_bar)?;
                } else {
                    let key: Arc<str> = Arc::from(ext_bar.instrument_id.as_str());
                    let buffer = ExtendedBarBuffer::new(
                        ExtendedBarMetadata::new(
                            &key,
                            ext_bar.price_precision,
                            ext_bar.size_precision,
                        ),
                        1_000,
                    );
                    self.extended_bar_buffers.insert(key, buffer);
                    let buffer = self
                        .extended_bar_buffers
                        .get_mut(ext_bar.instrument_id.as_str())
                        .expect("inserted extended bar buffer missing");
                    buffer.append(&ext_bar)?;
                }
            }
            ParquetEvent::OrderBookDelta(delta) => {
                if let Some(buffer) = self
                    .order_book_delta_buffers
                    .get_mut(delta.instrument_id.as_str())
                {
                    buffer.append(&delta)?;
                } else {
                    let key: Arc<str> = Arc::from(delta.instrument_id.as_str());
                    let buffer = OrderBookDeltaBuffer::new(
                        OrderBookDeltaMetadata {
                            instrument_id: key.to_string(),
                            price_precision: delta.price_precision,
                            size_precision: delta.size_precision,
                        },
                        10_000,
                        self.config.precision_mode,
                    );
                    self.order_book_delta_buffers.insert(key, buffer);
                    let buffer = self
                        .order_book_delta_buffers
                        .get_mut(delta.instrument_id.as_str())
                        .expect("inserted order book delta buffer missing");
                    buffer.append(&delta)?;
                }
            }
        }
        Ok(())
    }

    /// Flush all buffers to Parquet files.
    /// Returns the list of local file paths written (for S3 upload).
    pub fn flush(&mut self) -> Result<Vec<PathBuf>, ParquetError> {
        let now = chrono::Utc::now();
        let date_str = now.format("%Y-%m-%d").to_string();
        let hour_str = now.format("%H").to_string();
        let flush_seq = self.flush_seq.fetch_add(1, Ordering::Relaxed);
        let mut written_files = Vec::new();
        let mut errors = 0usize;

        // Collect trade batches first to avoid borrow issues
        let trade_batches: Vec<_> = self
            .trade_buffers
            .iter_mut()
            .filter(|(_, buffer)| !buffer.is_empty())
            .filter_map(|(instrument_id, buffer)| {
                let count = buffer.len();
                match buffer.to_record_batch() {
                    Ok(batch) => Some((Arc::clone(instrument_id), count, batch)),
                    Err(e) => {
                        error!("Failed to create trade batch for {}: {}", instrument_id, e);
                        None
                    }
                }
            })
            .collect();

        // Write trade batches
        for (instrument_id, count, batch) in trade_batches {
            let safe_id = sanitize_id(instrument_id.as_ref());
            let dir = self
                .config
                .output_dir
                .join("trades")
                .join(&safe_id)
                .join(&date_str);
            std::fs::create_dir_all(&dir)?;

            let path = dir.join(format!("{}_{}.parquet", hour_str, flush_seq));
            match Self::write_batch_static(&path, batch, self.config.fsync) {
                Ok(_) => {
                    written_files.push(path.clone());
                    self.trades_written
                        .fetch_add(count as u64, Ordering::Relaxed);
                    debug!(
                        "Flushed {} trades for {} to {:?}",
                        count,
                        instrument_id.as_ref(),
                        path
                    );
                }
                Err(e) => {
                    errors += 1;
                    error!(
                        "Failed to flush trades for {} to {:?}: {}",
                        instrument_id.as_ref(),
                        path,
                        e
                    );
                }
            }
        }

        // Collect bar batches first to avoid borrow issues
        let bar_batches: Vec<_> = self
            .bar_buffers
            .iter_mut()
            .filter(|(_, buffer)| !buffer.is_empty())
            .filter_map(|(bar_type, buffer)| {
                let count = buffer.len();
                let safe_id = sanitize_id(bar_type.as_ref());
                match buffer.to_record_batch() {
                    Ok(batch) => Some((safe_id, count, batch)),
                    Err(e) => {
                        error!(
                            "Failed to create bar batch for {}: {}",
                            bar_type.as_ref(),
                            e
                        );
                        None
                    }
                }
            })
            .collect();

        // Write bar batches
        for (safe_id, count, batch) in bar_batches {
            let dir = self
                .config
                .output_dir
                .join("bars_1m")
                .join(&safe_id)
                .join(&date_str);
            std::fs::create_dir_all(&dir)?;

            let path = dir.join(format!("{}_{}.parquet", hour_str, flush_seq));
            match Self::write_batch_static(&path, batch, self.config.fsync) {
                Ok(_) => {
                    written_files.push(path.clone());
                    self.bars_written.fetch_add(count as u64, Ordering::Relaxed);
                    debug!("Flushed {} bars for {} to {:?}", count, safe_id, path);
                }
                Err(e) => {
                    errors += 1;
                    error!("Failed to flush bars for {} to {:?}: {}", safe_id, path, e);
                }
            }
        }

        // Collect extended bar batches
        let extended_bar_batches: Vec<_> = self
            .extended_bar_buffers
            .iter_mut()
            .filter(|(_, buffer)| !buffer.is_empty())
            .filter_map(|(instrument_id, buffer)| {
                let count = buffer.len();
                let safe_id = sanitize_id(instrument_id.as_ref());
                match buffer.to_record_batch() {
                    Ok(batch) => Some((safe_id, count, batch)),
                    Err(e) => {
                        error!(
                            "Failed to create extended bar batch for {}: {}",
                            instrument_id.as_ref(),
                            e
                        );
                        None
                    }
                }
            })
            .collect();

        // Write extended bar batches (separate directory from core bars)
        for (safe_id, count, batch) in extended_bar_batches {
            let dir = self
                .config
                .output_dir
                .join("extended_bars_1m")
                .join(&safe_id)
                .join(&date_str);
            std::fs::create_dir_all(&dir)?;

            let path = dir.join(format!("{}_{}.parquet", hour_str, flush_seq));
            match Self::write_batch_static(&path, batch, self.config.fsync) {
                Ok(_) => {
                    written_files.push(path.clone());
                    // Extended bars also count toward bars_written
                    self.bars_written.fetch_add(count as u64, Ordering::Relaxed);
                    debug!(
                        "Flushed {} extended bars for {} to {:?}",
                        count, safe_id, path
                    );
                }
                Err(e) => {
                    errors += 1;
                    error!(
                        "Failed to flush extended bars for {} to {:?}: {}",
                        safe_id, path, e
                    );
                }
            }
        }

        // Collect order book delta batches
        let order_book_delta_batches: Vec<_> = self
            .order_book_delta_buffers
            .iter_mut()
            .filter(|(_, buffer)| !buffer.is_empty())
            .filter_map(|(instrument_id, buffer)| {
                let count = buffer.len();
                let safe_id = sanitize_id(instrument_id.as_ref());
                match buffer.to_record_batch() {
                    Ok(batch) => Some((safe_id, count, batch)),
                    Err(e) => {
                        error!(
                            "Failed to create order book delta batch for {}: {}",
                            instrument_id.as_ref(),
                            e
                        );
                        None
                    }
                }
            })
            .collect();

        // Write order book delta batches
        for (safe_id, count, batch) in order_book_delta_batches {
            let dir = self
                .config
                .output_dir
                .join("order_book_deltas")
                .join(&safe_id)
                .join(&date_str);
            std::fs::create_dir_all(&dir)?;

            let path = dir.join(format!("{}_{}.parquet", hour_str, flush_seq));
            match Self::write_batch_static(&path, batch, self.config.fsync) {
                Ok(_) => {
                    written_files.push(path.clone());
                    self.order_book_deltas_written
                        .fetch_add(count as u64, Ordering::Relaxed);
                    debug!(
                        "Flushed {} order book deltas for {} to {:?}",
                        count, safe_id, path
                    );
                }
                Err(e) => {
                    errors += 1;
                    error!(
                        "Failed to flush order book deltas for {} to {:?}: {}",
                        safe_id, path, e
                    );
                }
            }
        }

        if errors > 0 {
            warn!("Parquet flush completed with {} error(s)", errors);
        }

        Ok(written_files)
    }

    /// Write a record batch to a Parquet file ATOMICALLY.
    ///
    /// Uses temp file + fsync + rename pattern to prevent partial reads by watchers.
    fn write_batch_static(
        path: &Path,
        batch: RecordBatch,
        fsync: bool,
    ) -> Result<(), ParquetError> {
        if batch.num_rows() == 0 {
            return Ok(());
        }

        let tmp_path = path.with_extension("parquet.tmp");

        let props = WriterProperties::builder()
            .set_compression(Compression::SNAPPY)
            .build();

        // 1. Write to temp file
        {
            let file = File::create(&tmp_path)?;
            let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(props))?;
            if let Err(err) = writer.write(&batch) {
                if Self::is_writer_finished_error(&err) {
                    warn!(
                        "Parquet writer already finished for {:?}: {}",
                        tmp_path, err
                    );
                } else {
                    return Err(ParquetError::Parquet(err));
                }
            }
            if let Err(err) = writer.close() {
                if Self::is_writer_finished_error(&err) {
                    warn!(
                        "Parquet writer already finished on close for {:?}: {}",
                        tmp_path, err
                    );
                } else {
                    return Err(ParquetError::Parquet(err));
                }
            }
        }

        // 2. Fsync temp file to ensure durability before rename
        if fsync {
            fsync_path(&tmp_path)?;
        }

        // 3. Atomic rename (safe for watchers)
        std::fs::rename(&tmp_path, path)?;

        Ok(())
    }

    fn is_writer_finished_error(err: &parquet::errors::ParquetError) -> bool {
        err.to_string()
            .contains("SerializedFileWriter already finished")
    }

    /// Get total trades written.
    pub fn trades_written(&self) -> u64 {
        self.trades_written.load(Ordering::Relaxed)
    }

    /// Get total bars written.
    pub fn bars_written(&self) -> u64 {
        self.bars_written.load(Ordering::Relaxed)
    }

    /// Get total order book deltas written.
    pub fn order_book_deltas_written(&self) -> u64 {
        self.order_book_deltas_written.load(Ordering::Relaxed)
    }
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn fsync_path(path: &Path) -> Result<(), ParquetError> {
    let file = File::open(path)?;
    if Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| file.sync_all())?;
    } else {
        file.sync_all()?;
    }
    Ok(())
}

/// Run the Parquet writer as an async task.
///
/// Consumes events from the channel and periodically flushes to disk.
/// If S3 is enabled, also uploads written files to S3 after each flush.
pub async fn run_parquet_writer_task(mut rx: mpsc::Receiver<ParquetEvent>, config: ParquetConfig) {
    info!(
        "Parquet writer starting: output_dir={:?}, flush_interval={}s, s3_enabled={}",
        config.output_dir, config.flush_interval_secs, config.s3_enabled
    );
    info!("Parquet precision mode: {:?}", config.precision_mode);

    // Ensure output directory exists
    if let Err(e) = std::fs::create_dir_all(&config.output_dir) {
        error!("Failed to create Parquet output directory: {}", e);
        return;
    }

    // Initialize S3 storage if enabled
    let s3_storage = if config.s3_enabled {
        match (config.s3_bucket.as_ref(), config.s3_region.as_ref()) {
            (Some(bucket), region) => {
                match crate::storage::S3Storage::new(bucket.clone(), region.cloned()).await {
                    Ok(storage) => {
                        info!("S3 storage initialized: bucket={}", bucket);
                        Some(storage)
                    }
                    Err(e) => {
                        error!(
                            "Failed to initialize S3 storage, continuing with local only: {}",
                            e
                        );
                        None
                    }
                }
            }
            _ => {
                error!("S3 enabled but S3_BUCKET not set, continuing with local only");
                None
            }
        }
    } else {
        None
    };

    let output_dir = config.output_dir.clone();
    let flush_secs = config.flush_interval_secs;
    let retry_max = config.event_retry_max;
    let mut writer = ParquetWriter::new(config);
    let mut flush_interval = interval(Duration::from_secs(flush_secs));
    let mut events_since_flush = 0u64;
    let mut retry_queue: VecDeque<(ParquetEvent, usize)> = VecDeque::new();

    fn drain_retry_queue(
        writer: &mut ParquetWriter,
        retry_queue: &mut VecDeque<(ParquetEvent, usize)>,
        retry_max: usize,
        events_since_flush: &mut u64,
    ) {
        let mut pending = retry_queue.len();
        while pending > 0 {
            pending -= 1;
            if let Some((event, attempts)) = retry_queue.pop_front() {
                match writer.process_event(event.clone()) {
                    Ok(_) => {
                        *events_since_flush += 1;
                    }
                    Err(err) => {
                        if attempts < retry_max {
                            retry_queue.push_back((event, attempts + 1));
                            warn!(
                                "Parquet retry {}/{} failed: {}",
                                attempts + 1,
                                retry_max,
                                err
                            );
                        } else {
                            error!("Dropping Parquet event after {} retries: {}", attempts, err);
                        }
                    }
                }
            }
        }
    }

    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Some(e) => {
                        if let Err(err) = writer.process_event(e.clone()) {
                            if retry_max > 0 {
                                retry_queue.push_back((e, 1));
                                warn!(
                                    "Parquet process error (queued for retry 1/{}): {}",
                                    retry_max,
                                    err
                                );
                            } else {
                                error!("Parquet process error: {}", err);
                            }
                        } else {
                            events_since_flush += 1;
                        }
                    }
                    None => {
                        info!("Parquet channel closed, performing final flush");
                        if retry_max > 0 && !retry_queue.is_empty() {
                            drain_retry_queue(&mut writer, &mut retry_queue, retry_max, &mut events_since_flush);
                        }
                        match writer.flush() {
                            Ok(files) => {
                                if let Some(ref s3) = s3_storage {
                                    upload_to_s3(s3, &files, &output_dir).await;
                                }
                            }
                            Err(e) => error!("Final flush failed: {}", e),
                        }
                        break;
                    }
                }
            }
            _ = flush_interval.tick() => {
                if retry_max > 0 && !retry_queue.is_empty() {
                    drain_retry_queue(&mut writer, &mut retry_queue, retry_max, &mut events_since_flush);
                }
                if events_since_flush > 0 {
                    info!(
                        "Parquet flush: {} events buffered, trades_total={}, bars_total={}, l2_total={}",
                        events_since_flush,
                        writer.trades_written(),
                        writer.bars_written(),
                        writer.order_book_deltas_written()
                    );
                    match writer.flush() {
                        Ok(files) => {
                            // Upload to S3 if enabled
                            if let Some(ref s3) = s3_storage {
                                upload_to_s3(s3, &files, &output_dir).await;
                            }
                        }
                        Err(e) => error!("Parquet flush failed: {}", e),
                    }
                    events_since_flush = 0;
                }
            }
        }
    }

    info!(
        "Parquet writer stopped. Total: trades={}, bars={}, l2={}",
        writer.trades_written(),
        writer.bars_written(),
        writer.order_book_deltas_written()
    );
}

/// Upload local files to S3.
async fn upload_to_s3(s3: &crate::storage::S3Storage, files: &[PathBuf], output_dir: &Path) {
    use crate::storage::StorageBackend;

    for local_path in files {
        // Compute S3 key as relative path from output_dir
        let s3_key = match local_path.strip_prefix(output_dir) {
            Ok(rel) => rel.to_string_lossy().to_string(),
            Err(_) => {
                error!("Failed to compute S3 key for {:?}", local_path);
                continue;
            }
        };

        // Read local file and upload
        match std::fs::read(local_path) {
            Ok(data) => match s3.write(&s3_key, &data).await {
                Ok(()) => debug!("Uploaded {} bytes to s3://{}", data.len(), s3_key),
                Err(e) => error!("S3 upload failed for {}: {}", s3_key, e),
            },
            Err(e) => error!(
                "Failed to read local file {:?} for S3 upload: {}",
                local_path, e
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_trade_buffer_append_and_batch() {
        let meta = TradeMetadata::binance_perp("BTCUSDT", 2, 3);
        let mut buffer = TradeBuffer::new(meta, 100, PrecisionMode::High);

        let trade = TradeEvent {
            instrument_id: "BTCUSDT-PERP.BINANCE".to_string(),
            price: 100_000.50,
            size: 0.001,
            side: Some(barter_instrument::Side::Buy),
            trade_id: "12345".to_string(),
            ts_event_ns: 1_000_000_000,
            ts_init_ns: 1_000_000_100,
            price_precision: 2,
            size_precision: 3,
        };

        buffer.append(&trade).unwrap();
        assert_eq!(buffer.len(), 1);

        let batch = buffer.to_record_batch().unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 6);
    }

    #[test]
    fn test_bar_buffer_append_and_batch() {
        let meta = BarMetadata::binance_perp("BTCUSDT", 2, 3);
        let mut buffer = BarBuffer::new(meta, 100, PrecisionMode::High);

        let bar = BarEvent {
            bar_type: "BTCUSDT-PERP.BINANCE-1-MINUTE-LAST-EXTERNAL".to_string(),
            instrument_id: "BTCUSDT-PERP.BINANCE".to_string(),
            open: 100_000.0,
            high: 100_100.0,
            low: 99_900.0,
            close: 100_050.0,
            volume: 10.5,
            ts_event_ns: 1_000_000_000,
            ts_init_ns: 1_000_000_100,
            price_precision: 2,
            size_precision: 3,
        };

        buffer.append(&bar).unwrap();
        assert_eq!(buffer.len(), 1);

        let batch = buffer.to_record_batch().unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 7);
    }

    #[test]
    fn test_order_book_delta_buffer_append_and_batch() {
        let meta = OrderBookDeltaMetadata::binance_perp("BTCUSDT", 2, 3);
        let mut buffer = OrderBookDeltaBuffer::new(meta, 100, PrecisionMode::High);

        let delta = OrderBookDeltaEvent {
            instrument_id: "BTCUSDT-PERP.BINANCE".to_string(),
            action: 1,
            side: 1,
            price: 100_000.0,
            size: 1.0,
            order_id: 0,
            flags: 16,
            sequence: 42,
            ts_event_ns: 1_000_000_000,
            ts_init_ns: 1_000_000_100,
            price_precision: 2,
            size_precision: 3,
        };

        buffer.append(&delta).unwrap();
        assert_eq!(buffer.len(), 1);

        let batch = buffer.to_record_batch().unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 9);
    }

    #[test]
    fn test_writer_flush_creates_files() {
        let temp_dir = TempDir::new().unwrap();
        let config = ParquetConfig {
            output_dir: temp_dir.path().to_path_buf(),
            flush_interval_secs: 60,
            buffer_size: 1000,
            s3_enabled: false,
            s3_bucket: None,
            s3_region: None,
            precision_mode: PrecisionMode::High,
            fsync: false,
            event_retry_max: 0,
        };

        let mut writer = ParquetWriter::new(config);

        // Add a trade
        let trade = ParquetEvent::Trade(TradeEvent {
            instrument_id: "BTCUSDT-PERP.BINANCE".to_string(),
            price: 100_000.50,
            size: 0.001,
            side: Some(barter_instrument::Side::Buy),
            trade_id: "12345".to_string(),
            ts_event_ns: 1_000_000_000,
            ts_init_ns: 1_000_000_100,
            price_precision: 2,
            size_precision: 3,
        });
        writer.process_event(trade).unwrap();

        // Add a bar
        let bar = ParquetEvent::Bar(BarEvent {
            bar_type: "BTCUSDT-PERP.BINANCE-1-MINUTE-LAST-EXTERNAL".to_string(),
            instrument_id: "BTCUSDT-PERP.BINANCE".to_string(),
            open: 100_000.0,
            high: 100_100.0,
            low: 99_900.0,
            close: 100_050.0,
            volume: 10.5,
            ts_event_ns: 1_000_000_000,
            ts_init_ns: 1_000_000_100,
            price_precision: 2,
            size_precision: 3,
        });
        writer.process_event(bar).unwrap();

        // Flush
        writer.flush().unwrap();

        // Check files were created
        assert!(temp_dir.path().join("trades").exists());
        assert!(temp_dir.path().join("bars_1m").exists());
        assert_eq!(writer.trades_written(), 1);
        assert_eq!(writer.bars_written(), 1);
    }
}
