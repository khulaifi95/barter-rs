//! Feature computation modules.

pub mod events;
pub mod large_trades;
pub mod tpo;
pub mod volume_profile;

pub use events::{EventDetector, ProfileEvent, ProfileEventType};
pub use large_trades::{LargeTrade, LargeTradeCategory, LargeTradeDetector};
pub use tpo::{TpoBracket, TpoProcessor, bracket_label};
pub use volume_profile::{ValueArea, VolumeProfile, compute_value_area};
