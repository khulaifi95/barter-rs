/// Core data types for market events
///
/// These types match the JSON message format from the WebSocket server
/// at ws://127.0.0.1:9001

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

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
            (Some(bid), Some(ask)) => {
                Some((bid.price + ask.price) / Decimal::from(2))
            }
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
