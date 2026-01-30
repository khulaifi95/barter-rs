//! Barter Tools - CLI utilities for data pipeline operations.
//!
//! This crate provides tools for:
//! - Downloading historical data from Binance Public Data
//! - Converting CSV to Nautilus-compatible Parquet
//! - Verifying Parquet file integrity
//! - Detecting gaps in data coverage

pub mod binance;
pub mod convert;
pub mod verify;

/// Precision mode for Nautilus fixed-point encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrecisionMode {
    Standard,
    High,
}

impl PrecisionMode {
    /// Read precision mode from environment variable `NAUTILUS_PRECISION`.
    ///
    /// Accepted values:
    /// - "high", "16", "high-precision" => High
    /// - "standard", "8", "std" => Standard
    ///
    /// Defaults to High.
    pub fn from_env() -> Self {
        match std::env::var("NAUTILUS_PRECISION") {
            Ok(raw) => {
                let v = raw.trim().to_lowercase();
                if matches!(v.as_str(), "standard" | "std" | "8") {
                    PrecisionMode::Standard
                } else {
                    PrecisionMode::High
                }
            }
            Err(_) => PrecisionMode::High,
        }
    }

    pub fn multiplier(self) -> f64 {
        match self {
            PrecisionMode::Standard => 1_000_000_000.0,          // 1e9
            PrecisionMode::High => 10_000_000_000_000_000.0,     // 1e16
        }
    }
}

/// Fixed-point bytes for either precision mode.
pub enum FixedBytes {
    B8([u8; 8]),
    B16([u8; 16]),
}

impl FixedBytes {
    pub fn as_slice(&self) -> &[u8] {
        match self {
            FixedBytes::B8(bytes) => bytes,
            FixedBytes::B16(bytes) => bytes,
        }
    }
}

/// Encode a floating-point value to Nautilus fixed-point bytes.
#[inline]
pub fn encode_fixed_point(value: f64, mode: PrecisionMode) -> FixedBytes {
    match mode {
        PrecisionMode::Standard => {
            let fixed = (value * mode.multiplier()).round() as i64;
            FixedBytes::B8(fixed.to_le_bytes())
        }
        PrecisionMode::High => {
            let fixed = (value * mode.multiplier()).round() as i128;
            FixedBytes::B16(fixed.to_le_bytes())
        }
    }
}

/// Decode a Nautilus fixed-point value back to f64.
#[inline]
pub fn decode_fixed_point(bytes: &[u8]) -> f64 {
    match bytes.len() {
        8 => {
            let fixed = i64::from_le_bytes(bytes.try_into().unwrap_or([0u8; 8]));
            fixed as f64 / 1_000_000_000.0
        }
        16 => {
            let fixed = i128::from_le_bytes(bytes.try_into().unwrap_or([0u8; 16]));
            fixed as f64 / 10_000_000_000_000_000.0
        }
        _ => 0.0,
    }
}
