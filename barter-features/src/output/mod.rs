//! Output writing modules.

pub mod schema;
pub mod writer;

pub use schema::{OutputMetadata, large_trade_schema, profile_event_schema, tpo_bracket_schema};
pub use writer::{FeatureWriter, write_parquet_atomic};
