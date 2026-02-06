//! Arrow schemas for feature output.

use arrow::datatypes::{DataType, Field, Schema};

/// Metadata included in every output row.
#[derive(Debug, Clone)]
pub struct OutputMetadata {
    /// Schema version (e.g., "1.2.0")
    pub schema_version: String,
    /// Hash of config used
    pub config_hash: String,
    /// Source precision mode ("standard" or "high")
    pub source_precision: String,
    /// Output precision exponent (9 means × 1e9)
    pub output_precision: i32,
}

impl Default for OutputMetadata {
    fn default() -> Self {
        Self {
            schema_version: "1.2.0".to_string(),
            config_hash: String::new(),
            source_precision: "high".to_string(),
            output_precision: 9,
        }
    }
}

/// Create the Arrow schema for TPO bracket output.
pub fn tpo_bracket_schema() -> Schema {
    Schema::new(vec![
        // Timestamps
        Field::new("ts_event", DataType::Int64, false),
        Field::new("ts_init", DataType::Int64, false),
        // Metadata
        Field::new("schema_version", DataType::Utf8, false),
        Field::new("config_hash", DataType::Utf8, false),
        Field::new("source_precision", DataType::Utf8, false),
        Field::new("output_precision", DataType::Int32, false),
        // Identifiers
        Field::new("instrument_id", DataType::Utf8, false),
        Field::new("session_id", DataType::Utf8, false),
        Field::new("session_date", DataType::Utf8, false),
        // Bracket info
        Field::new("label", DataType::Utf8, false),
        Field::new("bracket_index", DataType::UInt8, false),
        Field::new("bracket_start_ts", DataType::Int64, false),
        Field::new("bracket_end_ts", DataType::Int64, false),
        // OHLCV (× 1e9)
        Field::new("bracket_open", DataType::Int64, false),
        Field::new("bracket_high", DataType::Int64, false),
        Field::new("bracket_low", DataType::Int64, false),
        Field::new("bracket_close", DataType::Int64, false),
        Field::new("bracket_volume", DataType::Int64, false),
        Field::new("bracket_buy_volume", DataType::Int64, false),
        Field::new("bracket_sell_volume", DataType::Int64, false),
        Field::new("bracket_delta", DataType::Int64, false),
        Field::new("bracket_trade_count", DataType::UInt32, false),
        Field::new("bracket_bar_count", DataType::UInt8, false),
        Field::new("bracket_has_gap", DataType::Boolean, false),
        // Volume Profile (× 1e9)
        Field::new("vol_poc", DataType::Int64, false),
        Field::new("vol_vah", DataType::Int64, false),
        Field::new("vol_val", DataType::Int64, false),
        Field::new("vol_total", DataType::Int64, false),
        // TPO Profile (× 1e9)
        Field::new("tpo_poc", DataType::Int64, false),
        Field::new("tpo_vah", DataType::Int64, false),
        Field::new("tpo_val", DataType::Int64, false),
        Field::new("tpo_total_periods", DataType::UInt16, false),
        // Initial Balance (× 1e9)
        Field::new("ib_high", DataType::Int64, false),
        Field::new("ib_low", DataType::Int64, false),
        Field::new("ib_complete", DataType::Boolean, false),
        // Analysis
        Field::new("poc_divergence", DataType::Int64, false),
        Field::new("session_type", DataType::Utf8, false),
    ])
}

/// Create the Arrow schema for large trade output.
pub fn large_trade_schema() -> Schema {
    Schema::new(vec![
        // Timestamps
        Field::new("ts_event", DataType::Int64, false),
        Field::new("ts_init", DataType::Int64, false),
        // Metadata
        Field::new("schema_version", DataType::Utf8, false),
        Field::new("config_hash", DataType::Utf8, false),
        Field::new("source_precision", DataType::Utf8, false),
        Field::new("output_precision", DataType::Int32, false),
        // Identifiers
        Field::new("instrument_id", DataType::Utf8, false),
        Field::new("trade_id", DataType::Utf8, false),
        // Trade data (all values × 1e9 for consistency)
        Field::new("price", DataType::Int64, false),
        Field::new("size", DataType::Int64, false),
        Field::new("side", DataType::Utf8, false),
        Field::new("notional_usd", DataType::Int64, false), // × 1e9
        Field::new("category", DataType::Utf8, false),
        Field::new("threshold_used", DataType::Int64, false), // × 1e9
        Field::new("threshold_mode", DataType::Utf8, false),
    ])
}

/// Create the Arrow schema for profile event output.
pub fn profile_event_schema() -> Schema {
    Schema::new(vec![
        // Timestamps
        Field::new("ts_event", DataType::Int64, false),
        Field::new("ts_init", DataType::Int64, false),
        // Metadata (all rows carry metadata for consistency)
        Field::new("schema_version", DataType::Utf8, false),
        Field::new("config_hash", DataType::Utf8, false),
        Field::new("source_precision", DataType::Utf8, false),
        Field::new("output_precision", DataType::Int32, false),
        // Identifiers
        Field::new("instrument_id", DataType::Utf8, false),
        Field::new("session_id", DataType::Utf8, false),
        Field::new("event_type", DataType::Utf8, false),
        // Profile state (× 1e9)
        Field::new("vol_poc", DataType::Int64, false),
        Field::new("vol_vah", DataType::Int64, false),
        Field::new("vol_val", DataType::Int64, false),
        Field::new("tpo_poc", DataType::Int64, false),
        Field::new("tpo_vah", DataType::Int64, false),
        Field::new("tpo_val", DataType::Int64, false),
        Field::new("ib_high", DataType::Int64, false),
        Field::new("ib_low", DataType::Int64, false),
        // Event-specific (nullable)
        Field::new("break_direction", DataType::Utf8, true),
        Field::new("break_price", DataType::Int64, true),
        Field::new("shift_amount", DataType::Int64, true),
        Field::new("previous_value", DataType::Int64, true),
        // Gap-specific (nullable)
        Field::new("gap_start_ts", DataType::Int64, true),
        Field::new("gap_end_ts", DataType::Int64, true),
        Field::new("missing_bars", DataType::UInt32, true),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tpo_bracket_schema_has_required_fields() {
        let schema = tpo_bracket_schema();

        // Required metadata fields
        assert!(schema.field_with_name("ts_event").is_ok());
        assert!(schema.field_with_name("ts_init").is_ok());
        assert!(schema.field_with_name("schema_version").is_ok());
        assert!(schema.field_with_name("config_hash").is_ok());
        assert!(schema.field_with_name("source_precision").is_ok());
        assert!(schema.field_with_name("output_precision").is_ok());

        // TPO specific
        assert!(schema.field_with_name("label").is_ok());
        assert!(schema.field_with_name("bracket_index").is_ok());
        assert!(schema.field_with_name("vol_poc").is_ok());
    }

    #[test]
    fn test_large_trade_schema_has_required_fields() {
        let schema = large_trade_schema();

        assert!(schema.field_with_name("ts_event").is_ok());
        assert!(schema.field_with_name("ts_init").is_ok());
        assert!(schema.field_with_name("notional_usd").is_ok());
        assert!(schema.field_with_name("category").is_ok());
    }

    #[test]
    fn test_profile_event_schema_has_nullable_fields() {
        let schema = profile_event_schema();

        // Gap fields should be nullable
        let gap_start = schema.field_with_name("gap_start_ts").unwrap();
        assert!(gap_start.is_nullable());

        let missing_bars = schema.field_with_name("missing_bars").unwrap();
        assert!(missing_bars.is_nullable());
    }
}
