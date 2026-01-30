//! Barter Data Server Library
//!
//! This library provides modules for real-time market data collection and storage:
//!
//! - `parquet` - Nautilus-compatible Parquet file writing
//! - `aggregator` - 1-minute bar aggregation from trades
//! - `storage` - Local and S3 storage backends
//! - `health` - Health monitoring and heartbeat

pub mod aggregator;
pub mod health;
pub mod parquet;
pub mod storage;
