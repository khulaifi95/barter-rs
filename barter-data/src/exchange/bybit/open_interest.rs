use crate::{
    Identifier,
    event::{MarketEvent, MarketIter},
    exchange::bybit::message::BybitPayload,
    subscription::open_interest::OpenInterest,
};
use barter_instrument::exchange::ExchangeId;
use barter_integration::subscription::SubscriptionId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// [`Bybit`](super::Bybit) tickers payload carrying open interest data.
pub type BybitOpenInterest = BybitPayload<BybitOpenInterestInner>;

/// Messages received on the Bybit tickers stream.
#[derive(Clone, Debug)]
pub enum BybitOpenInterestMessage {
    Ignore,
    Payload(BybitOpenInterest),
}

impl<'de> Deserialize<'de> for BybitOpenInterestMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;

        match value.get("topic") {
            Some(topic) if topic.is_string() => {
                let raw = serde_json::to_string(&value).map_err(serde::de::Error::custom)?;
                serde_json::from_str(&raw)
                    .map(BybitOpenInterestMessage::Payload)
                    .map_err(serde::de::Error::custom)
            }
            _ => Ok(BybitOpenInterestMessage::Ignore),
        }
    }
}

/// Subset of [`tickers`](https://bybit-exchange.github.io/docs/v5/websocket/public/tickers) fields
/// required to derive open interest updates.
#[derive(Clone, PartialEq, PartialOrd, Debug, Deserialize, Serialize)]
pub struct BybitOpenInterestInner {
    #[serde(rename = "symbol")]
    pub market: String,

    #[serde(default, alias = "openInterest", deserialize_with = "de_opt_str_f64")]
    pub contracts: Option<f64>,

    #[serde(
        alias = "openInterestValue",
        deserialize_with = "de_opt_str_f64",
        default
    )]
    pub notional: Option<f64>,

    #[serde(
        alias = "ts",
        default,
        deserialize_with = "crate::exchange::bybit::open_interest::de_opt_u64_epoch_ms_as_datetime_utc"
    )]
    pub timestamp: Option<DateTime<Utc>>,
}

impl<InstrumentKey> From<(ExchangeId, InstrumentKey, BybitOpenInterestMessage)>
    for MarketIter<InstrumentKey, OpenInterest>
{
    fn from(
        (exchange, instrument, message): (ExchangeId, InstrumentKey, BybitOpenInterestMessage),
    ) -> Self {
        match message {
            BybitOpenInterestMessage::Ignore => Self(vec![]),
            BybitOpenInterestMessage::Payload(payload) => {
                let time_exchange = payload.time;
                let BybitOpenInterestInner {
                    market: _,
                    contracts,
                    notional,
                    timestamp,
                } = payload.data;

                if let Some(contracts) = contracts {
                    let event_time = timestamp.unwrap_or(time_exchange);

                    Self(vec![Ok(MarketEvent {
                        time_exchange,
                        time_received: Utc::now(),
                        exchange,
                        instrument,
                        kind: OpenInterest {
                            contracts,
                            notional,
                            time: Some(event_time),
                        },
                    })])
                } else {
                    Self(vec![])
                }
            }
        }
    }
}

impl Identifier<Option<SubscriptionId>> for BybitOpenInterestMessage {
    fn id(&self) -> Option<SubscriptionId> {
        match self {
            BybitOpenInterestMessage::Payload(payload) => payload.id(),
            BybitOpenInterestMessage::Ignore => None,
        }
    }
}

fn de_opt_str_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let value: Option<&str> = Option::deserialize(deserializer)?;
    match value {
        Some(raw) if raw.trim().is_empty() => Ok(None),
        Some(raw) => raw
            .parse::<f64>()
            .map(Some)
            .map_err(serde::de::Error::custom),
        None => Ok(None),
    }
}

fn de_opt_u64_epoch_ms_as_datetime_utc<'de, D>(
    deserializer: D,
) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let value: Option<u64> = Option::deserialize(deserializer)?;
    Ok(value.map(|epoch_ms| {
        barter_integration::de::datetime_utc_from_epoch_duration(std::time::Duration::from_millis(
            epoch_ms,
        ))
    }))
}
