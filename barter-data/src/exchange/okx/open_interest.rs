use crate::{
    Identifier,
    event::{MarketEvent, MarketIter},
    exchange::ExchangeSub,
    subscription::open_interest::OpenInterest,
};
use barter_instrument::exchange::ExchangeId;
use barter_integration::subscription::SubscriptionId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Terse type alias for an [`Okx`](super::Okx) open interest WebSocket message.
pub type OkxOpenInterests = OkxOpenInterestMessage<OkxOpenInterest>;

/// [`Okx`](super::Okx) open interest WebSocket message.
///
/// See docs: <https://www.okx.com/docs-v5/en/#websocket-api-public-channel-open-interest-channel>
#[derive(Clone, PartialEq, PartialOrd, Debug, Deserialize, Serialize)]
pub struct OkxOpenInterestMessage<T> {
    #[serde(
        rename = "arg",
        deserialize_with = "de_okx_open_interest_arg_as_subscription_id"
    )]
    pub subscription_id: SubscriptionId,
    pub data: Vec<T>,
}

impl<T> Identifier<Option<SubscriptionId>> for OkxOpenInterestMessage<T> {
    fn id(&self) -> Option<SubscriptionId> {
        Some(self.subscription_id.clone())
    }
}

/// [`Okx`](super::Okx) open interest data.
///
/// See docs: <https://www.okx.com/docs-v5/en/#websocket-api-public-channel-open-interest-channel>
#[derive(Clone, PartialEq, PartialOrd, Debug, Deserialize, Serialize)]
pub struct OkxOpenInterest {
    #[serde(rename = "instType")]
    pub inst_type: String,
    #[serde(rename = "instId")]
    pub inst_id: String,
    #[serde(rename = "oi", deserialize_with = "barter_integration::de::de_str")]
    pub contracts: f64,
    #[serde(
        rename = "oiCcy",
        deserialize_with = "de_opt_str_f64",
        default
    )]
    pub notional_ccy: Option<f64>,
    #[serde(
        rename = "oiUsd",
        deserialize_with = "de_opt_str_f64",
        default
    )]
    pub notional_usd: Option<f64>,
    #[serde(
        rename = "ts",
        deserialize_with = "barter_integration::de::de_str_u64_epoch_ms_as_datetime_utc"
    )]
    pub time: DateTime<Utc>,
}

impl<InstrumentKey: Clone> From<(ExchangeId, InstrumentKey, OkxOpenInterests)>
    for MarketIter<InstrumentKey, OpenInterest>
{
    fn from(
        (exchange, instrument, open_interests): (ExchangeId, InstrumentKey, OkxOpenInterests),
    ) -> Self {
        open_interests
            .data
            .into_iter()
            .map(|oi| {
                Ok(MarketEvent {
                    time_exchange: oi.time,
                    time_received: Utc::now(),
                    exchange,
                    instrument: instrument.clone(),
                    kind: OpenInterest {
                        contracts: oi.contracts,
                        // Prefer USD notional if available, otherwise use currency notional
                        notional: oi.notional_usd.or(oi.notional_ccy),
                        time: Some(oi.time),
                    },
                })
            })
            .collect()
    }
}

/// Deserialize an [`OkxOpenInterestMessage`] "arg" field as a Barter [`SubscriptionId`].
fn de_okx_open_interest_arg_as_subscription_id<'de, D>(
    deserializer: D,
) -> Result<SubscriptionId, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Arg<'a> {
        channel: &'a str,
        inst_id: &'a str,
    }

    Deserialize::deserialize(deserializer)
        .map(|arg: Arg<'_>| ExchangeSub::from((arg.channel, arg.inst_id)).id())
}

/// Deserialize an optional string as an optional f64.
fn de_opt_str_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let value: Option<String> = Option::deserialize(deserializer)?;
    match value {
        Some(raw) if raw.trim().is_empty() => Ok(None),
        Some(raw) => raw
            .parse::<f64>()
            .map(Some)
            .map_err(serde::de::Error::custom),
        None => Ok(None),
    }
}
