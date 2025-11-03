use super::SubscriptionKind;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Barter [`Subscription`](super::Subscription) [`SubscriptionKind`] that yields [`OpenInterest`]
/// [`MarketEvent<T>`](crate::event::MarketEvent) events.
#[derive(
    Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Default, Deserialize, Serialize,
)]
pub struct OpenInterests;

impl SubscriptionKind for OpenInterests {
    type Event = OpenInterest;

    fn as_str(&self) -> &'static str {
        "open_interest"
    }
}

impl std::fmt::Display for OpenInterests {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Normalised Barter [`OpenInterest`] model.
#[derive(Clone, Copy, PartialEq, PartialOrd, Debug, Deserialize, Serialize)]
pub struct OpenInterest {
    /// Total open interest contracts / lots reported by the exchange.
    pub contracts: f64,
    /// Notional open interest value reported by the exchange (typically in quote currency).
    pub notional: Option<f64>,
    /// Exchange-provided timestamp associated with the open interest reading, when available.
    pub time: Option<DateTime<Utc>>,
}
