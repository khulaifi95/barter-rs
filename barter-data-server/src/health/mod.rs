//! Health monitoring and heartbeat module.
//!
//! Provides:
//! - Heartbeat JSON file for external monitoring
//! - Health status tracking for data collection

pub mod heartbeat;

#[allow(unused_imports)]
pub use heartbeat::{HeartbeatConfig, HeartbeatWriter, run_heartbeat_task};
