//! barter-features: Feature computation layer for barter-data-server.
//!
//! This crate processes raw market data parquet files to compute actionable trading features:
//!
//! - **TPO Brackets**: 30-minute time-price-opportunity brackets with volume profile
//! - **Large Trades**: Detection of significant trades ($2M+, $5M+, $10M+)
//! - **Profile Events**: POC shifts, IB breaks, gap detection
//!
//! # Usage
//!
//! ```ignore
//! use barter_features::{Config, Pipeline};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = Config::load()?;
//!     let mut pipeline = Pipeline::new(config)?;
//!
//!     // Run in batch mode (process existing files)
//!     pipeline.run_batch().await?;
//!
//!     // Or run in watch mode (continuous processing)
//!     // pipeline.run_watch().await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # Precision
//!
//! This crate matches the precision encoding of barter-data-server:
//! - Standard mode: i64 × 1e9 (FixedSizeBinary(8))
//! - High mode: i128 × 1e16 (FixedSizeBinary(16))
//!
//! Set `NAUTILUS_PRECISION=standard` or `NAUTILUS_PRECISION=high` to match your collector.

pub mod checkpoint;
pub mod config;
pub mod error;
pub mod features;
pub mod output;
pub mod pipeline;
pub mod precision;
pub mod reader;
pub mod watcher;

pub use checkpoint::Checkpoint;
pub use config::Config;
pub use error::{FeatureError, Result};
pub use features::{
    EventDetector, LargeTrade, LargeTradeCategory, LargeTradeDetector, ProfileEvent,
    ProfileEventType, TpoBracket, TpoProcessor, ValueArea, VolumeProfile, bracket_label,
    compute_value_area,
};
pub use output::{FeatureWriter, OutputMetadata};
pub use pipeline::Pipeline;
pub use precision::{
    PrecisionMode, decode_extended_bar_value, decode_fixed_point, encode_for_output,
};
pub use reader::{ExtendedBar, ExtendedBarReader, read_extended_bars};
pub use watcher::{AsyncFileWatcher, FileWatcher, is_file_ready, scan_ready_files};
