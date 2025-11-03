use super::ExchangeTransformer;
use crate::{
    Identifier,
    error::DataError,
    event::{MarketEvent, MarketIter},
    exchange::Connector,
    subscription::{
        Map,
        cvd::{CumulativeVolumeDelta, CumulativeVolumeDeltas},
        trade::PublicTrade,
    },
};
use async_trait::async_trait;
use barter_instrument::{Side, exchange::ExchangeId};
use barter_integration::{
    Transformer, protocol::websocket::WsMessage, subscription::SubscriptionId,
};
use fnv::FnvHashMap;
use serde::Deserialize;
use std::{hash::Hash, marker::PhantomData};
use tokio::sync::mpsc;

#[derive(Default, Clone, Debug)]
struct CvdTotals {
    delta_base: f64,
    delta_quote: f64,
}

/// Transformer that derives cumulative volume delta from an underlying trade feed.
#[derive(Clone, Debug)]
pub struct CumulativeVolumeDeltaTransformer<Exchange, InstrumentKey, Input> {
    instrument_map: Map<InstrumentKey>,
    totals: FnvHashMap<InstrumentKey, CvdTotals>,
    phantom: PhantomData<(Exchange, Input)>,
}

#[async_trait]
impl<Exchange, InstrumentKey, Input>
    ExchangeTransformer<Exchange, InstrumentKey, CumulativeVolumeDeltas>
    for CumulativeVolumeDeltaTransformer<Exchange, InstrumentKey, Input>
where
    Exchange: Connector + Send,
    InstrumentKey: Clone + Eq + Hash + Send,
    Input: Identifier<Option<SubscriptionId>> + for<'de> Deserialize<'de>,
    MarketIter<InstrumentKey, PublicTrade>: From<(ExchangeId, InstrumentKey, Input)>,
{
    async fn init(
        instrument_map: Map<InstrumentKey>,
        _: &[MarketEvent<InstrumentKey, CumulativeVolumeDelta>],
        _: mpsc::UnboundedSender<WsMessage>,
    ) -> Result<Self, DataError> {
        Ok(Self {
            instrument_map,
            totals: Default::default(),
            phantom: PhantomData,
        })
    }
}

impl<Exchange, InstrumentKey, Input> Transformer
    for CumulativeVolumeDeltaTransformer<Exchange, InstrumentKey, Input>
where
    Exchange: Connector,
    InstrumentKey: Clone + Eq + Hash,
    Input: Identifier<Option<SubscriptionId>> + for<'de> Deserialize<'de>,
    MarketIter<InstrumentKey, PublicTrade>: From<(ExchangeId, InstrumentKey, Input)>,
{
    type Error = DataError;
    type Input = Input;
    type Output = MarketEvent<InstrumentKey, CumulativeVolumeDelta>;
    type OutputIter = Vec<Result<Self::Output, Self::Error>>;

    fn transform(&mut self, input: Self::Input) -> Self::OutputIter {
        let subscription_id = match input.id() {
            Some(subscription_id) => subscription_id,
            None => return vec![],
        };

        match self.instrument_map.find(&subscription_id) {
            Ok(instrument) => {
                let instrument_key = instrument.clone();
                MarketIter::<InstrumentKey, PublicTrade>::from((
                    Exchange::ID,
                    instrument_key,
                    input,
                ))
                .0
                .into_iter()
                .map(|result| match result {
                    Ok(event) => {
                        let MarketEvent {
                            time_exchange,
                            time_received,
                            exchange,
                            instrument,
                            kind: trade,
                        } = event;

                        let signed_base = match trade.side {
                            Side::Buy => trade.amount,
                            Side::Sell => -trade.amount,
                        };
                        let signed_quote = signed_base * trade.price;

                        let totals = self
                            .totals
                            .entry(instrument.clone())
                            .or_insert_with(CvdTotals::default);
                        totals.delta_base += signed_base;
                        totals.delta_quote += signed_quote;

                        Ok(MarketEvent {
                            time_exchange,
                            time_received,
                            exchange,
                            instrument,
                            kind: CumulativeVolumeDelta {
                                delta_base: totals.delta_base,
                                delta_quote: totals.delta_quote,
                            },
                        })
                    }
                    Err(err) => Err(err),
                })
                .collect()
            }
            Err(unidentifiable) => vec![Err(DataError::from(unidentifiable))],
        }
    }
}
