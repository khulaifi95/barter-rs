//! Fixed-point encoding for Nautilus compatibility.
//!
//! Nautilus supports two precision modes:
//! - **Standard**: i64 × 1e9 stored as FixedSizeBinary(8)
//! - **High**: i128 × 1e16 stored as FixedSizeBinary(16)
//!
//! The precision mode must match the Nautilus build you will read with.

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

    /// Byte width for FixedSizeBinary columns.
    pub fn bytes_len(self) -> i32 {
        match self {
            PrecisionMode::Standard => 8,
            PrecisionMode::High => 16,
        }
    }

    /// Fixed-point multiplier for this precision mode.
    pub fn multiplier(self) -> f64 {
        match self {
            PrecisionMode::Standard => 1_000_000_000.0,      // 1e9
            PrecisionMode::High => 10_000_000_000_000_000.0, // 1e16
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
///
/// Returns 8 or 16 bytes (little-endian) depending on `mode`.
#[inline]
pub fn encode_fixed_point(value: f64, mode: PrecisionMode) -> FixedBytes {
    match mode {
        PrecisionMode::Standard => {
            let fixed = scaled_i64(value, mode.multiplier()).unwrap_or(0);
            FixedBytes::B8(fixed.to_le_bytes())
        }
        PrecisionMode::High => {
            let fixed = scaled_i128(value, mode.multiplier()).unwrap_or(0);
            FixedBytes::B16(fixed.to_le_bytes())
        }
    }
}

/// Encode a floating-point value to a fixed-point i128 using the selected precision.
#[inline]
#[allow(dead_code)]
pub fn encode_fixed_point_i128(value: f64, mode: PrecisionMode) -> i128 {
    scaled_i128(value, mode.multiplier()).unwrap_or(0)
}

/// Legacy: Encode a floating-point value to a fixed-point i64 (× 1e9).
/// Only use for extended bar schema (Barter-only, not Nautilus core).
#[inline]
pub fn encode_fixed_point_i64(value: f64) -> i64 {
    scaled_i64(value, 1_000_000_000.0).unwrap_or(0)
}

/// Decode a Nautilus fixed-point value back to f64.
///
/// Supports both 8-byte and 16-byte formats.
#[inline]
#[allow(dead_code)]
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

/// Encode a price with specified precision validation.
///
/// Returns None if the value would overflow i128 after scaling (very unlikely).
#[inline]
#[allow(dead_code)]
pub fn encode_price_checked(value: f64, mode: PrecisionMode) -> Option<FixedBytes> {
    match mode {
        PrecisionMode::Standard => {
            scaled_i64(value, mode.multiplier()).map(|fixed| FixedBytes::B8(fixed.to_le_bytes()))
        }
        PrecisionMode::High => {
            scaled_i128(value, mode.multiplier()).map(|fixed| FixedBytes::B16(fixed.to_le_bytes()))
        }
    }
}

#[inline]
fn scaled_i64(value: f64, multiplier: f64) -> Option<i64> {
    let scaled = value * multiplier;
    if !scaled.is_finite() {
        return None;
    }
    let rounded = scaled.round();
    if rounded > i64::MAX as f64 || rounded < i64::MIN as f64 {
        return None;
    }
    Some(rounded as i64)
}

#[inline]
fn scaled_i128(value: f64, multiplier: f64) -> Option<i128> {
    let scaled = value * multiplier;
    if !scaled.is_finite() {
        return None;
    }
    let rounded = scaled.round();
    let limit = i128::MAX as f64;
    if rounded > limit || rounded < -limit {
        return None;
    }
    Some(rounded as i128)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_roundtrip() {
        let values = [0.0, 1.0, -1.0, 42.123456789, 99999.999999999, 0.000000001];
        for value in values {
            let encoded = encode_fixed_point(value, PrecisionMode::High);
            let decoded = decode_fixed_point(encoded.as_slice());
            assert!(
                (decoded - value).abs() < 1e-9,
                "Roundtrip failed for {}: got {}",
                value,
                decoded
            );
        }
    }

    #[test]
    fn test_encode_returns_16_bytes() {
        let encoded = encode_fixed_point(100000.00, PrecisionMode::High);
        assert_eq!(
            encoded.as_slice().len(),
            16,
            "High-precision requires 16 bytes"
        );
    }

    #[test]
    fn test_encode_typical_btc_price() {
        // BTC at ~100,000 USD
        let price = 100_000.12345678;
        let encoded = encode_fixed_point(price, PrecisionMode::High);
        let decoded = decode_fixed_point(encoded.as_slice());
        assert!((decoded - price).abs() < 1e-8);
    }

    #[test]
    fn test_encode_matches_nautilus() {
        // Verify our encoding matches Nautilus Price(100000.00).raw = 1000000000000000000000
        let price = 100000.00_f64;
        let raw = encode_fixed_point_i128(price, PrecisionMode::High);
        // 100000 * 10^16 = 10^5 * 10^16 = 10^21
        assert_eq!(raw, 1_000_000_000_000_000_000_000_i128);
    }

    #[test]
    fn test_encode_small_quantity() {
        // Small BTC quantity: 0.00001 BTC
        let qty = 0.00001;
        let encoded = encode_fixed_point(qty, PrecisionMode::High);
        let decoded = decode_fixed_point(encoded.as_slice());
        assert!((decoded - qty).abs() < 1e-9);
    }

    #[test]
    fn test_encode_checked_valid() {
        // Normal values should succeed
        let result = encode_price_checked(100000.00, PrecisionMode::High);
        assert!(result.is_some());
    }

    #[test]
    fn test_encode_checked_infinity() {
        // Infinity should fail
        let huge = f64::INFINITY;
        assert!(encode_price_checked(huge, PrecisionMode::High).is_none());
    }
}
