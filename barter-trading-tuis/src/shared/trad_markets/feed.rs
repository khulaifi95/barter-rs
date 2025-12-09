//! WebSocket feed handler for ibkr-bridge (ES/NQ ticks)

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use super::state::TradMarketState;

/// Connection status for ibkr-bridge
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IbkrConnectionStatus {
    Connected,
    Disconnected,
    Reconnecting,
}

/// Messages from ibkr-bridge
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum IbkrMessage {
    #[serde(rename = "tick")]
    Tick {
        symbol: String,
        ts: i64,
        px: f64,
        #[serde(default)]
        sz: f64,
        #[serde(default)]
        bid: Option<f64>,
        #[serde(default)]
        ask: Option<f64>,
    },
    #[serde(rename = "tick_backfill")]
    TickBackfill {
        symbol: String,
        ticks: Vec<TickData>,
    },
    #[serde(rename = "welcome")]
    Welcome {
        #[serde(default)]
        message: Option<String>,
    },
    #[serde(rename = "status")]
    Status {
        #[serde(default)]
        connected: Option<bool>,
    },
}

#[derive(Debug, Deserialize)]
struct TickData {
    ts: i64,
    px: f64,
    #[serde(default)]
    sz: f64,
}

/// Get ibkr-bridge WebSocket URL from environment
fn get_ibkr_ws_url() -> String {
    std::env::var("IBKR_BRIDGE_WS_URL")
        .unwrap_or_else(|_| "ws://127.0.0.1:8765/ws".to_string())
}

/// Spawn ibkr-bridge WebSocket handler
/// Returns a task handle and a status receiver
pub fn spawn_ibkr_feed(
    state: Arc<Mutex<TradMarketState>>,
    status_tx: tokio::sync::watch::Sender<IbkrConnectionStatus>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let url = get_ibkr_ws_url();
        info!("Starting ibkr-bridge feed handler for {}", url);

        loop {
            let _ = status_tx.send(IbkrConnectionStatus::Reconnecting);

            match connect_async(&url).await {
                Ok((ws_stream, _)) => {
                    info!("Connected to ibkr-bridge at {}", url);
                    let _ = status_tx.send(IbkrConnectionStatus::Connected);

                    let (_, mut read) = ws_stream.split();

                    while let Some(msg) = read.next().await {
                        match msg {
                            Ok(Message::Text(text)) => {
                                // Try to parse as IbkrMessage
                                match serde_json::from_str::<IbkrMessage>(&text) {
                                    Ok(ibkr_msg) => {
                                        let mut state_guard = state.lock().await;
                                        match ibkr_msg {
                                            IbkrMessage::Tick { symbol, ts, px, sz, .. } => {
                                                let size = if sz > 0.0 { sz } else { 1.0 };
                                                match symbol.as_str() {
                                                    "ES" => state_guard.update_es_tick(px, size, ts),
                                                    "NQ" => state_guard.update_nq_tick(px, size, ts),
                                                    _ => {}
                                                }
                                            }
                                            IbkrMessage::TickBackfill { symbol, ticks } => {
                                                debug!("Received {} tick backfill for {}", ticks.len(), symbol);
                                                for tick in ticks {
                                                    let size = if tick.sz > 0.0 { tick.sz } else { 1.0 };
                                                    match symbol.as_str() {
                                                        "ES" => state_guard.update_es_tick(tick.px, size, tick.ts),
                                                        "NQ" => state_guard.update_nq_tick(tick.px, size, tick.ts),
                                                        _ => {}
                                                    }
                                                }
                                            }
                                            IbkrMessage::Welcome { .. } => {
                                                debug!("Received welcome from ibkr-bridge");
                                            }
                                            IbkrMessage::Status { .. } => {
                                                debug!("Received status from ibkr-bridge");
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        // Don't spam logs for every unparseable message
                                        debug!("Failed to parse ibkr message: {} - {}", e, &text[..text.len().min(100)]);
                                    }
                                }
                            }
                            Ok(Message::Close(_)) => {
                                warn!("ibkr-bridge connection closed");
                                break;
                            }
                            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {
                                // Heartbeat - handled automatically
                            }
                            Err(e) => {
                                error!("ibkr-bridge error: {}", e);
                                break;
                            }
                            _ => {}
                        }
                    }

                    let _ = status_tx.send(IbkrConnectionStatus::Disconnected);
                }
                Err(e) => {
                    error!("Failed to connect to ibkr-bridge at {}: {}", url, e);
                    let _ = status_tx.send(IbkrConnectionStatus::Disconnected);
                }
            }

            // Wait before reconnecting
            debug!("Waiting 5 seconds before reconnecting to ibkr-bridge...");
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    })
}
