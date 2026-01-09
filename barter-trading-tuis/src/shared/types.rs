/// Core data types for market events
///
/// These types match the JSON message format from the WebSocket server
/// at ws://127.0.0.1:9001
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn default_snapshot_version() -> u16 {
    1
}
/// Market event message envelope from the server
///
/// This is the top-level message structure that wraps all event types
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarketEventMessage {
    /// Timestamp when the event occurred on the exchange
    pub time_exchange: DateTime<Utc>,
    /// Timestamp when the event was received by our system
    pub time_received: DateTime<Utc>,
    /// Exchange name (e.g., "Okx", "BinanceFuturesUsd")
    pub exchange: String,
    /// Instrument details (base/quote pair and contract type)
    pub instrument: InstrumentInfo,
    /// Event type: "trade", "liquidation", "open_interest", "cumulative_volume_delta", "order_book_l1"
    pub kind: String,
    /// Event-specific data (deserialize based on `kind` field)
    pub data: serde_json::Value,
}

/// Versioned envelope for market events (optional).
///
/// When present, the payload is a `MarketEventMessage` and includes
/// schema metadata for compatibility during migrations.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarketEventEnvelope {
    pub schema_version: u16,
    pub source: String,
    pub time_sent: DateTime<Utc>,
    pub payload: MarketEventMessage,
}

/// Derived metrics snapshot from the centralized server.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarketSnapshotMessage {
    #[serde(default = "default_snapshot_version")]
    pub snapshot_version: u16,
    pub timestamp: i64,
    pub tickers: HashMap<String, SnapshotTicker>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SnapshotPerExchangeShort {
    pub cvd_30s: f64,
    pub total_30s: f64,
    pub trades_30s: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SnapshotTicker {
    pub price: f64,
    pub cvd_5m: f64,
    pub cvd_15m: f64,
    pub rvol_5m: f64,
    pub oi_delta_5m: f64,
    pub funding_rate: f64,
    pub funding_velocity: f64,
    pub liq_rate_usd_per_min: f64,
    pub vol_percentile: f64,
    pub vol_regime: String,
    #[serde(default)]
    pub vol_samples: u16,
    #[serde(default)]
    pub per_exchange_30s: HashMap<String, SnapshotPerExchangeShort>,
}

/// Instrument information
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InstrumentInfo {
    /// Base currency (e.g., "btc", "eth")
    pub base: String,
    /// Quote currency (e.g., "usdt", "usd")
    pub quote: String,
    /// Contract kind (e.g., "perpetual", "spot", "future")
    pub kind: String,
}

/// Order side (Buy or Sell)
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    /// Convert to display string
    pub fn as_str(&self) -> &'static str {
        match self {
            Side::Buy => "Buy",
            Side::Sell => "Sell",
        }
    }

    /// Check if this is a buy order
    pub fn is_buy(&self) -> bool {
        matches!(self, Side::Buy)
    }

    /// Check if this is a sell order
    pub fn is_sell(&self) -> bool {
        matches!(self, Side::Sell)
    }
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Liquidation event data
///
/// Represents a forced liquidation on an exchange
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LiquidationData {
    /// Side of the liquidated position
    pub side: Side,
    /// Liquidation price
    pub price: f64,
    /// Quantity liquidated
    pub quantity: f64,
    /// Time of liquidation
    pub time: DateTime<Utc>,
}

/// Open Interest event data
///
/// Represents the total outstanding contracts for an instrument
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenInterestData {
    /// Number of open contracts
    pub contracts: f64,
    /// Notional value (optional, may not be provided by all exchanges)
    pub notional: Option<f64>,
    /// Time of the snapshot (optional)
    pub time: Option<DateTime<Utc>>,
}

/// Funding rate event data
///
/// Represents the current funding rate for a perpetual contract
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FundingRateData {
    /// Funding rate as a decimal (eg 0.0001 = 0.01%)
    pub rate: f64,
    /// Exchange-provided timestamp (optional)
    pub time: Option<DateTime<Utc>>,
    /// Next funding time (optional)
    pub next_time: Option<DateTime<Utc>>,
}

/// Traditional markets tick data (ES/NQ) from ibkr-bridge
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TradTickData {
    pub symbol: String,
    pub ts: i64,
    pub px: f64,
    #[serde(default)]
    pub sz: f64,
    #[serde(default)]
    pub bid: Option<f64>,
    #[serde(default)]
    pub ask: Option<f64>,
    #[serde(default)]
    pub vwap: Option<f64>,
}

/// Cumulative Volume Delta (CVD) event data
///
/// Represents the net buying/selling pressure
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CvdData {
    /// Delta in base currency (positive = net buying, negative = net selling)
    pub delta_base: f64,
    /// Delta in quote currency
    pub delta_quote: f64,
}

/// Trade event data
///
/// Represents a single trade execution
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TradeData {
    /// Trade ID from the exchange
    pub id: String,
    /// Execution price
    pub price: f64,
    /// Trade size (in base currency)
    pub amount: f64,
    /// Side of the trade (buyer vs seller initiated)
    pub side: Side,
}

/// Price/quantity level in an order book
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Level {
    /// Price level
    pub price: Decimal,
    /// Quantity at this level
    pub amount: Decimal,
}

impl Level {
    /// Convert price to f64 for calculations
    pub fn price_f64(&self) -> f64 {
        self.price.to_string().parse().unwrap_or(0.0)
    }

    /// Convert amount to f64 for calculations
    pub fn amount_f64(&self) -> f64 {
        self.amount.to_string().parse().unwrap_or(0.0)
    }
}

/// Order Book Level 1 (top of book) event data
///
/// Contains the best bid and ask prices/quantities
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OrderBookL1Data {
    /// Last update time from exchange
    pub last_update_time: DateTime<Utc>,
    /// Best bid (highest buy order)
    pub best_bid: Option<Level>,
    /// Best ask (lowest sell order)
    pub best_ask: Option<Level>,
}

