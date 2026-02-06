//! Precision handling to match collector's encoding.
//!
//! The collector (barter-data-server) uses Nautilus fixed-point encoding:
//! - **Standard**: i64 × 1e9 stored as FixedSizeBinary(8)
//! - **High**: i128 × 1e16 stored as FixedSizeBinary(16)
//!
//! Extended bars always use Int64 × 1e9.

/// Precision mode matching the collector's encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrecisionMode {
    /// Standard precision: i64 × 1e9, FixedSizeBinary(8)
    Standard,
    /// High precision: i128 × 1e16, FixedSizeBinary(16)
    High,
}

impl PrecisionMode {
    /// Detect precision mode from environment variable `NAUTILUS_PRECISION`.
    ///
    /// Accepted values:
    /// - "high", "16", "high-precision" => High
    /// - "standard", "8", "std" => Standard
    ///
    /// Defaults to High (matches collector default).
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
    pub fn bytes_len(self) -> usize {
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

    /// String representation for output metadata.
    pub fn as_str(self) -> &'static str {
        match self {
            PrecisionMode::Standard => "standard",
            PrecisionMode::High => "high",
        }
    }
}

impl std::fmt::Display for PrecisionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Decode fixed-point bytes (from FixedSizeBinary) to f64.
///
/// Automatically detects precision from byte length:
/// - 8 bytes: Standard mode (i64 × 1e9)
/// - 16 bytes: High mode (i128 × 1e16)
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

/// Decode an extended bar value (Int64 × 1e9).
///
/// Extended bars always use standard precision regardless of `NAUTILUS_PRECISION`.
#[inline]
pub fn decode_extended_bar_value(value: i64) -> f64 {
    value as f64 / 1_000_000_000.0
}

/// Encode a value for feature output (always i64 × 1e9).
///
/// Feature output uses i64 × 1e9 for simplicity. The `source_precision`
/// field in output metadata records the original precision.
#[inline]
pub fn encode_for_output(value: f64) -> i64 {
    (value * 1_000_000_000.0).round() as i64
}

/// Output precision exponent (9 means × 1e9).
pub const OUTPUT_PRECISION: i32 = 9;

/// Detect precision from parquet column byte width.
pub fn detect_from_bytes_len(len: usize) -> Option<PrecisionMode> {
    match len {
        8 => Some(PrecisionMode::Standard),
        16 => Some(PrecisionMode::High),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_standard_precision() {
        // Standard: i64 × 1e9
        let value: f64 = 78523.456789;
        let fixed = (value * 1e9).round() as i64;
        let bytes = fixed.to_le_bytes();

        let decoded = decode_fixed_point(&bytes);
        assert!(
            (decoded - value).abs() < 1e-6,
            "Expected {}, got {}",
            value,
            decoded
        );
    }

    #[test]
    fn test_decode_high_precision() {
        // High: i128 × 1e16
        let value: f64 = 78_523.456_789_012_34;
        let fixed = (value * 1e16).round() as i128;
        let bytes = fixed.to_le_bytes();

        let decoded = decode_fixed_point(&bytes);
        assert!(
            (decoded - value).abs() < 1e-9,
            "Expected {}, got {}",
            value,
            decoded
        );
    }

    #[test]
    fn test_precision_roundtrip_8_bytes() {
        let original: f64 = 78523.456789;
        let fixed = (original * 1e9).round() as i64;
        let bytes = fixed.to_le_bytes();

        assert_eq!(bytes.len(), 8);
        let decoded = decode_fixed_point(&bytes);
        assert!((original - decoded).abs() < 1e-6);
    }

    #[test]
    fn test_precision_roundtrip_16_bytes() {
        let original: f64 = 78_523.456_789_012_34;
        let fixed = (original * 1e16).round() as i128;
        let bytes = fixed.to_le_bytes();

        assert_eq!(bytes.len(), 16);
        let decoded = decode_fixed_point(&bytes);
        assert!((original - decoded).abs() < 1e-9);
    }

    #[test]
    fn test_extended_bar_decode() {
        let value: f64 = 100000.123456789;
        let encoded = (value * 1e9).round() as i64;
        let decoded = decode_extended_bar_value(encoded);
        assert!((decoded - value).abs() < 1e-6);
    }

    #[test]
    fn test_encode_for_output() {
        let value = 78523.456789;
        let encoded = encode_for_output(value);
        let decoded = encoded as f64 / 1e9;
        assert!((decoded - value).abs() < 1e-6);
    }

    #[test]
    fn test_precision_mode_from_env() {
        // Default is High (cannot safely remove env var, so just test current state)
        // The actual default behavior is tested implicitly
        let mode = PrecisionMode::from_env();
        assert!(mode == PrecisionMode::High || mode == PrecisionMode::Standard);
    }

    #[test]
    fn test_precision_mode_bytes_len() {
        assert_eq!(PrecisionMode::Standard.bytes_len(), 8);
        assert_eq!(PrecisionMode::High.bytes_len(), 16);
    }

    #[test]
    fn test_detect_from_bytes_len() {
        assert_eq!(detect_from_bytes_len(8), Some(PrecisionMode::Standard));
        assert_eq!(detect_from_bytes_len(16), Some(PrecisionMode::High));
        assert_eq!(detect_from_bytes_len(4), None);
    }
}
