use crate::{
    Identifier,
    event::{MarketEvent, MarketIter},
    exchange::ExchangeSub,
    subscription::trade::PublicTrade,
};
use super::ctval;
use barter_instrument::{Side, exchange::ExchangeId};
use barter_integration::subscription::SubscriptionId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Terse type alias for an [`Okx`](super::Okx) real-time trades WebSocket message.
pub type OkxTrades = OkxMessage<OkxTrade>;

/// [`Okx`](super::Okx) market data WebSocket message.
///
/// ### Raw Payload Examples
/// See docs: <https://www.okx.com/docs-v5/en/#websocket-api-public-channel>
/// #### Spot Buy Trade
/// ```json
/// {
///   "arg": {
///     "channel": "trades",
///     "instId": "BTC-USDT"
///   },
///   "data": [
///     {
///       "instId": "BTC-USDT",
///       "tradeId": "130639474",
///       "px": "42219.9",
///       "sz": "0.12060306",
///       "side": "buy",
///       "ts": "1630048897897"
///     }
///   ]
/// }
/// ```
///
/// #### Option Call Sell Trade
/// ```json
/// {
///   "arg": {
///     "channel": "trades",
///     "instId": "BTC-USD-231229-35000-C"
///   },
///   "data": [
///     {
///       "instId": "BTC-USD-231229-35000-C",
///       "tradeId": "4",
///       "px": "0.1525",
///       "sz": "21",
///       "side": "sell",
///       "ts": "1681473269025"
///     }
///   ]
/// }
/// ```
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Deserialize, Serialize)]
pub struct OkxMessage<T> {
    #[serde(rename = "arg")]
    pub arg: OkxMessageArg,
    pub data: Vec<T>,
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OkxMessageArg {
    pub channel: String,
    #[serde(rename = "instId")]
    pub inst_id: String,
}

impl<T> Identifier<Option<SubscriptionId>> for OkxMessage<T> {
    fn id(&self) -> Option<SubscriptionId> {
        Some(ExchangeSub::from((self.arg.channel.as_str(), self.arg.inst_id.as_str())).id())
    }
}

/// [`Okx`](super::Okx) real-time trade WebSocket message.
///
/// See [`OkxMessage`] for full raw payload examples.
///
/// See docs: <https://www.okx.com/docs-v5/en/#websocket-api-public-channel-trades-channel>
#[derive(Clone, PartialEq, PartialOrd, Debug, Deserialize, Serialize)]
pub struct OkxTrade {
    #[serde(rename = "tradeId")]
    pub id: String,
    #[serde(rename = "px", deserialize_with = "barter_integration::de::de_str")]
    pub price: f64,
    #[serde(rename = "sz", deserialize_with = "barter_integration::de::de_str")]
    pub amount: f64,
    pub side: Side,
    #[serde(
        rename = "ts",
        deserialize_with = "barter_integration::de::de_str_u64_epoch_ms_as_datetime_utc"
    )]
    pub time: DateTime<Utc>,
}

impl<InstrumentKey: Clone> From<(ExchangeId, InstrumentKey, OkxTrades)>
    for MarketIter<InstrumentKey, PublicTrade>
{
    fn from((exchange, instrument, trades): (ExchangeId, InstrumentKey, OkxTrades)) -> Self {
        tracing::debug!(trade_count = trades.data.len(), "OKX trades batch received");
        let multiplier = ctval::ctval_multiplier_f64(&trades.arg.inst_id);
        trades
            .data
            .into_iter()
            .map(|trade| {
                tracing::debug!(
                    id = %trade.id,
                    price = trade.price,
                    amount = trade.amount,
                    side = ?trade.side,
                    "OKX trade received"
                );
                Ok(MarketEvent {
                    time_exchange: trade.time,
                    time_received: Utc::now(),
                    exchange,
                    instrument: instrument.clone(),
                    kind: PublicTrade {
                        id: trade.id,
                        price: trade.price,
                        amount: trade.amount * multiplier,
                        side: trade.side,
                    },
                })
            })
            .collect()
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    mod de {
        use super::*;
        use barter_integration::{de::datetime_utc_from_epoch_duration, error::SocketError};
        use std::time::Duration;

        #[test]
        fn test_okx_message_trades() {
            let input = r#"
            {
                "arg": {
                    "channel": "trades",
                    "instId": "BTC-USDT"
                },
                "data": [
                    {
                        "instId": "BTC-USDT",
                        "tradeId": "130639474",
                        "px": "42219.9",
                        "sz": "0.12060306",
                        "side": "buy",
                        "ts": "1630048897897"
                    }
                ]
            }
            "#;

            let actual = serde_json::from_str::<OkxTrades>(input);
            let expected: Result<OkxTrades, SocketError> = Ok(OkxTrades {
                arg: OkxMessageArg {
                    channel: "trades".to_string(),
                    inst_id: "BTC-USDT".to_string(),
                },
                data: vec![OkxTrade {
                    id: "130639474".to_string(),
                    price: 42219.9,
                    amount: 0.12060306,
                    side: Side::Buy,
                    time: datetime_utc_from_epoch_duration(Duration::from_millis(1630048897897)),
                }],
            });

            match (actual, expected) {
                (Ok(actual), Ok(expected)) => {
                    assert_eq!(actual, expected, "TC failed")
                }
                (Err(_), Err(_)) => {
                    // Test passed
                }
                (actual, expected) => {
                    // Test failed
                    panic!(
                        "TC failed because actual != expected. \nActual: {actual:?}\nExpected: {expected:?}\n"
                    );
                }
            }
        }
    }
}
