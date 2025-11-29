use crate::{
    Identifier,
    exchange::bybit::Bybit,
    subscription::{
        Subscription,
        book::{OrderBooksL1, OrderBooksL2},
        cvd::CumulativeVolumeDeltas,
        liquidation::Liquidations,
        open_interest::OpenInterests,
        trade::PublicTrades,
    },
};
use serde::Serialize;

/// Type that defines how to translate a Barter [`Subscription`] into a [`Bybit`]
/// channel to be subscribed to.
///
/// See docs: <https://bybit-exchange.github.io/docs/v5/ws/connect>
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize)]
pub struct BybitChannel(pub &'static str);

impl BybitChannel {
    /// [`Bybit`] real-time trades channel name.
    ///
    /// See docs: <https://bybit-exchange.github.io/docs/v5/websocket/public/trade>
    pub const TRADES: Self = Self("publicTrade");

    /// [`Bybit`] real-time OrderBook Level1 (top of books) channel name.
    ///
    /// See docs: <https://bybit-exchange.github.io/docs/v5/websocket/public/orderbook>
    pub const ORDER_BOOK_L1: Self = Self("orderbook.1");

    /// [`Bybit`] OrderBook Level2 channel name (100ms delta updates, 200 levels).
    ///
    /// See docs: <https://bybit-exchange.github.io/docs/v5/websocket/public/orderbook>
    pub const ORDER_BOOK_L2: Self = Self("orderbook.200");

    /// [`Bybit`] tickers channel name, used to stream open interest updates.
    ///
    /// See docs: <https://bybit-exchange.github.io/docs/v5/websocket/public/tickers>
    pub const TICKERS: Self = Self("tickers");

    /// [`Bybit`] stream emitting global liquidation events across instruments.
    ///
    /// See docs: <https://bybit-exchange.github.io/docs/v5/websocket/public/all-liquidation>
    pub const ALL_LIQUIDATION: Self = Self("allLiquidation");
}

impl<Server, Instrument> Identifier<BybitChannel>
    for Subscription<Bybit<Server>, Instrument, PublicTrades>
{
    fn id(&self) -> BybitChannel {
        BybitChannel::TRADES
    }
}

impl<Server, Instrument> Identifier<BybitChannel>
    for Subscription<Bybit<Server>, Instrument, OrderBooksL1>
{
    fn id(&self) -> BybitChannel {
        BybitChannel::ORDER_BOOK_L1
    }
}

impl<Server, Instrument> Identifier<BybitChannel>
    for Subscription<Bybit<Server>, Instrument, OrderBooksL2>
{
    fn id(&self) -> BybitChannel {
        BybitChannel::ORDER_BOOK_L2
    }
}

impl<Server, Instrument> Identifier<BybitChannel>
    for Subscription<Bybit<Server>, Instrument, OpenInterests>
{
    fn id(&self) -> BybitChannel {
        BybitChannel::TICKERS
    }
}

impl<Server, Instrument> Identifier<BybitChannel>
    for Subscription<Bybit<Server>, Instrument, Liquidations>
{
    fn id(&self) -> BybitChannel {
        BybitChannel::ALL_LIQUIDATION
    }
}

impl<Server, Instrument> Identifier<BybitChannel>
    for Subscription<Bybit<Server>, Instrument, CumulativeVolumeDeltas>
{
    fn id(&self) -> BybitChannel {
        BybitChannel::TRADES
    }
}

impl AsRef<str> for BybitChannel {
    fn as_ref(&self) -> &str {
        self.0
    }
}
