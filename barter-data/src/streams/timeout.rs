//! Timeout wrapper for WebSocket streams.
//!
//! Provides a stream wrapper that monitors idle time and generates a timeout error
//! if no data is received for a configurable period. This is crucial for detecting
//! silent WebSocket disconnections that don't generate explicit errors.

use barter_integration::protocol::websocket::{WsError, WsMessage};
use futures::Stream;
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::Instant;

/// Default read timeout for WebSocket streams (2 minutes).
/// If no data is received within this period, a timeout error is generated.
pub const DEFAULT_WS_READ_TIMEOUT: Duration = Duration::from_secs(120);

/// A stream wrapper that monitors idle time and terminates the stream
/// if no data is received for the configured timeout period.
/// This triggers the reconnection logic in the parent ReconnectingStream.
#[derive(Debug)]
pub struct TimeoutStream<S> {
    inner: S,
    timeout_duration: Duration,
    deadline: Pin<Box<tokio::time::Sleep>>,
}

impl<S> TimeoutStream<S> {
    /// Create a new timeout stream wrapper with the specified timeout duration.
    pub fn new(inner: S, timeout_duration: Duration) -> Self {
        Self {
            inner,
            timeout_duration,
            deadline: Box::pin(tokio::time::sleep(timeout_duration)),
        }
    }

    /// Create a new timeout stream wrapper with the default timeout (2 minutes).
    pub fn with_default_timeout(inner: S) -> Self {
        Self::new(inner, DEFAULT_WS_READ_TIMEOUT)
    }
}

impl<S> Stream for TimeoutStream<S>
where
    S: Stream<Item = Result<WsMessage, WsError>> + Unpin,
{
    type Item = Result<WsMessage, WsError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let timeout_duration = self.timeout_duration;

        // First, check if the inner stream has data
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(item)) => {
                // Reset the deadline since we received data
                self.deadline.as_mut().reset(Instant::now() + timeout_duration);
                Poll::Ready(Some(item))
            }
            Poll::Ready(None) => {
                // Inner stream ended normally
                Poll::Ready(None)
            }
            Poll::Pending => {
                // No data available, check if we've timed out
                match self.deadline.as_mut().poll(cx) {
                    Poll::Ready(()) => {
                        // Timeout elapsed! Signal stream termination.
                        // This will trigger reconnection in the ReconnectingStream.
                        tracing::warn!(
                            timeout_secs = timeout_duration.as_secs(),
                            "WebSocket read timeout - no data received, triggering reconnection"
                        );

                        // Reset the deadline to avoid immediately timing out again if polled
                        self.deadline.as_mut().reset(Instant::now() + timeout_duration);

                        // Return None to signal stream termination, which triggers reconnection
                        Poll::Ready(None)
                    }
                    Poll::Pending => {
                        // Still waiting for data, deadline not reached
                        Poll::Pending
                    }
                }
            }
        }
    }
}

// Implement Unpin for TimeoutStream since we use Box<Sleep> which is Unpin
impl<S: Unpin> Unpin for TimeoutStream<S> {}
