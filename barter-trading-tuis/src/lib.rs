/// Barter Trading TUIs - Shared Library
///
/// This library provides common functionality for the three TUI binaries:
/// - market-microstructure: Advanced market microstructure analytics
/// - institutional-flow: Institutional order flow analysis
/// - risk-scanner: Real-time risk monitoring
///
/// The library includes:
/// - Core data types for market events
/// - WebSocket client for connecting to the aggregated server
/// - Aggregation utilities for market data analysis
pub mod shared;
pub mod views;

// Re-export commonly used types for convenience
pub use shared::types::{
    CvdData, InstrumentInfo, Level, LiquidationData, MarketEventMessage, OpenInterestData,
    OrderBookL1Data, Side, TradeData,
};

pub use shared::websocket::ConnectionStatus;
pub use shared::websocket::{WebSocketClient, WebSocketConfig};

pub use shared::aggregation::{calculate_vwap, VolumeWindow};

// Traditional markets (ES/NQ) correlation module
pub use shared::trad_markets::{
    render_trad_markets_panel, spawn_ibkr_feed, CorrelationSignals, IbkrConnectionStatus,
    TradMarketState,
};

// Market State Engine - orchestrator result for TUI display
pub use shared::orchestrator::OrchestratorResult;

// Aggregation engine (shared across all TUIs)
pub use shared::state::{
    fetch_binance_1m_candles,
    ticker_to_binance_symbol,
    AggregatedSnapshot,
    Aggregator,
    BackfillResult,
    BasisMomentum,
    BasisState,
    BasisStats,
    BasisTrend,
    // 1m kline support (authoritative source for tvVWAP/ATR/RV)
    Candle1m,
    CascadeLevel,
    CvdSummary,
    DivergenceSignal,
    FlowSignal,
    LiquidationCluster,
    OrderflowStats,
    TickDirection,
    TickerSnapshot,
    TradingSession,
    VolTrend,
    WhaleRecord,
};
