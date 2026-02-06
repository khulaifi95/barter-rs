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
    /// Expect envelope format (must match server WS_ENVELOPE setting)
    /// When true: parse only envelope format, fail on non-envelope
    /// When false: parse only raw format, fail on envelope
    pub expect_envelope: bool,
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
            // Read from same env var as server to avoid config drift
            expect_envelope: std::env::var("WS_ENVELOPE")
                .ok()
                .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE"))
                .unwrap_or(false),
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

    /// Set expected message format (must match server WS_ENVELOPE setting)
    pub fn with_expect_envelope(mut self, expect: bool) -> Self {
        self.expect_envelope = expect;
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
                let mut dropped_events: u64 = 0;
                let mut last_drop_log = std::time::Instant::now();
                let mut stale_interval = tokio::time::interval(config.stale_timeout);
                stale_interval.tick().await;
                let mut lag_breach_since: Option<std::time::Instant> = None;

                // Main message reading loop
                loop {
                    tokio::select! {
                        _ = stale_interval.tick() => {
                            if last_event.elapsed() > config.stale_timeout {
                                warn!(
                                    "WebSocket stale (no data for {:?}), reconnecting...",
                                    config.stale_timeout
                                );
                                break;
                            }
                            if last_trade_event.elapsed() > config.trade_stale_timeout {
                                warn!(
                                    "WebSocket trade stale (no trades for {:?}), reconnecting...",
                                    config.trade_stale_timeout
                                );
                                break;
                            }
                        }
                        msg = read.next() => {
                            match msg {
                                Some(Ok(msg)) => {
                                    last_event = std::time::Instant::now();
                                    match msg {
                                        Message::Text(text) => {
                                            // Fast path: check for welcome message without full JSON parse
                                            if text.contains(r#""type":"welcome"#) {
                                                debug!("Received welcome message");
                                                continue;
                                            }

                                            // Single format parsing based on config (no fallback = faster)
                                            let parse_result: Result<MarketEventMessage, _> = if config.expect_envelope {
                                                // Envelope format: extract payload
                                                serde_json::from_str::<MarketEventEnvelope>(&text)
                                                    .map(|env| env.payload)
                                            } else {
                                                // Raw format: parse directly
                                                serde_json::from_str::<MarketEventMessage>(&text)
                                            };

                                            match parse_result {
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
                                                                    break;
                                                                }
                                                            }
                                                        } else {
                                                            lag_breach_since = None;
                                                        }
                                                    }
                                                    match event_tx.try_send(event) {
                                                        Ok(()) => {}
                                                        Err(mpsc::error::TrySendError::Closed(_)) => {
                                                            warn!("Event receiver dropped, stopping client");
                                                            break;
                                                        }
                                                        Err(mpsc::error::TrySendError::Full(_)) => {
                                                            dropped_events += 1;
                                                            if last_drop_log.elapsed() >= Duration::from_secs(1) {
                                                                warn!(
                                                                    "Event channel full - dropped {} events in last 1s",
                                                                    dropped_events
                                                                );
                                                                dropped_events = 0;
                                                                last_drop_log = std::time::Instant::now();
                                                            }
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    error!("Failed to parse message (expect_envelope={}): {}", config.expect_envelope, e);
                                                    debug!("Raw message: {}", text);
                                                }
                                            }
                                        }
                                        Message::Binary(bytes) => {
                                            // Binary frame: parse from bytes (zero-copy from server)
                                            let text = match std::str::from_utf8(&bytes) {
                                                Ok(t) => t,
                                                Err(e) => {
                                                    error!("Invalid UTF-8 in binary frame: {}", e);
                                                    continue;
                                                }
                                            };

                                            // Fast path: check for welcome message without full JSON parse
                                            if text.contains(r#""type":"welcome"#) {
                                                debug!("Received welcome message");
                                                continue;
                                            }

                                            // Single format parsing based on config (no fallback = faster)
                                            let parse_result: Result<MarketEventMessage, _> = if config.expect_envelope {
                                                // Envelope format: extract payload
                                                serde_json::from_str::<MarketEventEnvelope>(text)
                                                    .map(|env| env.payload)
                                            } else {
                                                // Raw format: parse directly
                                                serde_json::from_str::<MarketEventMessage>(text)
                                            };

                                            match parse_result {
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
                                                                    break;
                                                                }
                                                            }
                                                        } else {
                                                            lag_breach_since = None;
                                                        }
                                                    }
                                                    match event_tx.try_send(event) {
                                                        Ok(()) => {}
                                                        Err(mpsc::error::TrySendError::Closed(_)) => {
                                                            warn!("Event receiver dropped, stopping client");
                                                            break;
                                                        }
                                                        Err(mpsc::error::TrySendError::Full(_)) => {
                                                            dropped_events += 1;
                                                            if last_drop_log.elapsed() >= Duration::from_secs(1) {
                                                                warn!(
                                                                    "Event channel full - dropped {} events in last 1s",
                                                                    dropped_events
                                                                );
                                                                dropped_events = 0;
                                                                last_drop_log = std::time::Instant::now();
                                                            }
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    error!("Failed to parse binary message (expect_envelope={}): {}", config.expect_envelope, e);
                                                    debug!("Raw message: {}", text);
                                                }
                                            }
                                        }
                                        Message::Close(_) => {
                                            info!("Server closed connection");
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
                                    break;
                                }
                                None => {
                                    info!("WebSocket stream ended");
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

                warn!("Connection closed, will reconnect...");
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
        assert!(
            !config.expect_envelope,
            "Default should be non-envelope format"
        );
    }

    #[test]
    fn test_expect_envelope_config() {
        let config = WebSocketConfig::default().with_expect_envelope(true);
        assert!(config.expect_envelope);

        let config_raw = WebSocketConfig::default().with_expect_envelope(false);
        assert!(!config_raw.expect_envelope);
    }

    // ========== CRITICAL: Binary frame parsing tests ==========

    fn create_test_event() -> MarketEventMessage {
        use chrono::Utc;
        MarketEventMessage {
            time_exchange: Utc::now(),
            time_received: Utc::now(),
            exchange: "TestExchange".to_string(),
            instrument: crate::shared::types::InstrumentInfo {
                base: "BTC".to_string(),
                quote: "USD".to_string(),
                kind: "Perpetual".to_string(),
            },
            kind: "trade".to_string(),
            data: serde_json::json!({"price": 50000.0, "amount": 1.0}),
        }
    }

    #[test]
    fn test_binary_frame_parsing_raw_format() {
        // Simulate server sending raw format (no envelope)
        let event = create_test_event();
        let json = serde_json::to_string(&event).unwrap();
        let bytes = json.as_bytes();

        // Parse as if we received binary frame
        let text = std::str::from_utf8(bytes).expect("Should be valid UTF-8");

        // With expect_envelope=false, should parse successfully
        let parsed: Result<MarketEventMessage, _> = serde_json::from_str(text);
        assert!(parsed.is_ok(), "Should parse raw format successfully");

        let msg = parsed.unwrap();
        assert_eq!(msg.exchange, "TestExchange");
        assert_eq!(msg.kind, "trade");
    }

    #[test]
    fn test_binary_frame_parsing_envelope_format() {
        use chrono::Utc;
        // Simulate server sending envelope format
        let event = create_test_event();
        let envelope = MarketEventEnvelope {
            schema_version: 1,
            source: "test-server".to_string(),
            time_sent: Utc::now(),
            payload: event,
        };
        let json = serde_json::to_string(&envelope).unwrap();
        let bytes = json.as_bytes();

        // Parse as if we received binary frame
        let text = std::str::from_utf8(bytes).expect("Should be valid UTF-8");

        // With expect_envelope=true, should parse successfully
        let parsed: Result<MarketEventEnvelope, _> = serde_json::from_str(text);
        assert!(parsed.is_ok(), "Should parse envelope format successfully");

        let env = parsed.unwrap();
        assert_eq!(env.schema_version, 1);
        assert_eq!(env.payload.exchange, "TestExchange");
    }

    #[test]
    fn test_envelope_mismatch_graceful_failure() {
        // Server sends envelope, client expects raw
        use chrono::Utc;
        let event = create_test_event();
        let envelope = MarketEventEnvelope {
            schema_version: 1,
            source: "test-server".to_string(),
            time_sent: Utc::now(),
            payload: event.clone(),
        };
        let envelope_json = serde_json::to_string(&envelope).unwrap();

        // Try to parse envelope as raw format (mismatch)
        let parsed_raw: Result<MarketEventMessage, _> = serde_json::from_str(&envelope_json);
        // This should fail gracefully (no crash, just parse error)
        // The envelope has different fields, so parsing as raw will fail
        assert!(
            parsed_raw.is_err(),
            "Should fail to parse envelope as raw format"
        );

        // Now test the reverse: raw sent, envelope expected
        let raw_json = serde_json::to_string(&event).unwrap();
        let parsed_envelope: Result<MarketEventEnvelope, _> = serde_json::from_str(&raw_json);
        assert!(
            parsed_envelope.is_err(),
            "Should fail to parse raw as envelope format"
        );
    }

    #[test]
    #[allow(invalid_from_utf8)]
    fn test_invalid_utf8_binary_frame_handling() {
        // Simulate invalid UTF-8 bytes
        let invalid_bytes: &[u8] = &[0xFF, 0xFE, 0x00, 0x01];

        // This is how the code handles it
        let result = std::str::from_utf8(invalid_bytes);
        assert!(result.is_err(), "Should fail on invalid UTF-8");

        // The error should be catchable (no panic)
        match result {
            Ok(_) => panic!("Should not succeed with invalid UTF-8"),
            Err(e) => {
                // Error is handled gracefully
                assert!(e.to_string().contains("invalid"));
            }
        }
    }

    #[test]
    fn test_lag_threshold_detection() {
        use chrono::Utc;
        use std::time::Duration;

        let config = WebSocketConfig::default()
            .with_lag_stale_threshold(Duration::from_secs(15))
            .with_lag_stale_duration(Duration::from_secs(10));

        // Simulate trade event with different lag values
        let now_ms = Utc::now().timestamp_millis();

        // Event with 5s lag (under threshold)
        let event_time_5s = chrono::DateTime::from_timestamp_millis(now_ms - 5_000).unwrap();
        let lag_5s = now_ms - event_time_5s.timestamp_millis();
        assert!(
            lag_5s < config.lag_stale_threshold.as_millis() as i64,
            "5s lag should be under 15s threshold"
        );

        // Event with 20s lag (over threshold)
        let event_time_20s = chrono::DateTime::from_timestamp_millis(now_ms - 20_000).unwrap();
        let lag_20s = now_ms - event_time_20s.timestamp_millis();
        assert!(
            lag_20s >= config.lag_stale_threshold.as_millis() as i64,
            "20s lag should exceed 15s threshold"
        );
    }

    #[test]
    fn test_welcome_message_detection() {
        // Welcome messages should be detected and skipped
        let welcome_json = r#"{"type":"welcome","message":"Connected to barter-data market feed","timestamp":"2024-01-01T00:00:00Z"}"#;

        // Fast path check that's used in the actual code
        let is_welcome = welcome_json.contains(r#""type":"welcome"#);
        assert!(is_welcome, "Should detect welcome message");

        // Non-welcome message
        let trade_json =
            r#"{"time_exchange":"2024-01-01T00:00:00Z","exchange":"Test","kind":"trade"}"#;
        let is_not_welcome = !trade_json.contains(r#""type":"welcome"#);
        assert!(is_not_welcome, "Should not detect trade as welcome");
    }

    // ========== CRITICAL: Stale/Lag reconnection trigger tests ==========

    #[test]
    fn test_stale_timeout_should_trigger_reconnect() {
        // This tests the logic that triggers reconnection when no events arrive
        let config = WebSocketConfig::default().with_stale_timeout(Duration::from_secs(15));

        // Simulate: last event was 20 seconds ago
        let last_event = std::time::Instant::now() - Duration::from_secs(20);

        // This is the check from the actual code
        let should_reconnect = last_event.elapsed() > config.stale_timeout;
        assert!(
            should_reconnect,
            "Should trigger reconnect when stale_timeout (15s) exceeded"
        );
    }

    #[test]
    fn test_stale_timeout_should_not_trigger_prematurely() {
        let config = WebSocketConfig::default().with_stale_timeout(Duration::from_secs(15));

        // Simulate: last event was 10 seconds ago (under threshold)
        let last_event = std::time::Instant::now() - Duration::from_secs(10);

        let should_reconnect = last_event.elapsed() > config.stale_timeout;
        assert!(
            !should_reconnect,
            "Should NOT trigger reconnect when under stale_timeout"
        );
    }

    #[test]
    fn test_trade_stale_timeout_trigger() {
        let config = WebSocketConfig::default().with_trade_stale_timeout(Duration::from_secs(15));

        // Last trade was 20 seconds ago
        let last_trade_event = std::time::Instant::now() - Duration::from_secs(20);

        // This is the check from the actual code
        let should_reconnect = last_trade_event.elapsed() > config.trade_stale_timeout;
        assert!(
            should_reconnect,
            "Should trigger reconnect when no trades for > trade_stale_timeout"
        );
    }

    #[test]
    fn test_lag_stale_duration_accumulation() {
        use std::time::Instant;

        let config = WebSocketConfig::default()
            .with_lag_stale_threshold(Duration::from_secs(15))
            .with_lag_stale_duration(Duration::from_secs(10));

        // Simulate lag that exceeds threshold
        let lag_ms: i64 = 20_000; // 20 seconds lag
        let threshold_ms = config.lag_stale_threshold.as_millis() as i64;

        // First check: lag exceeds threshold, start timer
        let mut lag_breach_since: Option<Instant> = None;
        if lag_ms >= threshold_ms && lag_breach_since.is_none() {
            lag_breach_since = Some(Instant::now());
        }
        assert!(
            lag_breach_since.is_some(),
            "Should start lag breach timer when threshold exceeded"
        );

        // Simulate: breach has been ongoing for 5 seconds (under duration)
        // In real code, we'd wait; here we just check the logic
        let breach_start = Instant::now() - Duration::from_secs(5);
        let should_reconnect_5s = breach_start.elapsed() >= config.lag_stale_duration;
        assert!(
            !should_reconnect_5s,
            "Should NOT reconnect after only 5s of lag (duration is 10s)"
        );

        // Simulate: breach has been ongoing for 12 seconds (over duration)
        let breach_start_12s = Instant::now() - Duration::from_secs(12);
        let should_reconnect_12s = breach_start_12s.elapsed() >= config.lag_stale_duration;
        assert!(
            should_reconnect_12s,
            "Should reconnect after 12s of lag (duration is 10s)"
        );
    }

    #[test]
    fn test_lag_breach_resets_when_lag_recovers() {
        let config = WebSocketConfig::default().with_lag_stale_threshold(Duration::from_secs(15));

        let threshold_ms = config.lag_stale_threshold.as_millis() as i64;

        // Start with lag breach
        let mut lag_breach_since: Option<std::time::Instant> = Some(std::time::Instant::now());

        // Simulate: lag recovers (now only 5 seconds)
        let recovered_lag_ms: i64 = 5_000;
        if recovered_lag_ms < threshold_ms {
            lag_breach_since = None; // Reset
        }

        assert!(
            lag_breach_since.is_none(),
            "Lag breach should reset when lag recovers"
        );
    }

    #[test]
    fn test_reconnect_delay_configuration() {
        let config = WebSocketConfig::default().with_reconnect_delay(Duration::from_secs(5));

        assert_eq!(
            config.reconnect_delay,
            Duration::from_secs(5),
            "Reconnect delay should be configurable"
        );

        // Default is 2 seconds
        let default_config = WebSocketConfig::default();
        assert_eq!(
            default_config.reconnect_delay,
            Duration::from_secs(2),
            "Default reconnect delay should be 2 seconds"
        );
    }

    #[test]
    fn test_all_stale_conditions_combined() {
        // Comprehensive test of all three stale conditions

        let config = WebSocketConfig::default()
            .with_stale_timeout(Duration::from_secs(15))
            .with_trade_stale_timeout(Duration::from_secs(15))
            .with_lag_stale_threshold(Duration::from_secs(15))
            .with_lag_stale_duration(Duration::from_secs(10));

        // Condition 1: General stale (no events at all)
        let last_event = std::time::Instant::now() - Duration::from_secs(20);
        let general_stale = last_event.elapsed() > config.stale_timeout;

        // Condition 2: Trade stale (no trade events)
        let last_trade = std::time::Instant::now() - Duration::from_secs(20);
        let trade_stale = last_trade.elapsed() > config.trade_stale_timeout;

        // Condition 3: Lag stale (trades arriving but with high lag for too long)
        let lag_breach_start = std::time::Instant::now() - Duration::from_secs(12);
        let lag_stale = lag_breach_start.elapsed() >= config.lag_stale_duration;

        // Any of these should trigger reconnect
        let should_reconnect = general_stale || trade_stale || lag_stale;
        assert!(
            should_reconnect,
            "At least one stale condition should trigger reconnect"
        );

        // Verify each individually
        assert!(general_stale, "General stale should be true");
        assert!(trade_stale, "Trade stale should be true");
        assert!(lag_stale, "Lag stale should be true");
    }
}
