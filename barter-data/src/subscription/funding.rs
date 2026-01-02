use super::SubscriptionKind;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Barter [`Subscription`](super::Subscription) [`SubscriptionKind`] that yields [`FundingRate`]
/// [`MarketEvent<T>`](crate::event::MarketEvent) events.
#[derive(
    Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Default, Deserialize, Serialize,
)]
pub struct FundingRates;

impl SubscriptionKind for FundingRates {
    type Event = FundingRate;

    fn as_str(&self) -> &'static str {
        "funding_rate"
    }
}

impl std::fmt::Display for FundingRates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Normalised Barter [`FundingRate`] model.
#[derive(Clone, Copy, PartialEq, PartialOrd, Debug, Deserialize, Serialize)]
pub struct FundingRate {
    /// Funding rate as a decimal (eg 0.0001 = 0.01%).
    pub rate: f64,
    /// Exchange-provided timestamp associated with the funding rate, when available.
    pub time: Option<DateTime<Utc>>,
    /// Next funding time, when available.
    pub next_time: Option<DateTime<Utc>>,
}
