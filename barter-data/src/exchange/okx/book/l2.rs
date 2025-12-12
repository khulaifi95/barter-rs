//! OKX Level 2 OrderBook transformer.

use crate::{
    Identifier,
    error::DataError,
    event::MarketEvent,
    exchange::okx::Okx,
    subscription::{
        Map,
        book::{OrderBookEvent, OrderBooksL2},
    },
    transformer::ExchangeTransformer,
};
use async_trait::async_trait;
use barter_instrument::exchange::ExchangeId;
use barter_integration::{Transformer, protocol::websocket::WsMessage};
use chrono::{DateTime, Utc};
use derive_more::Constructor;
use tokio::sync::mpsc::UnboundedSender;
use tracing::debug;

use super::{OkxOrderBookAction, OkxOrderBookMessage};

/// Metadata for tracking OKX orderbook state per instrument.
#[derive(Debug, Constructor)]
pub struct OkxOrderBookL2Meta<InstrumentKey> {
    pub key: InstrumentKey,
    pub initialized: bool,
    pub last_update: Option<DateTime<Utc>>,
}

/// OKX Level 2 OrderBook transformer.
#[derive(Debug)]
pub struct OkxOrderBooksL2Transformer<InstrumentKey> {
    instrument_map: Map<OkxOrderBookL2Meta<InstrumentKey>>,
}

#[async_trait]
impl<InstrumentKey> ExchangeTransformer<Okx, InstrumentKey, OrderBooksL2>
    for OkxOrderBooksL2Transformer<InstrumentKey>
where
    InstrumentKey: Clone + PartialEq + Send + Sync,
{
    async fn init(
        instrument_map: Map<InstrumentKey>,
        _initial_snapshots: &[MarketEvent<InstrumentKey, OrderBookEvent>],
        _ws_sink: UnboundedSender<WsMessage>,
    ) -> Result<Self, DataError> {
        let instrument_map = instrument_map
            .0
            .into_iter()
            .map(|(sub_id, instrument_key)| {
                (
                    sub_id,
                    OkxOrderBookL2Meta::new(instrument_key, false, None),
                )
            })
            .collect();

        Ok(Self { instrument_map })
    }
}

impl<InstrumentKey> Transformer for OkxOrderBooksL2Transformer<InstrumentKey>
where
    InstrumentKey: Clone,
{
    type Error = DataError;
    type Input = OkxOrderBookMessage;
    type Output = MarketEvent<InstrumentKey, OrderBookEvent>;
    type OutputIter = Vec<Result<Self::Output, Self::Error>>;

    fn transform(&mut self, input: Self::Input) -> Self::OutputIter {
        // Determine if the message has an identifiable SubscriptionId
        let subscription_id = match input.id() {
            Some(subscription_id) => subscription_id,
            None => return vec![],
        };

        // Find Instrument associated with Input and transform
        let instrument = match self.instrument_map.find_mut(&subscription_id) {
            Ok(instrument) => instrument,
            Err(unidentifiable) => return vec![Err(DataError::from(unidentifiable))],
        };

        // Extract payload
        let payload = match &input {
            OkxOrderBookMessage::Ignore => return vec![],
            OkxOrderBookMessage::Payload(p) => p,
        };

        // Handle snapshot vs update
        match payload.action {
            OkxOrderBookAction::Snapshot => {
                // Mark as initialized on snapshot
                instrument.initialized = true;
                if let Some(data) = payload.data.first() {
                    instrument.last_update = Some(data.ts);
                }
            }
            OkxOrderBookAction::Update => {
                // Drop updates if we haven't received a snapshot yet
                if !instrument.initialized {
                    debug!("OKX: Update received before snapshot, ignoring");
                    return vec![];
                }
                if let Some(data) = payload.data.first() {
                    instrument.last_update = Some(data.ts);
                }
            }
        }

        // Convert to MarketEvent
        crate::event::MarketIter::<InstrumentKey, OrderBookEvent>::from((
            ExchangeId::Okx,
            instrument.key.clone(),
            input,
        ))
        .0
    }
}
