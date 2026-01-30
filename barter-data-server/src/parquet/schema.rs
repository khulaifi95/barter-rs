//! Nautilus-compatible Arrow schemas for bars and trades.
//!
//! These schemas MUST match exactly what Nautilus Trader expects:
//! - Column order matters
//! - Column types must be exact
//! - Metadata keys must match

use arrow::datatypes::{DataType, Field, Schema};
use crate::parquet::encoder::PrecisionMode;
use std::collections::HashMap;
use std::sync::Arc;

/// Metadata for bar parquet files (required by Nautilus).
#[derive(Debug, Clone)]
pub struct BarMetadata {
    /// Bar type string in Nautilus format: `{symbol}.{venue}-{step}-{aggregation}-{price_type}-EXTERNAL`
    /// Example: `BTCUSDT-PERP.BINANCE-1-MINUTE-LAST-EXTERNAL`
    pub bar_type: String,
    /// Instrument ID in Nautilus format: `{symbol}.{venue}`
    /// Example: `BTCUSDT-PERP.BINANCE`
    pub instrument_id: String,
    /// Price precision (decimal places)
    pub price_precision: u8,
    /// Size/volume precision (decimal places)
    pub size_precision: u8,
}

impl BarMetadata {
    /// Create metadata for a Binance perpetual instrument.
    pub fn binance_perp(symbol: &str, price_precision: u8, size_precision: u8) -> Self {
        let instrument_id = format!("{}-PERP.BINANCE", symbol);
        Self {
            bar_type: format!("{}-1-MINUTE-LAST-EXTERNAL", instrument_id),
            instrument_id,
            price_precision,
            size_precision,
        }
    }

    /// Convert to Arrow schema metadata map.
    pub fn to_schema_metadata(&self) -> HashMap<String, String> {
        let mut meta = HashMap::new();
        meta.insert("bar_type".to_string(), self.bar_type.clone());
        meta.insert("instrument_id".to_string(), self.instrument_id.clone());
        meta.insert("price_precision".to_string(), self.price_precision.to_string());
        meta.insert("size_precision".to_string(), self.size_precision.to_string());
        meta
    }
}

/// Metadata for trade parquet files (required by Nautilus).
#[derive(Debug, Clone)]
pub struct TradeMetadata {
    /// Instrument ID in Nautilus format: `{symbol}.{venue}`
    /// Example: `BTCUSDT-PERP.BINANCE`
    pub instrument_id: String,
    /// Price precision (decimal places)
    pub price_precision: u8,
    /// Size precision (decimal places)
    pub size_precision: u8,
}

impl TradeMetadata {
    /// Create metadata for a Binance perpetual instrument.
    pub fn binance_perp(symbol: &str, price_precision: u8, size_precision: u8) -> Self {
        Self {
            instrument_id: format!("{}-PERP.BINANCE", symbol),
            price_precision,
            size_precision,
        }
    }

    /// Convert to Arrow schema metadata map.
    pub fn to_schema_metadata(&self) -> HashMap<String, String> {
        let mut meta = HashMap::new();
        meta.insert("instrument_id".to_string(), self.instrument_id.clone());
        meta.insert("price_precision".to_string(), self.price_precision.to_string());
        meta.insert("size_precision".to_string(), self.size_precision.to_string());
        meta
    }
}

/// Create the Nautilus-compatible Arrow schema for bars.
///
/// **CRITICAL: Column order and types must match exactly!**
///
/// Precision mode determines FixedSizeBinary width:
/// - Standard: FixedSizeBinary(8) (i64 × 1e9)
/// - High: FixedSizeBinary(16) (i128 × 1e16)
///
/// | Column | Type | Description |
/// |--------|------|-------------|
/// | open | FixedSizeBinary(PRECISION_BYTES) | fixed-point |
/// | high | FixedSizeBinary(PRECISION_BYTES) | fixed-point |
/// | low | FixedSizeBinary(PRECISION_BYTES) | fixed-point |
/// | close | FixedSizeBinary(PRECISION_BYTES) | fixed-point |
/// | volume | FixedSizeBinary(PRECISION_BYTES) | fixed-point |
/// | ts_event | UInt64 | Bar CLOSE time (nanos) |
/// | ts_init | UInt64 | Ingest time (nanos) |
pub fn bar_schema(metadata: &BarMetadata, precision_mode: PrecisionMode) -> Arc<Schema> {
    let bytes = precision_mode.bytes_len();
    let fields = vec![
        Field::new("open", DataType::FixedSizeBinary(bytes), false),
        Field::new("high", DataType::FixedSizeBinary(bytes), false),
        Field::new("low", DataType::FixedSizeBinary(bytes), false),
        Field::new("close", DataType::FixedSizeBinary(bytes), false),
        Field::new("volume", DataType::FixedSizeBinary(bytes), false),
        Field::new("ts_event", DataType::UInt64, false),
        Field::new("ts_init", DataType::UInt64, false),
    ];
    Arc::new(Schema::new_with_metadata(fields, metadata.to_schema_metadata()))
}

