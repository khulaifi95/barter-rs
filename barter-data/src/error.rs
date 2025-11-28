use crate::subscription::SubKind;
use barter_instrument::{exchange::ExchangeId, index::error::IndexError};
use barter_integration::{error::SocketError, subscription::SubscriptionId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// All errors generated in `barter-data`.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Deserialize, Serialize, Error)]
pub enum DataError {
    #[error("failed to index market data Subscriptions: {0}")]
    Index(#[from] IndexError),

    #[error("failed to initialise reconnecting MarketStream due to empty subscriptions")]
    SubscriptionsEmpty,

    #[error("unsupported DynamicStreams Subscription SubKind: {0}")]
    UnsupportedSubKind(SubKind),

    #[error("initial snapshot missing for: {0}")]
    InitialSnapshotMissing(SubscriptionId),

    #[error("initial snapshot invalid: {0}")]
    InitialSnapshotInvalid(String),

    #[error("SocketError: {0}")]
    Socket(String),

    #[error("unsupported dynamic Subscription for exchange: {exchange}, kind: {sub_kind}")]
    Unsupported {
        exchange: ExchangeId,
        sub_kind: SubKind,
    },

    #[error(
        "\
        InvalidSequence: first_update_id {first_update_id} does not follow on from the \
        prev_last_update_id {prev_last_update_id} \
    "
    )]
    InvalidSequence {
        prev_last_update_id: u64,
        first_update_id: u64,
    },
}

impl DataError {
    /// Determine if an error requires a [`MarketStream`](super::MarketStream) to re-initialise.
    #[allow(clippy::match_like_matches_macro)]
    pub fn is_terminal(&self) -> bool {
        match self {
            DataError::InvalidSequence { .. } => true,
            DataError::Socket(error_msg) => {
                // Socket termination errors require reconnection
                // Check for keywords that indicate the connection is closed/terminated
                // Use case-insensitive matching for robustness
                let error_lower = error_msg.to_lowercase();
                error_lower.contains("terminated")
                    || error_lower.contains("connectionclosed")
                    || error_lower.contains("alreadyclosed")
                    || error_lower.contains("sendafterclosing")
                    // IO errors often indicate network disconnection
                    || error_lower.contains("io(")
                    // Read timeout indicates silent stream death - requires reconnection
                    || error_lower.contains("timeout")
            }
            _ => false,
        }
    }
}

impl From<SocketError> for DataError {
    fn from(value: SocketError) -> Self {
        Self::Socket(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_error_is_terminal() {
        struct TestCase {
            input: DataError,
            expected: bool,
        }

        let tests = vec![
            TestCase {
                // TC0: is terminal w/ DataError::InvalidSequence
                input: DataError::InvalidSequence {
                    prev_last_update_id: 0,
                    first_update_id: 0,
                },
                expected: true,
            },
            TestCase {
                // TC1: is not terminal w/ DataError::Socket (non-termination error)
                input: DataError::from(SocketError::Sink),
                expected: false,
            },
            TestCase {
                // TC2: is terminal w/ DataError::Socket containing "Terminated"
                input: DataError::Socket("ExchangeStream terminated with closing frame".to_string()),
                expected: true,
            },
            TestCase {
                // TC3: is terminal w/ DataError::Socket containing "ConnectionClosed"
                input: DataError::Socket("WebSocket error: ConnectionClosed".to_string()),
                expected: true,
            },
            TestCase {
                // TC4: is terminal w/ DataError::Socket containing "Io("
                input: DataError::Socket("WebSocket error: Io(Kind(UnexpectedEof))".to_string()),
                expected: true,
            },
            TestCase {
                // TC5: is terminal w/ DataError::Socket containing "timeout"
                input: DataError::Socket("WebSocket read timeout: no data received for 120 seconds".to_string()),
                expected: true,
            },
        ];

        for (index, test) in tests.into_iter().enumerate() {
            let actual = test.input.is_terminal();
            assert_eq!(actual, test.expected, "TC{} failed", index);
        }
    }
}
