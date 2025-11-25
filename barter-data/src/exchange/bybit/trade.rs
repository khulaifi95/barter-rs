use crate::{
    Identifier,
    event::{MarketEvent, MarketIter},
    exchange::bybit::message::BybitPayload,
    subscription::trade::PublicTrade,
};
use barter_instrument::{Side, exchange::ExchangeId};
use barter_integration::subscription::SubscriptionId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Terse type alias for an [`BybitTrade`](BybitTradeInner) real-time trades WebSocket message.
pub type BybitTrade = BybitPayload<Vec<BybitTradeInner>>;

/// Messages received on the Bybit trade stream.
#[derive(Clone, Debug)]
pub enum BybitTradeMessage {
    Ignore,
    Payload(BybitTrade),
}

impl<'de> Deserialize<'de> for BybitTradeMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;

        // DIAGNOSTIC LOGGING - Check if this is a trade-related message
        if value.get("data").is_some() || value.get("topic").and_then(|t| t.as_str()).map(|s| s.contains("publicTrade")).unwrap_or(false) {
            let has_topic = value.get("topic").is_some();
            let topic_value = value.get("topic").and_then(|t| t.as_str()).unwrap_or("NONE");

            eprintln!("[BYBIT TRADE DEBUG] Message received:");
            eprintln!("[BYBIT TRADE DEBUG]   - has 'topic' field: {}", has_topic);
            eprintln!("[BYBIT TRADE DEBUG]   - topic value: {}", topic_value);
            eprintln!("[BYBIT TRADE DEBUG]   - has 'data' field: {}", value.get("data").is_some());

            if !has_topic {
                eprintln!("[BYBIT TRADE DEBUG] ⚠️  MESSAGE WITHOUT TOPIC - WILL BE DROPPED!");
                eprintln!("[BYBIT TRADE DEBUG] Full message: {}", serde_json::to_string_pretty(&value).unwrap_or_default());
            }
        }

        match value.get("topic") {
            Some(topic) if topic.is_string() => {
                eprintln!("[BYBIT TRADE DEBUG] ✓ Processing message with topic");
                let raw = serde_json::to_string(&value).map_err(serde::de::Error::custom)?;
                match serde_json::from_str::<BybitPayload<Vec<BybitTradeInner>>>(&raw) {
                    Ok(payload) => {
                        eprintln!("[BYBIT TRADE DEBUG] ✓ Successfully deserialized {} trades", payload.data.len());
                        Ok(BybitTradeMessage::Payload(payload))
                    }
                    Err(e) => {
                        eprintln!("[BYBIT TRADE DEBUG] ✗ Deserialization failed: {}", e);
                        Err(serde::de::Error::custom(e))
                    }
                }
            }
            _ => {
                eprintln!("[BYBIT TRADE DEBUG] → Ignoring message (no topic or non-string topic)");
                Ok(BybitTradeMessage::Ignore)
            }
        }
    }
}

/// ### Raw Payload Examples
/// See docs: <https://bybit-exchange.github.io/docs/v5/websocket/public/trade>
/// Spot Side::Buy Trade
///```json
/// {
///     "T": 1672304486865,
///     "s": "BTCUSDT",
///     "S": "Buy",
///     "v": "0.001",
///     "p": "16578.50",
///     "L": "PlusTick",
///     "i": "20f43950-d8dd-5b31-9112-a178eb6023af",
///     "BT": false
/// }
/// ```
#[derive(Clone, PartialEq, PartialOrd, Debug, Deserialize, Serialize)]
pub struct BybitTradeInner {
    #[serde(
        alias = "T",
        deserialize_with = "barter_integration::de::de_u64_epoch_ms_as_datetime_utc"
    )]
    pub time: DateTime<Utc>,

    #[serde(rename = "s")]
    pub market: String,

    #[serde(rename = "S")]
    pub side: Side,

    #[serde(alias = "v", deserialize_with = "barter_integration::de::de_str")]
    pub amount: f64,

    #[serde(alias = "p", deserialize_with = "barter_integration::de::de_str")]
    pub price: f64,

    #[serde(rename = "i")]
    pub id: String,
}

