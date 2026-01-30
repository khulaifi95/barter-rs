//! Parquet module for Nautilus-compatible data storage.
//!
//! This module provides:
//! - Arrow schemas matching Nautilus Trader format
//! - Fixed-point encoding for prices/quantities
//! - Buffered Parquet file writing

pub mod encoder;
pub mod schema;
pub mod writer;

pub use encoder::{encode_fixed_point, decode_fixed_point, PrecisionMode};
pub use schema::{
    bar_schema, trade_schema, extended_bar_schema, BarMetadata, TradeMetadata, ExtendedBarMetadata,
};
pub use writer::{ParquetConfig, ParquetEvent, ParquetWriter, run_parquet_writer_task};
