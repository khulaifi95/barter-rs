use super::SubscriptionKind;
use serde::{Deserialize, Serialize};

/// Barter [`Subscription`](super::Subscription) [`SubscriptionKind`] that yields cumulative volume
/// delta updates derived from trade data.
#[derive(
    Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Default, Deserialize, Serialize,
)]
pub struct CumulativeVolumeDeltas;

impl SubscriptionKind for CumulativeVolumeDeltas {
    type Event = CumulativeVolumeDelta;

    fn as_str(&self) -> &'static str {
        "cumulative_volume_delta"
    }
}

impl std::fmt::Display for CumulativeVolumeDeltas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Normalised Barter cumulative volume delta model.
#[derive(Clone, Copy, PartialEq, PartialOrd, Debug, Deserialize, Serialize, Default)]
pub struct CumulativeVolumeDelta {
    /// Running sum of aggressive buy minus sell volume in base units.
    pub delta_base: f64,
    /// Running sum of aggressive buy minus sell volume in quote units.
    pub delta_quote: f64,
}
