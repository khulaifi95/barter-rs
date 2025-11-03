use crate::{
    Identifier,
    books::{Level, OrderBook},
    event::{MarketEvent, MarketIter},
    subscription::book::OrderBookEvent,
};
use barter_instrument::exchange::ExchangeId;
use barter_integration::subscription::SubscriptionId;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::message::{BybitPayload, BybitPayloadKind};

/// Level 1 OrderBook types.
pub mod l1;

/// Level 2 OrderBook types.
pub mod l2;

/// Terse type alias for an OrderBook WebSocket payload.
pub type BybitOrderBook = BybitPayload<BybitOrderBookInner>;

/// Messages received on the Bybit order book stream.
#[derive(Clone, Debug)]
pub enum BybitOrderBookMessage {
    Ignore,
    Payload(BybitOrderBook),
}

impl<'de> Deserialize<'de> for BybitOrderBookMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;

        match value.get("topic") {
            Some(topic) if topic.is_string() => {
                let raw = serde_json::to_string(&value).map_err(serde::de::Error::custom)?;
                serde_json::from_str(&raw)
                    .map(BybitOrderBookMessage::Payload)
                    .map_err(serde::de::Error::custom)
            }
            _ => Ok(BybitOrderBookMessage::Ignore),
        }
    }
}

impl Identifier<Option<SubscriptionId>> for BybitOrderBookMessage {
    fn id(&self) -> Option<SubscriptionId> {
        match self {
            BybitOrderBookMessage::Payload(payload) => payload.id(),
            BybitOrderBookMessage::Ignore => None,
        }
    }
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct BybitOrderBookInner {
    #[serde(rename = "b")]
    pub bids: Vec<BybitLevel>,

    #[serde(rename = "a")]
    pub asks: Vec<BybitLevel>,

    #[serde(rename = "u")]
    pub update_id: u64,

    #[serde(rename = "seq")]
    pub sequence: u64,
}

impl<InstrumentKey> From<(ExchangeId, InstrumentKey, BybitOrderBookMessage)>
    for MarketIter<InstrumentKey, OrderBookEvent>
{
    fn from(
        (exchange, instrument, message): (ExchangeId, InstrumentKey, BybitOrderBookMessage),
    ) -> Self {
        match message {
            BybitOrderBookMessage::Ignore => Self(vec![]),
            BybitOrderBookMessage::Payload(payload) => {
                let orderbook = OrderBook::new(
                    payload.data.sequence,
                    Some(payload.time),
                    payload.data.bids,
                    payload.data.asks,
                );

                let kind = match payload.kind {
                    BybitPayloadKind::Snapshot => OrderBookEvent::Snapshot(orderbook),
                    BybitPayloadKind::Delta => OrderBookEvent::Update(orderbook),
                };

                Self(vec![Ok(MarketEvent {
                    time_exchange: payload.time,
                    time_received: Utc::now(),
                    exchange,
                    instrument,
                    kind,
                })])
            }
        }
    }
}

/// [`Bybit`](super::Bybit) OrderBook level.
///
/// #### Raw Payload Examples
/// See docs: <https://bybit-exchange.github.io/docs/v5/websocket/public/orderbook#response-parameters>
///
/// ```json
/// ["16493.50", "0.006"]
/// ```
#[derive(Clone, Copy, PartialEq, PartialOrd, Debug, Deserialize, Serialize)]
pub struct BybitLevel {
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub amount: Decimal,
}

impl From<BybitLevel> for Level {
    fn from(level: BybitLevel) -> Self {
        Self {
            price: level.price,
            amount: level.amount,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod de {
        use super::*;
        use rust_decimal_macros::dec;

        #[test]
        fn test_bybit_level() {
            let input = r#"["16493.50", "0.006"]"#;
            assert_eq!(
                serde_json::from_str::<BybitLevel>(input).unwrap(),
                BybitLevel {
                    price: dec!(16493.50),
                    amount: dec!(0.006)
                },
            )
        }
    }
}
