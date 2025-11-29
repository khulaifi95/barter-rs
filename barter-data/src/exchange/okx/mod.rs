use self::{
    book::l2::OkxOrderBooksL2Transformer,
    channel::OkxChannel,
    liquidation::OkxLiquidations,
    market::OkxMarket,
    open_interest::OkxOpenInterests,
    subscription::OkxSubResponse,
    trade::OkxTrades,
};
use crate::{
    ExchangeWsStream, NoInitialSnapshots,
    exchange::{Connector, ExchangeSub, PingInterval, StreamSelector},
    instrument::InstrumentData,
    subscriber::{WebSocketSubscriber, validator::WebSocketSubValidator},
    subscription::{
        book::OrderBooksL2,
        cvd::CumulativeVolumeDeltas,
        liquidation::Liquidations,
        open_interest::OpenInterests,
        trade::PublicTrades,
    },
    transformer::{cvd::CumulativeVolumeDeltaTransformer, stateless::StatelessTransformer},
};
use barter_instrument::exchange::ExchangeId;
use barter_integration::{
    error::SocketError,
    protocol::websocket::{WebSocketSerdeParser, WsMessage},
};
use barter_macro::{DeExchange, SerExchange};
use derive_more::Display;
use serde_json::json;
use std::{hash::Hash, time::Duration};
use url::Url;

/// OrderBook types for [`Okx`].
pub mod book;

/// Defines the type that translates a Barter [`Subscription`](crate::subscription::Subscription)
/// into an exchange [`Connector`] specific channel used for generating [`Connector::requests`].
pub mod channel;

/// Liquidation types for [`Okx`].
pub mod liquidation;

/// Defines the type that translates a Barter [`Subscription`](crate::subscription::Subscription)
/// into an exchange [`Connector`] specific market used for generating [`Connector::requests`].
pub mod market;

/// Open interest types for [`Okx`].
pub mod open_interest;

/// [`Subscription`](crate::subscription::Subscription) response type and response
/// [`Validator`](barter_integration::Validator) for [`Okx`].
pub mod subscription;

/// Public trade types for [`Okx`].
pub mod trade;

/// [`Okx`] server base url.
///
/// See docs: <https://www.okx.com/docs-v5/en/#overview-api-resources-and-support>
pub const BASE_URL_OKX: &str = "wss://ws.okx.com:8443/ws/v5/public";

/// [`Okx`] server [`PingInterval`] duration.
///
/// See docs: <https://www.okx.com/docs-v5/en/#websocket-api-connect>
pub const PING_INTERVAL_OKX: Duration = Duration::from_secs(29);

/// Convenient type alias for an Okx [`ExchangeWsStream`] using [`WebSocketSerdeParser`](barter_integration::protocol::websocket::WebSocketSerdeParser).
pub type OkxWsStream<Transformer> = ExchangeWsStream<WebSocketSerdeParser, Transformer>;

/// [`Okx`] exchange.
///
/// See docs: <https://www.okx.com/docs-v5/en/#websocket-api>
#[derive(
    Copy,
    Clone,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    Debug,
    Default,
    Display,
    DeExchange,
    SerExchange,
)]
pub struct Okx;

impl Connector for Okx {
    const ID: ExchangeId = ExchangeId::Okx;
    type Channel = OkxChannel;
    type Market = OkxMarket;
    type Subscriber = WebSocketSubscriber;
    type SubValidator = WebSocketSubValidator;
    type SubResponse = OkxSubResponse;

    fn url() -> Result<Url, SocketError> {
        Url::parse(BASE_URL_OKX).map_err(SocketError::UrlParse)
    }

    fn ping_interval() -> Option<PingInterval> {
        Some(PingInterval {
            interval: tokio::time::interval(PING_INTERVAL_OKX),
            ping: || WsMessage::text("ping"),
        })
    }

    fn requests(exchange_subs: Vec<ExchangeSub<Self::Channel, Self::Market>>) -> Vec<WsMessage> {
        let args: Vec<serde_json::Value> = exchange_subs
            .into_iter()
            .map(|sub| {
                let channel = sub.channel.as_ref();
                let market = sub.market.as_ref();

                if channel == "liquidation-orders" {
                    // Liquidation channel expects derivatives metadata rather than plain instId
                    let parts: Vec<&str> = market.split('-').collect();

                    let inst_type = match parts.last().copied().unwrap_or_default() {
                        last if last.eq_ignore_ascii_case("swap") => "SWAP",
                        last if last.eq_ignore_ascii_case("c")
                            || last.eq_ignore_ascii_case("p") =>
                        {
                            "OPTION"
                        }
                        _ => "FUTURES",
                    };

                    let uly = if parts.len() >= 2 {
                        Some(format!("{}-{}", parts[0], parts[1]))
                    } else {
                        None
                    };

                    let mut arg = json!({
                        "channel": channel,
                        "instType": inst_type,
                        "instId": market,
                    });

                    if let Some(uly) = uly {
                        if let serde_json::Value::Object(ref mut map) = arg {
                            map.insert("uly".into(), serde_json::Value::String(uly));
                        }
                    }

                    arg
                } else {
                    // For other channels, use normal instId format
                    json!({
                        "channel": channel,
                        "instId": market
                    })
                }
            })
            .collect();

        vec![WsMessage::text(
            json!({
                "op": "subscribe",
                "args": args,
            })
            .to_string(),
        )]
    }
}

impl<Instrument> StreamSelector<Instrument, PublicTrades> for Okx
where
    Instrument: InstrumentData,
{
    type SnapFetcher = NoInitialSnapshots;
    type Stream = OkxWsStream<StatelessTransformer<Self, Instrument::Key, PublicTrades, OkxTrades>>;
}

impl<Instrument> StreamSelector<Instrument, Liquidations> for Okx
where
    Instrument: InstrumentData,
{
    type SnapFetcher = NoInitialSnapshots;
    type Stream =
        OkxWsStream<StatelessTransformer<Self, Instrument::Key, Liquidations, OkxLiquidations>>;
}

impl<Instrument> StreamSelector<Instrument, CumulativeVolumeDeltas> for Okx
where
    Instrument: InstrumentData,
    Instrument::Key: Eq + Hash,
{
    type SnapFetcher = NoInitialSnapshots;
    type Stream = OkxWsStream<CumulativeVolumeDeltaTransformer<Self, Instrument::Key, OkxTrades>>;
}

impl<Instrument> StreamSelector<Instrument, OpenInterests> for Okx
where
    Instrument: InstrumentData,
{
    type SnapFetcher = NoInitialSnapshots;
    type Stream =
        OkxWsStream<StatelessTransformer<Self, Instrument::Key, OpenInterests, OkxOpenInterests>>;
}

impl<Instrument> StreamSelector<Instrument, OrderBooksL2> for Okx
where
    Instrument: InstrumentData,
{
    type SnapFetcher = NoInitialSnapshots;
    type Stream = OkxWsStream<OkxOrderBooksL2Transformer<Instrument::Key>>;
}