/// Create the Nautilus-compatible Arrow schema for trades.
///
/// **CRITICAL: Column order and types must match exactly!**
///
/// Precision mode determines FixedSizeBinary width:
/// - Standard: FixedSizeBinary(8) (i64 × 1e9)
/// - High: FixedSizeBinary(16) (i128 × 1e16)
///
/// | Column | Type | Description |
/// |--------|------|-------------|
/// | price | FixedSizeBinary(PRECISION_BYTES) | fixed-point |
/// | size | FixedSizeBinary(PRECISION_BYTES) | fixed-point |
/// | aggressor_side | UInt8 | 0=NO_AGGRESSOR, 1=BUYER, 2=SELLER |
/// | trade_id | Utf8 | Exchange trade ID |
/// | ts_event | UInt64 | Exchange timestamp (nanos) |
/// | ts_init | UInt64 | Ingest time (nanos) |
pub fn trade_schema(metadata: &TradeMetadata, precision_mode: PrecisionMode) -> Arc<Schema> {
    let bytes = precision_mode.bytes_len();
    let fields = vec![
        Field::new("price", DataType::FixedSizeBinary(bytes), false),
        Field::new("size", DataType::FixedSizeBinary(bytes), false),
        Field::new("aggressor_side", DataType::UInt8, false),
        Field::new("trade_id", DataType::Utf8, false),
        Field::new("ts_event", DataType::UInt64, false),
        Field::new("ts_init", DataType::UInt64, false),
    ];
    Arc::new(Schema::new_with_metadata(fields, metadata.to_schema_metadata()))
}

/// Aggressor side values matching Nautilus enum.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggressorSide {
    NoAggressor = 0,
    Buyer = 1,
    Seller = 2,
}

impl AggressorSide {
    /// Convert from barter Side.
    pub fn from_side(side: Option<barter_instrument::Side>) -> Self {
        match side {
            Some(barter_instrument::Side::Buy) => AggressorSide::Buyer,
            Some(barter_instrument::Side::Sell) => AggressorSide::Seller,
            None => AggressorSide::NoAggressor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bar_schema_column_count() {
        let meta = BarMetadata::binance_perp("BTCUSDT", 2, 3);
        let schema = bar_schema(&meta, PrecisionMode::High);
        assert_eq!(schema.fields().len(), 7);
    }

    #[test]
    fn test_bar_schema_column_order() {
        let meta = BarMetadata::binance_perp("BTCUSDT", 2, 3);
        let schema = bar_schema(&meta, PrecisionMode::High);
        let names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, vec!["open", "high", "low", "close", "volume", "ts_event", "ts_init"]);
    }

    #[test]
    fn test_bar_schema_metadata() {
        let meta = BarMetadata::binance_perp("BTCUSDT", 2, 3);
        let schema = bar_schema(&meta, PrecisionMode::High);
        assert_eq!(
            schema.metadata().get("bar_type"),
            Some(&"BTCUSDT-PERP.BINANCE-1-MINUTE-LAST-EXTERNAL".to_string())
        );
        assert_eq!(schema.metadata().get("price_precision"), Some(&"2".to_string()));
        assert_eq!(schema.metadata().get("size_precision"), Some(&"3".to_string()));
    }

    #[test]
    fn test_trade_schema_column_count() {
        let meta = TradeMetadata::binance_perp("BTCUSDT", 2, 3);
        let schema = trade_schema(&meta, PrecisionMode::High);
        assert_eq!(schema.fields().len(), 6);
    }

    #[test]
    fn test_trade_schema_column_order() {
        let meta = TradeMetadata::binance_perp("BTCUSDT", 2, 3);
        let schema = trade_schema(&meta, PrecisionMode::High);
        let names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, vec!["price", "size", "aggressor_side", "trade_id", "ts_event", "ts_init"]);
    }

    #[test]
    fn test_trade_schema_metadata() {
        let meta = TradeMetadata::binance_perp("BTCUSDT", 2, 3);
        let schema = trade_schema(&meta, PrecisionMode::High);
        assert_eq!(
            schema.metadata().get("instrument_id"),
            Some(&"BTCUSDT-PERP.BINANCE".to_string())
        );
    }

    #[test]
    fn test_aggressor_side_values() {
        assert_eq!(AggressorSide::NoAggressor as u8, 0);
        assert_eq!(AggressorSide::Buyer as u8, 1);
        assert_eq!(AggressorSide::Seller as u8, 2);
    }

