//! Traditional Markets (ES/NQ) correlation module for Scalper V2
//!
//! Provides:
//! - 5-second micro-bar aggregation from live ticks
//! - Correlation, divergence z-score, and lead/lag calculations
//! - WebSocket feed handler for ibkr-bridge
//! - Ratatui widget for display

mod aggregator;
mod calc;
mod feed;
mod state;
mod widget;

pub use aggregator::{BarBuffer, MicroBar, MicroBarAggregator};
pub use calc::{calc_correlation, calc_divergence_zscore, calc_lead_lag};
pub use feed::{spawn_ibkr_feed, IbkrConnectionStatus};
pub use state::{CorrelationSignals, TradMarketState};
pub use widget::render_trad_markets_panel;
