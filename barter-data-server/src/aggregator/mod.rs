//! Trade aggregation module for building 1-minute bars.
//!
//! This module provides:
//! - `MinuteBarBuilder` - Accumulates trades into 1-minute OHLCV bars
//! - `DeltaTracker` - Tracks buy/sell volume delta and CVD
//! - `ExtendedBar` - Extended bar with OI, funding, delta, L2, liquidations

pub mod delta;
pub mod depth_bands;
pub mod extended_bar;
pub mod minute_bar;

#[allow(unused_imports)]
pub use delta::DeltaTracker;
#[allow(unused_imports)]
pub use depth_bands::compute_depth_bands;
#[allow(unused_imports)]
pub use extended_bar::ExtendedBar;
#[allow(unused_imports)]
pub use minute_bar::{MinuteBar, MinuteBarAggregator, MinuteBarBuilder};
