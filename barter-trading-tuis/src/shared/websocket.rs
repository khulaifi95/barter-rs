/// WebSocket client for connecting to the aggregated market data server
///
/// Provides automatic reconnection, heartbeat, and event parsing
use crate::shared::types::{MarketEventEnvelope, MarketEventMessage};
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

/// WebSocket client configuration
#[derive(Debug, Clone)]
pub struct WebSocketConfig {
    /// WebSocket server URL
    pub url: String,
    /// Ping interval to keep connection alive
    pub ping_interval: Duration,
    /// Stale timeout - reconnect if no messages arrive within this duration
    pub stale_timeout: Duration,
    /// Trade stale timeout - reconnect if no trade events arrive within this duration
    pub trade_stale_timeout: Duration,
    /// Stale lag threshold - reconnect if trade lag exceeds this
    pub lag_stale_threshold: Duration,
    /// Stale lag duration - how long lag must persist before reconnecting
    pub lag_stale_duration: Duration,
    /// Reconnection delay after disconnect
    pub reconnect_delay: Duration,
    /// Maximum channel buffer size for events
    pub channel_buffer_size: usize,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            url: "ws://127.0.0.1:9001".to_string(),
            ping_interval: Duration::from_secs(30),
            stale_timeout: Duration::from_secs(15),
            trade_stale_timeout: Duration::from_secs(15),
            lag_stale_threshold: Duration::from_secs(15),
            lag_stale_duration: Duration::from_secs(10),
            reconnect_delay: Duration::from_secs(2),
            channel_buffer_size: 1000,
        }
    }
}

impl WebSocketConfig {
    /// Create a new configuration with custom URL
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            ..Default::default()
        }
    }

    /// Set ping interval
    pub fn with_ping_interval(mut self, interval: Duration) -> Self {
        self.ping_interval = interval;
        self
    }

    /// Set reconnect delay
    pub fn with_reconnect_delay(mut self, delay: Duration) -> Self {
        self.reconnect_delay = delay;
        self
    }

    /// Set stale timeout
    pub fn with_stale_timeout(mut self, timeout: Duration) -> Self {
        self.stale_timeout = timeout;
        self
    }

    /// Set trade stale timeout
    pub fn with_trade_stale_timeout(mut self, timeout: Duration) -> Self {
        self.trade_stale_timeout = timeout;
        self
    }

    /// Set lag stale threshold
    pub fn with_lag_stale_threshold(mut self, threshold: Duration) -> Self {
        self.lag_stale_threshold = threshold;
        self
    }

    /// Set lag stale duration
    pub fn with_lag_stale_duration(mut self, duration: Duration) -> Self {
        self.lag_stale_duration = duration;
        self
    }

    /// Set channel buffer size
    pub fn with_channel_buffer_size(mut self, size: usize) -> Self {
        self.channel_buffer_size = size;
        self
    }
}

/// WebSocket client for market data events
pub struct WebSocketClient {
    config: WebSocketConfig,
    event_tx: mpsc::Sender<MarketEventMessage>,
    event_rx: mpsc::Receiver<MarketEventMessage>,
    status_tx: mpsc::Sender<ConnectionStatus>,
    status_rx: mpsc::Receiver<ConnectionStatus>,
}

/// Connection status updates
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connected,
    Disconnected,
    Reconnecting,
}

impl WebSocketClient {
    /// Create a new WebSocket client with default configuration
    pub fn new() -> Self {
        Self::with_config(WebSocketConfig::default())
    }

    /// Create a new WebSocket client with custom configuration
    pub fn with_config(config: WebSocketConfig) -> Self {
        let (event_tx, event_rx) = mpsc::channel(config.channel_buffer_size);
        let (status_tx, status_rx) = mpsc::channel(10);

        Self {
            config,
            event_tx,
            event_rx,
            status_tx,
            status_rx,
        }
    }

    /// Start the WebSocket client connection
    ///
    /// Returns a receiver for market events and a receiver for connection status updates
    pub fn start(
        self,
    ) -> (
        mpsc::Receiver<MarketEventMessage>,
        mpsc::Receiver<ConnectionStatus>,
    ) {
        let config = self.config.clone();
        let event_tx = self.event_tx.clone();
        let status_tx = self.status_tx.clone();

        tokio::spawn(async move {
            run_websocket_loop(config, event_tx, status_tx).await;
        });

        (self.event_rx, self.status_rx)
    }
}