impl OrderBookL1Data {
    /// Calculate the bid-ask spread
    pub fn spread(&self) -> Option<Decimal> {
        match (&self.best_bid, &self.best_ask) {
            (Some(bid), Some(ask)) => Some(ask.price - bid.price),
            _ => None,
        }
    }

    /// Calculate the mid price
    pub fn mid_price(&self) -> Option<Decimal> {
        match (&self.best_bid, &self.best_ask) {
            (Some(bid), Some(ask)) => Some((bid.price + ask.price) / Decimal::from(2)),
            _ => None,
        }
    }

    /// Calculate spread as a percentage of mid price
    pub fn spread_percentage(&self) -> Option<f64> {
        let spread = self.spread()?;
        let mid = self.mid_price()?;

        if mid > Decimal::ZERO {
            let pct = (spread / mid) * Decimal::from(100);
            Some(pct.to_string().parse().unwrap_or(0.0))
        } else {
            None
        }
    }
}

/// Order Book Level 2 (depth) event data
///
/// Contains multiple price levels for both bids and asks.
/// Matches barter-data's OrderBookEvent serialization format.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum OrderBookL2Data {
    /// Full orderbook snapshot
    Snapshot(OrderBook),
    /// Incremental update
    Update(OrderBook),
}

/// Wrapper for order book side with nested levels (matches barter-data format)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OrderBookSide {
    /// The price levels for this side
    pub levels: Vec<Level>,
}

/// Orderbook with bid and ask levels (matches barter-data's OrderBook serialization)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OrderBook {
    /// Sequence number for ordering
    #[serde(default)]
    pub sequence: u64,
    /// Engine timestamp (matches barter-data's time_engine field)
    #[serde(default, alias = "last_update_time")]
    pub time_engine: Option<DateTime<Utc>>,
    /// Bid levels (price, amount) sorted by price descending
    pub bids: OrderBookSide,
    /// Ask levels (price, amount) sorted by price ascending
    pub asks: OrderBookSide,
}

impl OrderBook {
    /// Calculate the book imbalance ratio
    /// Returns value from -1.0 (all asks) to +1.0 (all bids)
    pub fn imbalance(&self, levels: usize) -> f64 {
        let bid_vol: f64 = self.bids.levels.iter().take(levels).map(|l| l.amount_f64()).sum();
        let ask_vol: f64 = self.asks.levels.iter().take(levels).map(|l| l.amount_f64()).sum();
        let total = bid_vol + ask_vol;
        if total > 0.0 {
            (bid_vol - ask_vol) / total
        } else {
            0.0
        }
    }

    /// Calculate the bid imbalance percentage (0-100%)
    pub fn bid_imbalance_pct(&self, levels: usize) -> f64 {
        let bid_vol: f64 = self.bids.levels.iter().take(levels).map(|l| l.amount_f64()).sum();
        let ask_vol: f64 = self.asks.levels.iter().take(levels).map(|l| l.amount_f64()).sum();
        let total = bid_vol + ask_vol;
        if total > 0.0 {
            (bid_vol / total) * 100.0
        } else {
            50.0
        }
    }

    /// Get the best bid level
    pub fn best_bid(&self) -> Option<&Level> {
        self.bids.levels.first()
    }

    /// Get the best ask level
    pub fn best_ask(&self) -> Option<&Level> {
        self.asks.levels.first()
    }

    /// Calculate mid price
    pub fn mid_price(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some((bid.price_f64() + ask.price_f64()) / 2.0),
            _ => None,
        }
    }

    /// Get total bid volume within N levels
    pub fn bid_volume(&self, levels: usize) -> f64 {
        self.bids.levels.iter().take(levels).map(|l| l.amount_f64()).sum()
    }

    /// Get total ask volume within N levels
    pub fn ask_volume(&self, levels: usize) -> f64 {
        self.asks.levels.iter().take(levels).map(|l| l.amount_f64()).sum()
    }
}

impl OrderBookL2Data {
    /// Get the underlying orderbook
    pub fn book(&self) -> &OrderBook {
        match self {
            OrderBookL2Data::Snapshot(book) => book,
            OrderBookL2Data::Update(book) => book,
        }
    }

    /// Check if this is a snapshot
    pub fn is_snapshot(&self) -> bool {
        matches!(self, OrderBookL2Data::Snapshot(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::prelude::FromPrimitive;

    #[test]
    fn test_side_display() {
        assert_eq!(Side::Buy.to_string(), "Buy");
        assert_eq!(Side::Sell.to_string(), "Sell");
    }

    #[test]
    fn test_side_checks() {
        assert!(Side::Buy.is_buy());
        assert!(!Side::Buy.is_sell());
        assert!(Side::Sell.is_sell());
        assert!(!Side::Sell.is_buy());
    }

    #[test]
    fn test_orderbook_calculations() {
        let ob = OrderBookL1Data {
            last_update_time: Utc::now(),
            best_bid: Some(Level {
                price: Decimal::from_f64(100.0).unwrap(),
                amount: Decimal::from_f64(1.5).unwrap(),
            }),
            best_ask: Some(Level {
                price: Decimal::from_f64(100.5).unwrap(),
                amount: Decimal::from_f64(2.0).unwrap(),
            }),
        };

        assert_eq!(ob.spread(), Some(Decimal::from_f64(0.5).unwrap()));
        assert_eq!(ob.mid_price(), Some(Decimal::from_f64(100.25).unwrap()));

        let spread_pct = ob.spread_percentage().unwrap();
        assert!((spread_pct - 0.4987).abs() < 0.001); // ~0.5%
    }
}
