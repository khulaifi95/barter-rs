use crate::{
    Identifier,
    event::{MarketEvent, MarketIter},
    exchange::ExchangeSub,
    subscription::liquidation::Liquidation,
};
use barter_instrument::{Side, exchange::ExchangeId};
use barter_integration::subscription::SubscriptionId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Terse type alias for an [`Okx`](super::Okx) liquidation orders WebSocket message.
pub type OkxLiquidations = OkxLiquidationMessage<OkxLiquidation>;

/// [`Okx`](super::Okx) liquidation WebSocket message.
///
/// See docs: <https://www.okx.com/docs-v5/en/#websocket-api-public-channel-liquidation-orders-channel>
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Deserialize, Serialize)]
pub struct OkxLiquidationMessage<T> {
    #[serde(
        rename = "arg",
        deserialize_with = "de_okx_liquidation_arg_as_subscription_id"
    )]
    pub subscription_id: SubscriptionId,
    pub data: Vec<T>,
}

impl<T> Identifier<Option<SubscriptionId>> for OkxLiquidationMessage<T> {
    fn id(&self) -> Option<SubscriptionId> {
        Some(self.subscription_id.clone())
    }
}

/// [`Okx`](super::Okx) liquidation order.
///
/// See docs: <https://www.okx.com/docs-v5/en/#websocket-api-public-channel-liquidation-orders-channel>
#[derive(Clone, PartialEq, PartialOrd, Debug, Deserialize, Serialize)]
pub struct OkxLiquidation {
    #[serde(rename = "instId")]
    pub inst_id: String,
    #[serde(rename = "instFamily")]
    pub inst_family: String,
    #[serde(rename = "details")]
    pub details: Vec<OkxLiquidationDetail>,
}

#[derive(Clone, PartialEq, PartialOrd, Debug, Deserialize, Serialize)]
pub struct OkxLiquidationDetail {
    #[serde(rename = "side")]
    pub side: Side,
    #[serde(rename = "bkPx", deserialize_with = "barter_integration::de::de_str")]
    pub price: f64,
    #[serde(rename = "sz", deserialize_with = "barter_integration::de::de_str")]
    pub size: f64,
    #[serde(rename = "bkLoss", deserialize_with = "barter_integration::de::de_str")]
    pub loss: f64,
    #[serde(
        rename = "ts",
        deserialize_with = "barter_integration::de::de_str_u64_epoch_ms_as_datetime_utc"
    )]
    pub time: DateTime<Utc>,
}

impl<InstrumentKey: Clone> From<(ExchangeId, InstrumentKey, OkxLiquidations)>
    for MarketIter<InstrumentKey, Liquidation>
{
    fn from(
        (exchange, instrument, liquidations): (ExchangeId, InstrumentKey, OkxLiquidations),
    ) -> Self {
        liquidations
            .data
            .into_iter()
            .flat_map(|liq| {
                let instrument = instrument.clone();
                liq.details.into_iter().map(move |detail| {
                    Ok(MarketEvent {
                        time_exchange: detail.time,
                        time_received: Utc::now(),
                        exchange,
                        instrument: instrument.clone(),
                        kind: Liquidation {
                            side: detail.side,
                            price: detail.price,
                            quantity: detail.size,
                            time: detail.time,
                        },
                    })
                })
            })
            .collect()
    }
}

/// Deserialize an [`OkxLiquidationMessage`] "arg" field as a Barter [`SubscriptionId`].
fn de_okx_liquidation_arg_as_subscription_id<'de, D>(
    deserializer: D,
) -> Result<SubscriptionId, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Arg<'a> {
        channel: &'a str,
        inst_type: &'a str,
        #[serde(default)]
        uly: Option<&'a str>,
    }

    Deserialize::deserialize(deserializer).map(|arg: Arg<'_>| {
        // For liquidation-orders, the subscription ID format is different
        // It uses inst_type (e.g., "SWAP") instead of specific instId
        if let Some(uly) = arg.uly {
            ExchangeSub::from((arg.channel, &format!("{}-{}", arg.inst_type, uly))).id()
        } else {
            ExchangeSub::from((arg.channel, arg.inst_type)).id()
        }
    })
}