impl Default for WebSocketClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Main WebSocket connection loop with auto-reconnect
async fn run_websocket_loop(
    config: WebSocketConfig,
    event_tx: mpsc::Sender<MarketEventMessage>,
    status_tx: mpsc::Sender<ConnectionStatus>,
) {
    info!("Starting WebSocket client for {}", config.url);

    loop {
        // Notify about reconnection attempt
        let _ = status_tx.send(ConnectionStatus::Reconnecting).await;

        match connect_async(&config.url).await {
            Ok((ws_stream, _)) => {
                info!("Connected to WebSocket server at {}", config.url);
                let _ = status_tx.send(ConnectionStatus::Connected).await;

                let (mut write, mut read) = ws_stream.split();

                // Spawn ping task to keep connection alive
                let ping_interval = config.ping_interval;
                let (ping_shutdown_tx, mut ping_shutdown_rx) = mpsc::channel::<()>(1);

                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(ping_interval);
                    loop {
                        tokio::select! {
                            _ = interval.tick() => {
                                if write.send(Message::Ping(vec![].into())).await.is_err() {
                                    debug!("Failed to send ping, connection likely dead");
                                    break;
                                }
                            }
                            _ = ping_shutdown_rx.recv() => {
                                debug!("Ping task shutting down");
                                break;
                            }
                        }
                    }
                });

                let mut last_event = std::time::Instant::now();
                let mut last_trade_event = std::time::Instant::now();
                let mut stale_interval = tokio::time::interval(config.stale_timeout);
                stale_interval.tick().await;
                let mut lag_breach_since: Option<std::time::Instant> = None;

                // Main message reading loop
                let mut should_break = false;
                loop {
                    tokio::select! {
                        _ = stale_interval.tick() => {
                            if last_event.elapsed() > config.stale_timeout {
                                warn!(
                                    "WebSocket stale (no data for {:?}), reconnecting...",
                                    config.stale_timeout
                                );
                                should_break = true;
                                break;
                            }
                            if last_trade_event.elapsed() > config.trade_stale_timeout {
                                warn!(
                                    "WebSocket trade stale (no trades for {:?}), reconnecting...",
                                    config.trade_stale_timeout
                                );
                                should_break = true;
                                break;
                            }
                        }
                        msg = read.next() => {
                            match msg {
                                Some(Ok(msg)) => {
                                    last_event = std::time::Instant::now();
                                    match msg {
                                        Message::Text(text) => {
                                            // Check if it's a welcome message
                                            if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&text) {
                                                if json_val.get("type").and_then(|v| v.as_str()) == Some("welcome")
                                                {
                                                    debug!("Received welcome message");
                                                    continue;
                                                }
                                            }

                                            // Try to parse as market event
                                            if let Ok(envelope) = serde_json::from_str::<MarketEventEnvelope>(&text)
                                            {
                                                if envelope.payload.kind == "trade" {
                                                    last_trade_event = std::time::Instant::now();
                                                    let lag_ms = chrono::Utc::now().timestamp_millis()
                                                        - envelope.payload.time_exchange.timestamp_millis();
                                                    if lag_ms >= config.lag_stale_threshold.as_millis() as i64 {
                                                        if lag_breach_since.is_none() {
                                                            lag_breach_since = Some(std::time::Instant::now());
                                                        }
                                                        if let Some(start) = lag_breach_since {
                                                            if start.elapsed() >= config.lag_stale_duration {
                                                                warn!(
                                                                    "WebSocket lag stale ({}ms), reconnecting...",
                                                                    lag_ms
                                                                );
                                                                should_break = true;
                                                                break;
                                                            }
                                                        }
                                                    } else {
                                                        lag_breach_since = None;
                                                    }
                                                }
                                                if event_tx.send(envelope.payload).await.is_err() {
                                                    warn!("Event receiver dropped, stopping client");
                                                    should_break = true;
                                                    break;
                                                }
                                                continue;
                                            }

                                            match serde_json::from_str::<MarketEventMessage>(&text) {
                                                Ok(event) => {
                                                    if event.kind == "trade" {
                                                        last_trade_event = std::time::Instant::now();
                                                        let lag_ms = chrono::Utc::now().timestamp_millis()
                                                            - event.time_exchange.timestamp_millis();
                                                        if lag_ms >= config.lag_stale_threshold.as_millis() as i64 {
                                                            if lag_breach_since.is_none() {
                                                                lag_breach_since = Some(std::time::Instant::now());
                                                            }
                                                            if let Some(start) = lag_breach_since {
                                                                if start.elapsed() >= config.lag_stale_duration {
                                                                    warn!(
                                                                        "WebSocket lag stale ({}ms), reconnecting...",
                                                                        lag_ms
                                                                    );
                                                                    should_break = true;
                                                                    break;
                                                                }
                                                            }
                                                        } else {
                                                            lag_breach_since = None;
                                                        }
                                                    }
                                                    if event_tx.send(event).await.is_err() {
                                                        warn!("Event receiver dropped, stopping client");
                                                        should_break = true;
                                                        break;
                                                    }
                                                }
                                                Err(e) => {
                                                    error!("Failed to parse message: {}", e);
                                                    debug!("Raw message: {}", text);
                                                }
                                            }
                                        }
                                        Message::Close(_) => {
                                            info!("Server closed connection");
                                            should_break = true;
                                            break;
                                        }
                                        Message::Ping(_) | Message::Pong(_) => {
                                            // Heartbeat messages - tungstenite handles these automatically
                                        }
                                        _ => {}
                                    }
                                }
                                Some(Err(e)) => {
                                    error!("WebSocket error: {}", e);
                                    should_break = true;
                                    break;
                                }
                                None => {
                                    info!("WebSocket stream ended");
                                    should_break = true;
                                    break;
                                }
                            }
                        }
                    }
                }

                // Stop ping task
                let _ = ping_shutdown_tx.send(()).await;

                // Notify disconnection
                let _ = status_tx.send(ConnectionStatus::Disconnected).await;

                if should_break {
                    warn!("Connection closed, will reconnect...");
                }
            }
            Err(e) => {
                error!("Failed to connect to {}: {}", config.url, e);
                let _ = status_tx.send(ConnectionStatus::Disconnected).await;
            }
        }

        // Wait before reconnecting
        debug!(
            "Waiting {:?} before reconnecting...",
            config.reconnect_delay
        );
        tokio::time::sleep(config.reconnect_delay).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_builder() {
        let config = WebSocketConfig::new("ws://localhost:8080")
            .with_ping_interval(Duration::from_secs(15))
            .with_reconnect_delay(Duration::from_secs(5))
            .with_stale_timeout(Duration::from_secs(12))
            .with_trade_stale_timeout(Duration::from_secs(9))
            .with_lag_stale_threshold(Duration::from_secs(20))
            .with_lag_stale_duration(Duration::from_secs(8))
            .with_channel_buffer_size(500);

        assert_eq!(config.url, "ws://localhost:8080");
        assert_eq!(config.ping_interval, Duration::from_secs(15));
        assert_eq!(config.reconnect_delay, Duration::from_secs(5));
        assert_eq!(config.stale_timeout, Duration::from_secs(12));
        assert_eq!(config.trade_stale_timeout, Duration::from_secs(9));
        assert_eq!(config.lag_stale_threshold, Duration::from_secs(20));
        assert_eq!(config.lag_stale_duration, Duration::from_secs(8));
        assert_eq!(config.channel_buffer_size, 500);
    }

    #[test]
    fn test_default_config() {
        let config = WebSocketConfig::default();
        assert_eq!(config.url, "ws://127.0.0.1:9001");
        assert_eq!(config.ping_interval, Duration::from_secs(30));
        assert_eq!(config.stale_timeout, Duration::from_secs(15));
        assert_eq!(config.trade_stale_timeout, Duration::from_secs(15));
        assert_eq!(config.lag_stale_threshold, Duration::from_secs(15));
        assert_eq!(config.lag_stale_duration, Duration::from_secs(10));
        assert_eq!(config.reconnect_delay, Duration::from_secs(2));
        assert_eq!(config.channel_buffer_size, 1000);
    }
}