impl<InstrumentKey: Clone> From<(ExchangeId, InstrumentKey, BybitTradeMessage)>
    for MarketIter<InstrumentKey, PublicTrade>
{
    fn from(
        (exchange, instrument, message): (ExchangeId, InstrumentKey, BybitTradeMessage),
    ) -> Self {
        match message {
            BybitTradeMessage::Ignore => {
                eprintln!("[BYBIT TRADE DEBUG] Converting Ignore to empty vec - trade(s) lost");
                Self(vec![])
            }
            BybitTradeMessage::Payload(trades) => {
                eprintln!("[BYBIT TRADE DEBUG] Converting {} Bybit trades to MarketEvents", trades.data.len());
                Self(
                    trades
                        .data
                        .into_iter()
                        .map(|trade| {
                            eprintln!("[BYBIT TRADE DEBUG]   Trade: {} @ {} qty {} side {:?}",
                                trade.market, trade.price, trade.amount, trade.side);
                            Ok(MarketEvent {
                                time_exchange: trade.time,
                                time_received: Utc::now(),
                                exchange,
                                instrument: instrument.clone(),
                                kind: PublicTrade {
                                    id: trade.id,
                                    price: trade.price,
                                    amount: trade.amount,
                                    side: trade.side,
                                },
                            })
                        })
                        .collect(),
                )
            }
        }
    }
}

