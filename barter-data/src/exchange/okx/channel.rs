use super::Okx;
use crate::{
    Identifier,
    subscription::{
        Subscription,
        book::OrderBooksL2,
        cvd::CumulativeVolumeDeltas,
        liquidation::Liquidations,
        open_interest::OpenInterests,
        trade::PublicTrades,
    },
};
use serde::Serialize;

/// Type that defines how to translate a Barter [`Subscription`] into a
/// [`Okx`] channel to be subscribed to.
///
/// See docs: <https://www.okx.com/docs-v5/en/#websocket-api-public-channel>
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize)]
pub struct OkxChannel(pub &'static str);

impl OkxChannel {
    /// [`Okx`] real-time trades channel.
    ///
    /// See docs: <https://www.okx.com/docs-v5/en/#websocket-api-public-channel-trades-channel>
    pub const TRADES: Self = Self("trades");

    /// [`Okx`] liquidation orders channel.
    ///
    /// See docs: <https://www.okx.com/docs-v5/en/#websocket-api-public-channel-liquidation-orders-channel>
    pub const LIQUIDATION_ORDERS: Self = Self("liquidation-orders");

    /// [`Okx`] open interest channel.
    ///
    /// See docs: <https://www.okx.com/docs-v5/en/#websocket-api-public-channel-open-interest-channel>
    pub const OPEN_INTEREST: Self = Self("open-interest");

    /// [`Okx`] OrderBook Level2 channel (400 levels, 100ms push).
    ///
    /// See docs: <https://www.okx.com/docs-v5/en/#websocket-api-public-channel-order-book-channel>
    pub const ORDER_BOOK_L2: Self = Self("books");
}

impl<Instrument> Identifier<OkxChannel> for Subscription<Okx, Instrument, PublicTrades> {
    fn id(&self) -> OkxChannel {
        OkxChannel::TRADES
    }
}

impl<Instrument> Identifier<OkxChannel> for Subscription<Okx, Instrument, Liquidations> {
    fn id(&self) -> OkxChannel {
        OkxChannel::LIQUIDATION_ORDERS
    }
}

impl<Instrument> Identifier<OkxChannel> for Subscription<Okx, Instrument, CumulativeVolumeDeltas> {
    fn id(&self) -> OkxChannel {
        OkxChannel::TRADES
    }
}

impl<Instrument> Identifier<OkxChannel> for Subscription<Okx, Instrument, OpenInterests> {
    fn id(&self) -> OkxChannel {
        OkxChannel::OPEN_INTEREST
    }
}

impl<Instrument> Identifier<OkxChannel> for Subscription<Okx, Instrument, OrderBooksL2> {
    fn id(&self) -> OkxChannel {
        OkxChannel::ORDER_BOOK_L2
    }
}

impl AsRef<str> for OkxChannel {
    fn as_ref(&self) -> &str {
        self.0
    }
}
