//! Parquet file readers for market data.

pub mod extended_bar;
pub mod trades;

pub use extended_bar::{ExtendedBar, ExtendedBarReader, read_extended_bars};
pub use trades::{
    Trade, TradeReader, read_trades, read_trades_with_precision, trade_instrument_id_from_schema,
};
