//! OKX OrderBook types and transformers.

use crate::{
    Identifier,
    books::{Level, OrderBook},
    event::{MarketEvent, MarketIter},
    subscription::book::OrderBookEvent,
};
use barter_instrument::exchange::ExchangeId;
use barter_integration::subscription::SubscriptionId;
use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::str::FromStr;

/// Level 2 OrderBook types and transformer.
pub mod l2;

/// OKX OrderBook WebSocket message wrapper.
#[derive(Clone, Debug)]
pub enum OkxOrderBookMessage {
    /// Non-orderbook message (subscription response, etc.)
    Ignore,
    /// OrderBook payload
    Payload(OkxOrderBookPayload),
}

impl<'de> Deserialize<'de> for OkxOrderBookMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;

        // Check if this is an orderbook message (has "arg" with "channel": "books")
        if let Some(arg) = value.get("arg") {
            if let Some(channel) = arg.get("channel") {
                if channel.as_str() == Some("books") {
                    let raw = serde_json::to_string(&value).map_err(serde::de::Error::custom)?;
                    return serde_json::from_str(&raw)
                        .map(OkxOrderBookMessage::Payload)
                        .map_err(serde::de::Error::custom);
                }
            }
        }

        Ok(OkxOrderBookMessage::Ignore)
    }
}

impl Identifier<Option<SubscriptionId>> for OkxOrderBookMessage {
    fn id(&self) -> Option<SubscriptionId> {
        match self {
            OkxOrderBookMessage::Payload(payload) => payload.id(),
            OkxOrderBookMessage::Ignore => None,
        }
    }
}

/// OKX OrderBook payload structure.
///
/// Example:
/// ```json
/// {
///   "arg": { "channel": "books", "instId": "BTC-USDT-SWAP" },
///   "action": "snapshot",
///   "data": [{
///     "asks": [["price", "size", "0", "numOrders"], ...],
///     "bids": [["price", "size", "0", "numOrders"], ...],
///     "ts": "1597026383085",
///     "checksum": -855196043
///   }]
/// }
/// ```
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OkxOrderBookPayload {
    pub arg: OkxOrderBookArg,
    pub action: OkxOrderBookAction,
    pub data: Vec<OkxOrderBookData>,
}

impl Identifier<Option<SubscriptionId>> for OkxOrderBookPayload {
    fn id(&self) -> Option<SubscriptionId> {
        Some(SubscriptionId::from(format!(
            "books|{}",
            self.arg.inst_id
        )))
    }
}

/// OKX OrderBook subscription argument.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OkxOrderBookArg {
    pub channel: String,
    #[serde(rename = "instId")]
    pub inst_id: String,
}

/// OKX OrderBook action type.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OkxOrderBookAction {
    Snapshot,
    Update,
}

/// OKX OrderBook data.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OkxOrderBookData {
    pub asks: Vec<OkxLevel>,
    pub bids: Vec<OkxLevel>,
    /// Timestamp in milliseconds
    #[serde(deserialize_with = "de_timestamp_ms")]
    pub ts: DateTime<Utc>,
    /// Checksum for validation (optional)
    #[serde(default)]
    pub checksum: Option<i64>,
}

/// Deserialize timestamp from string milliseconds to DateTime<Utc>
fn de_timestamp_ms<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    let ms: i64 = s.parse().map_err(serde::de::Error::custom)?;
    Utc.timestamp_millis_opt(ms)
        .single()
        .ok_or_else(|| serde::de::Error::custom("invalid timestamp"))
}

/// OKX OrderBook level: ["price", "size", "deprecated", "numOrders"]
#[derive(Clone, Copy, Debug, Serialize)]
pub struct OkxLevel {
    pub price: Decimal,
    pub amount: Decimal,
    pub num_orders: u32,
}

impl<'de> Deserialize<'de> for OkxLevel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let arr: Vec<String> = Deserialize::deserialize(deserializer)?;
        if arr.len() < 4 {
            return Err(serde::de::Error::custom("expected 4 elements in level array"));
        }

        Ok(OkxLevel {
            price: Decimal::from_str(&arr[0]).map_err(serde::de::Error::custom)?,
            amount: Decimal::from_str(&arr[1]).map_err(serde::de::Error::custom)?,
            num_orders: arr[3].parse().map_err(serde::de::Error::custom)?,
        })
    }
}

impl From<OkxLevel> for Level {
    fn from(level: OkxLevel) -> Self {
        Self {
            price: level.price,
            amount: level.amount,
        }
    }
}

impl<InstrumentKey> From<(ExchangeId, InstrumentKey, OkxOrderBookMessage)>
    for MarketIter<InstrumentKey, OrderBookEvent>
{
    fn from(
        (exchange, instrument, message): (ExchangeId, InstrumentKey, OkxOrderBookMessage),
    ) -> Self {
        match message {
            OkxOrderBookMessage::Ignore => Self(vec![]),
            OkxOrderBookMessage::Payload(payload) => {
                // OKX sends data as array, we take the first element
                let Some(data) = payload.data.into_iter().next() else {
                    return Self(vec![]);
                };

                let orderbook = OrderBook::new(
                    0, // OKX doesn't provide sequence number in the same way
                    Some(data.ts),
                    data.bids,
                    data.asks,
                );

                let kind = match payload.action {
                    OkxOrderBookAction::Snapshot => OrderBookEvent::Snapshot(orderbook),
                    OkxOrderBookAction::Update => OrderBookEvent::Update(orderbook),
                };

                Self(vec![Ok(MarketEvent {
                    time_exchange: data.ts,
                    time_received: Utc::now(),
                    exchange,
                    instrument,
                    kind,
                })])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_okx_level_deserialize() {
        let input = r#"["41006.8", "0.60038921", "0", "1"]"#;
        let level: OkxLevel = serde_json::from_str(input).unwrap();
        assert_eq!(level.price, dec!(41006.8));
        assert_eq!(level.amount, dec!(0.60038921));
        assert_eq!(level.num_orders, 1);
    }

    #[test]
    fn test_okx_orderbook_payload_deserialize() {
        let input = r#"{
            "arg": { "channel": "books", "instId": "BTC-USDT-SWAP" },
            "action": "snapshot",
            "data": [{
                "asks": [["41006.8", "0.60038921", "0", "1"]],
                "bids": [["41006.7", "0.30178218", "0", "2"]],
                "ts": "1629966436396",
                "checksum": -855196043
            }]
        }"#;

        let payload: OkxOrderBookPayload = serde_json::from_str(input).unwrap();
        assert_eq!(payload.arg.inst_id, "BTC-USDT-SWAP");
        assert_eq!(payload.action, OkxOrderBookAction::Snapshot);
        assert_eq!(payload.data.len(), 1);
        assert_eq!(payload.data[0].asks.len(), 1);
        assert_eq!(payload.data[0].bids.len(), 1);
    }
}
