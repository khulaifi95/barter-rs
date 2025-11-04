use crate::{
    Identifier,
    event::{MarketEvent, MarketIter},
    exchange::bybit::message::BybitPayload,
    subscription::liquidation::Liquidation,
};
use barter_instrument::{Side, exchange::ExchangeId};
use barter_integration::subscription::SubscriptionId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// [`Bybit`](super::Bybit) All Liquidation payload.
pub type BybitAllLiquidation = BybitPayload<Vec<BybitAllLiquidationInner>>;

/// Messages received on the Bybit all liquidation stream.
#[derive(Clone, Debug)]
pub enum BybitLiquidationMessage {
    Ignore,
    Payload(BybitAllLiquidation),
}

impl<'de> Deserialize<'de> for BybitLiquidationMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;

        match value.get("topic") {
            Some(topic) if topic.is_string() => {
                let raw = serde_json::to_string(&value).map_err(serde::de::Error::custom)?;
                serde_json::from_str(&raw)
                    .map(BybitLiquidationMessage::Payload)
                    .map_err(serde::de::Error::custom)
            }
            _ => Ok(BybitLiquidationMessage::Ignore),
        }
    }
}

/// Individual liquidation entry included within an [`BybitAllLiquidation`] payload.
#[derive(Clone, PartialEq, PartialOrd, Debug, Deserialize, Serialize)]
pub struct BybitAllLiquidationInner {
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
    pub quantity: f64,

    #[serde(alias = "p", deserialize_with = "barter_integration::de::de_str")]
    pub price: f64,
}

impl<InstrumentKey: Clone> From<(ExchangeId, InstrumentKey, BybitLiquidationMessage)>
    for MarketIter<InstrumentKey, Liquidation>
{
    fn from(
        (exchange, instrument, message): (ExchangeId, InstrumentKey, BybitLiquidationMessage),
    ) -> Self {
        match message {
            BybitLiquidationMessage::Ignore => Self(vec![]),
            BybitLiquidationMessage::Payload(payload) => Self(
                payload
                    .data
                    .into_iter()
                    .map(|entry| {
                        Ok(MarketEvent {
                            time_exchange: entry.time,
                            time_received: Utc::now(),
                            exchange,
                            instrument: instrument.clone(),
                            kind: Liquidation {
                                side: entry.side,
                                price: entry.price,
                                quantity: entry.quantity,
                                time: entry.time,
                            },
                        })
                    })
                    .collect(),
            ),
        }
    }
}

impl Identifier<Option<SubscriptionId>> for BybitLiquidationMessage {
    fn id(&self) -> Option<SubscriptionId> {
        match self {
            BybitLiquidationMessage::Payload(payload) => payload.id(),
            BybitLiquidationMessage::Ignore => None,
        }
    }
}