    #[test]
    fn test_extended_bar_schema_column_count() {
        let meta = ExtendedBarMetadata::new("BTCUSDT-PERP.BINANCE", 2, 3);
        let schema = extended_bar_schema(&meta);
        assert_eq!(schema.fields().len(), 43);
    }
}

/// Metadata for extended bar parquet files (Barter-only, not Nautilus core).
#[derive(Debug, Clone)]
pub struct ExtendedBarMetadata {
    /// Instrument ID
    pub instrument_id: String,
    /// Price precision (decimal places)
    pub price_precision: u8,
    /// Size precision (decimal places)
    pub size_precision: u8,
}

impl ExtendedBarMetadata {
    pub fn new(instrument_id: &str, price_precision: u8, size_precision: u8) -> Self {
        Self {
            instrument_id: instrument_id.to_string(),
            price_precision,
            size_precision,
        }
    }

    /// Convert to Arrow schema metadata map.
    pub fn to_schema_metadata(&self) -> HashMap<String, String> {
        let mut meta = HashMap::new();
        meta.insert("instrument_id".to_string(), self.instrument_id.clone());
        meta.insert("price_precision".to_string(), self.price_precision.to_string());
        meta.insert("size_precision".to_string(), self.size_precision.to_string());
        meta.insert("schema_version".to_string(), "2".to_string());
        meta
    }
}

/// Create the extended bar schema (Barter-only, for joining with core bars on ts_event).
///
/// Unlike Nautilus core bars, this uses simple types for easier querying.
pub fn extended_bar_schema(metadata: &ExtendedBarMetadata) -> Arc<Schema> {
    let fields = vec![
        // Timestamps (UInt64 nanos - same as Nautilus for joins)
        Field::new("ts_event", DataType::UInt64, false),  // Bar CLOSE time
        Field::new("ts_init", DataType::UInt64, false),   // Ingest time
        Field::new("ts_open", DataType::UInt64, false),   // Bar OPEN time (for TradingView)
        // Identity
        Field::new("instrument_id", DataType::Utf8, false),
        // OHLCV (fixed-point i64, Barter-only)
        Field::new("open", DataType::Int64, false),
        Field::new("high", DataType::Int64, false),
        Field::new("low", DataType::Int64, false),
        Field::new("close", DataType::Int64, false),
        Field::new("volume", DataType::Int64, false),
        Field::new("quote_volume", DataType::Int64, false),
        Field::new("trade_count", DataType::UInt64, false),
        // Delta/CVD
        Field::new("buy_volume", DataType::Int64, false),
        Field::new("sell_volume", DataType::Int64, false),
        Field::new("delta", DataType::Int64, false),
        Field::new("cvd", DataType::Int64, false),
        // Derivatives / L1
        Field::new("open_interest", DataType::Int64, false),
        Field::new("oi_change", DataType::Int64, false),
        Field::new("funding_rate", DataType::Float64, false),
        Field::new("bid_price", DataType::Int64, false),
        Field::new("bid_size", DataType::Int64, false),
        Field::new("ask_price", DataType::Int64, false),
        Field::new("ask_size", DataType::Int64, false),
        Field::new("spread_bps", DataType::Float64, false),
        Field::new("book_imbalance", DataType::Float64, false),
        // Liquidations (1m aggregates, quote notional)
        Field::new("liq_buy_usd", DataType::Int64, false),
        Field::new("liq_sell_usd", DataType::Int64, false),
        Field::new("liq_total_usd", DataType::Int64, false),
        Field::new("liq_count", DataType::UInt64, false),
        // L2 depth bands (base + notional within bps of mid)
        Field::new("bid_depth_10bps_base", DataType::Int64, false),
        Field::new("ask_depth_10bps_base", DataType::Int64, false),
        Field::new("bid_depth_10bps_usd", DataType::Int64, false),
        Field::new("ask_depth_10bps_usd", DataType::Int64, false),
        Field::new("depth_imb_10bps", DataType::Float64, false),
        Field::new("bid_depth_50bps_base", DataType::Int64, false),
        Field::new("ask_depth_50bps_base", DataType::Int64, false),
        Field::new("bid_depth_50bps_usd", DataType::Int64, false),
        Field::new("ask_depth_50bps_usd", DataType::Int64, false),
        Field::new("depth_imb_50bps", DataType::Float64, false),
        Field::new("bid_depth_100bps_base", DataType::Int64, false),
        Field::new("ask_depth_100bps_base", DataType::Int64, false),
        Field::new("bid_depth_100bps_usd", DataType::Int64, false),
        Field::new("ask_depth_100bps_usd", DataType::Int64, false),
        Field::new("depth_imb_100bps", DataType::Float64, false),
    ];
    Arc::new(Schema::new_with_metadata(fields, metadata.to_schema_metadata()))
}
