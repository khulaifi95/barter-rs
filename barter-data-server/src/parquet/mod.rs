//! Parquet module for Nautilus-compatible data storage.
//!
//! This module provides:
//! - Arrow schemas matching Nautilus Trader format
//! - Fixed-point encoding for prices/quantities
//! - Buffered Parquet file writing

pub mod encoder;
pub mod schema;
pub mod writer;

#[allow(unused_imports)]
pub use encoder::{PrecisionMode, decode_fixed_point, encode_fixed_point};
#[allow(unused_imports)]
pub use schema::{
    BarMetadata, ExtendedBarMetadata, TradeMetadata, bar_schema, extended_bar_schema, trade_schema,
};
#[allow(unused_imports)]
pub use writer::{ParquetConfig, ParquetEvent, ParquetWriter, run_parquet_writer_task};