impl Identifier<Option<SubscriptionId>> for BybitTradeMessage {
    fn id(&self) -> Option<SubscriptionId> {
        match self {
            BybitTradeMessage::Payload(payload) => payload.id(),
            BybitTradeMessage::Ignore => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod de {
        use crate::exchange::bybit::message::BybitPayloadKind;

        use super::*;
        use barter_integration::{
            de::datetime_utc_from_epoch_duration, error::SocketError, subscription::SubscriptionId,
        };
        use smol_str::ToSmolStr;
        use std::time::Duration;

        #[test]
        fn test_bybit_trade() {
            struct TestCase {
                input: &'static str,
                expected: Result<BybitTradeInner, SocketError>,
            }

            let tests = vec![
                // TC0: input BybitTradeInner is deserialised
                TestCase {
                    input: r#"
                        {
                            "T": 1672304486865,
                            "s": "BTCUSDT",
                            "S": "Buy",
                            "v": "0.001",
                            "p": "16578.50",
                            "L": "PlusTick",
                            "i": "20f43950-d8dd-5b31-9112-a178eb6023af",
                            "BT": false
                        }
                    "#,
                    expected: Ok(BybitTradeInner {
                        time: datetime_utc_from_epoch_duration(Duration::from_millis(
                            1672304486865,
                        )),
                        market: "BTCUSDT".to_string(),
                        side: Side::Buy,
                        amount: 0.001,
                        price: 16578.50,
                        id: "20f43950-d8dd-5b31-9112-a178eb6023af".to_string(),
                    }),
                },
                // TC1: input BybitTradeInner is deserialised
                TestCase {
                    input: r#"
                        {
                            "T": 1672304486865,
                            "s": "BTCUSDT",
                            "S": "Sell",
                            "v": "0.001",
                            "p": "16578.50",
                            "L": "PlusTick",
                            "i": "20f43950-d8dd-5b31-9112-a178eb6023af",
                            "BT": false
                        }
                    "#,
                    expected: Ok(BybitTradeInner {
                        time: datetime_utc_from_epoch_duration(Duration::from_millis(
                            1672304486865,
                        )),
                        market: "BTCUSDT".to_string(),
                        side: Side::Sell,
                        amount: 0.001,
                        price: 16578.50,
                        id: "20f43950-d8dd-5b31-9112-a178eb6023af".to_string(),
                    }),
                },
                // TC2: input BybitTradeInner is unable to be deserialised
                TestCase {
                    input: r#"
                        {
                            "T": 1672304486865,
                            "s": "BTCUSDT",
                            "S": "Unknown",
                            "v": "0.001",
                            "p": "16578.50",
                            "L": "PlusTick",
                            "i": "20f43950-d8dd-5b31-9112-a178eb6023af",
                            "BT": false
                        }
                    "#,
                    expected: Err(SocketError::Unsupported {
                        entity: "".to_string(),
                        item: "".to_string(),
                    }),
                },
            ];

            for (index, test) in tests.into_iter().enumerate() {
                let actual = serde_json::from_str::<BybitTradeInner>(test.input);
                match (actual, test.expected) {
                    (Ok(actual), Ok(expected)) => {
                        assert_eq!(actual, expected, "TC{} failed", index)
                    }
                    (Err(_), Err(_)) => {
                        // Test passed
                    }
                    (actual, expected) => {
                        // Test failed
                        panic!(
                            "TC{index} failed because actual != expected. \nActual: {actual:?}\nExpected: {expected:?}\n"
                        );
                    }
                }
            }
        }

        #[test]
        fn test_bybit_trade_payload() {
            struct TestCase {
                input: &'static str,
                expected: Result<BybitTrade, SocketError>,
            }

            let tests = vec![
                // TC0: input BybitTrade is deserialised
                TestCase {
                    input: r#"
                        {
                        "topic": "publicTrade.BTCUSDT",
                        "type": "snapshot",
                        "ts": 1672304486868,
                            "data": [
                                {
                                    "T": 1672304486865,
                                    "s": "BTCUSDT",
                                    "S": "Buy",
                                    "v": "0.001",
                                    "p": "16578.50",
                                    "L": "PlusTick",
                                    "i": "20f43950-d8dd-5b31-9112-a178eb6023af",
                                    "BT": false
                                },
                                {
                                    "T": 1672304486865,
                                    "s": "BTCUSDT",
                                    "S": "Sell",
                                    "v": "0.001",
                                    "p": "16578.50",
                                    "L": "PlusTick",
                                    "i": "20f43950-d8dd-5b31-9112-a178eb6023af",
                                    "BT": false
                                }
                            ]
                        }
                    "#,
                    expected: Ok(BybitTrade {
                        subscription_id: SubscriptionId("publicTrade|BTCUSDT".to_smolstr()),
                        kind: BybitPayloadKind::Snapshot,
                        time: datetime_utc_from_epoch_duration(Duration::from_millis(
                            1672304486868,
                        )),
                        data: vec![
                            BybitTradeInner {
                                time: datetime_utc_from_epoch_duration(Duration::from_millis(
                                    1672304486865,
                                )),
                                market: "BTCUSDT".to_string(),
                                side: Side::Buy,
                                amount: 0.001,
                                price: 16578.50,
                                id: "20f43950-d8dd-5b31-9112-a178eb6023af".to_string(),
                            },
                            BybitTradeInner {
                                time: datetime_utc_from_epoch_duration(Duration::from_millis(
                                    1672304486865,
                                )),
                                market: "BTCUSDT".to_string(),
                                side: Side::Sell,
                                amount: 0.001,
                                price: 16578.50,
                                id: "20f43950-d8dd-5b31-9112-a178eb6023af".to_string(),
                            },
                        ],
                    }),
                },
                // TC1: input BybitTrade is invalid w/ no subscription_id
                TestCase {
                    input: r#"
                        {
                            "data": [
                                {
                                    "T": 1672304486865,
                                    "s": "BTCUSDT",
                                    "S": "Unknown",
                                    "v": "0.001",
                                    "p": "16578.50",
                                    "L": "PlusTick",
                                    "i": "20f43950-d8dd-5b31-9112-a178eb6023af",
                                    "BT": false
                                }
                            ]
                        }
                    "#,
                    expected: Err(SocketError::Unsupported {
                        entity: "".to_string(),
                        item: "".to_string(),
                    }),
                },
                // TC1: input BybitTrade is invalid w/ invalid subscription_id format
                TestCase {
                    input: r#"
                        {
                        "topic": "publicTrade.BTCUSDT.should_not_be_present",
                        "type": "snapshot",
                        "ts": 1672304486868,
                            "data": [
                                {
                                    "T": 1672304486865,
                                    "s": "BTCUSDT",
                                    "S": "Buy",
                                    "v": "0.001",
                                    "p": "16578.50",
                                    "L": "PlusTick",
                                    "i": "20f43950-d8dd-5b31-9112-a178eb6023af",
                                    "BT": false
                                },
                                {
                                    "T": 1672304486865,
                                    "s": "BTCUSDT",
                                    "S": "Sell",
                                    "v": "0.001",
                                    "p": "16578.50",
                                    "L": "PlusTick",
                                    "i": "20f43950-d8dd-5b31-9112-a178eb6023af",
                                    "BT": false
                                }
                            ]
                        }
                    "#,
                    expected: Err(SocketError::Unsupported {
                        entity: "".to_string(),
                        item: "".to_string(),
                    }),
                },
            ];

            for (index, test) in tests.into_iter().enumerate() {
                let actual = serde_json::from_str::<BybitTrade>(test.input);
                match (actual, test.expected) {
                    (Ok(actual), Ok(expected)) => {
                        assert_eq!(actual, expected, "TC{} failed", index)
                    }
                    (Err(_), Err(_)) => {
                        // Test passed
                    }
                    (actual, expected) => {
                        // Test failed
                        panic!(
                            "TC{index} failed because actual != expected. \nActual: {actual:?}\nExpected: {expected:?}\n"
                        );
                    }
                }
            }
        }
    }
}
