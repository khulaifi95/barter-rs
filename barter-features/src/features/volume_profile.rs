//! Volume Profile computation with expanding value area algorithm.
//!
//! Computes:
//! - POC (Point of Control): price level with maximum volume
//! - VAH (Value Area High): upper bound of value area
//! - VAL (Value Area Low): lower bound of value area

use std::collections::HashMap;

/// Value area result (POC, VAH, VAL).
#[derive(Debug, Clone, Copy, Default)]
pub struct ValueArea {
    /// Point of Control: price with maximum volume
    pub poc: f64,
    /// Value Area High: upper bound of 70% value area
    pub vah: f64,
    /// Value Area Low: lower bound of 70% value area
    pub val: f64,
}

impl ValueArea {
    /// Check if a price is within the value area.
    pub fn contains(&self, price: f64) -> bool {
        price >= self.val && price <= self.vah
    }

    /// Value area range (VAH - VAL).
    pub fn range(&self) -> f64 {
        self.vah - self.val
    }
}

/// Volume profile accumulator.
#[derive(Debug, Clone, Default)]
pub struct VolumeProfile {
    /// Price bucket -> volume mapping
    levels: HashMap<i64, f64>,
    /// Total volume accumulated
    total: f64,
}

impl VolumeProfile {
    /// Create a new empty volume profile.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add volume at a price bucket.
    pub fn add_volume(&mut self, price_bucket: i64, volume: f64) {
        *self.levels.entry(price_bucket).or_insert(0.0) += volume;
        self.total += volume;
    }

    /// Get total volume.
    pub fn total_volume(&self) -> f64 {
        self.total
    }

    /// Get volume at a specific price bucket.
    pub fn volume_at(&self, price_bucket: i64) -> f64 {
        self.levels.get(&price_bucket).copied().unwrap_or(0.0)
    }

    /// Get the POC (point of control) bucket.
    pub fn poc_bucket(&self) -> Option<i64> {
        self.levels
            .iter()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(bucket, _)| *bucket)
    }

    /// Compute value area using the expanding algorithm.
    ///
    /// Algorithm:
    /// 1. Start with POC
    /// 2. Expand to include price levels until 70% of volume is captured
    /// 3. At each step, add the higher-volume adjacent level
    pub fn compute_value_area(&self, target_pct: f64) -> ValueArea {
        if self.levels.is_empty() {
            return ValueArea::default();
        }

        let poc_bucket = match self.poc_bucket() {
            Some(b) => b,
            None => return ValueArea::default(),
        };

        let target_volume = self.total * target_pct;
        let poc_volume = self.levels.get(&poc_bucket).copied().unwrap_or(0.0);

        let mut va_low = poc_bucket;
        let mut va_high = poc_bucket;
        let mut va_volume = poc_volume;

        // Expand until we reach target volume
        while va_volume < target_volume {
            let vol_above = self.levels.get(&(va_high + 1)).copied().unwrap_or(0.0);
            let vol_below = self.levels.get(&(va_low - 1)).copied().unwrap_or(0.0);

            if vol_above == 0.0 && vol_below == 0.0 {
                break;
            }

            if vol_above >= vol_below {
                va_high += 1;
                va_volume += vol_above;
            } else {
                va_low -= 1;
                va_volume += vol_below;
            }
        }

        // Convert buckets back to prices (assuming bucket = price / bucket_size)
        // Caller is responsible for proper bucket size conversion
        ValueArea {
            poc: poc_bucket as f64,
            vah: va_high as f64,
            val: va_low as f64,
        }
    }

    /// Compute value area with price conversion.
    pub fn compute_value_area_with_price(&self, target_pct: f64, bucket_size: f64) -> ValueArea {
        let va = self.compute_value_area(target_pct);
        ValueArea {
            poc: va.poc * bucket_size,
            vah: va.vah * bucket_size,
            val: va.val * bucket_size,
        }
    }

    /// Clear all volume data.
    pub fn clear(&mut self) {
        self.levels.clear();
        self.total = 0.0;
    }

    /// Get number of price levels.
    pub fn num_levels(&self) -> usize {
        self.levels.len()
    }
}

/// Compute value area from price-volume pairs directly.
///
/// This is a convenience function for one-off calculations.
pub fn compute_value_area(
    price_volumes: &[(f64, f64)],
    bucket_size: f64,
    target_pct: f64,
) -> ValueArea {
    let mut profile = VolumeProfile::new();

    for (price, volume) in price_volumes {
        let bucket = (*price / bucket_size).floor() as i64;
        profile.add_volume(bucket, *volume);
    }

    profile.compute_value_area_with_price(target_pct, bucket_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_profile() {
        let profile = VolumeProfile::new();
        let va = profile.compute_value_area(0.70);
        assert_eq!(va.poc, 0.0);
        assert_eq!(va.vah, 0.0);
        assert_eq!(va.val, 0.0);
    }

    #[test]
    fn test_single_level() {
        let mut profile = VolumeProfile::new();
        profile.add_volume(100, 1000.0);

        let va = profile.compute_value_area(0.70);
        assert_eq!(va.poc, 100.0);
        assert_eq!(va.vah, 100.0);
        assert_eq!(va.val, 100.0);
    }

    #[test]
    fn test_poc_is_max_volume() {
        let mut profile = VolumeProfile::new();
        profile.add_volume(100, 100.0);
        profile.add_volume(101, 500.0); // Max
        profile.add_volume(102, 200.0);

        let va = profile.compute_value_area(0.70);
        assert_eq!(va.poc, 101.0);
    }

    #[test]
    fn test_value_area_70_percent() {
        let mut profile = VolumeProfile::new();

        // Create a distribution where POC is at 100 with 30% volume
        // and adjacent levels have decreasing volume
        profile.add_volume(98, 50.0);
        profile.add_volume(99, 100.0);
        profile.add_volume(100, 300.0); // POC - 30%
        profile.add_volume(101, 200.0);
        profile.add_volume(102, 150.0);
        profile.add_volume(103, 100.0);
        profile.add_volume(104, 100.0);
        // Total: 1000

        let va = profile.compute_value_area(0.70);

        // POC should be at 100
        assert_eq!(va.poc, 100.0);

        // Verify the value area captures roughly 70%
        let va_volume: f64 = profile
            .levels
            .iter()
            .filter(|(bucket, _)| **bucket >= va.val as i64 && **bucket <= va.vah as i64)
            .map(|(_, vol)| vol)
            .sum();

        assert!(
            va_volume / profile.total_volume() >= 0.70,
            "VA should contain at least 70% of volume, got {:.2}%",
            va_volume / profile.total_volume() * 100.0
        );
    }

    #[test]
    fn test_symmetric_distribution() {
        let mut profile = VolumeProfile::new();

        // Symmetric distribution around 100
        profile.add_volume(97, 50.0);
        profile.add_volume(98, 100.0);
        profile.add_volume(99, 200.0);
        profile.add_volume(100, 300.0); // POC
        profile.add_volume(101, 200.0);
        profile.add_volume(102, 100.0);
        profile.add_volume(103, 50.0);

        let va = profile.compute_value_area(0.70);

        // Should be symmetric around POC
        assert_eq!(va.poc, 100.0);
        assert!((va.vah - va.poc).abs() - (va.poc - va.val).abs() <= 1.0);
    }

    #[test]
    fn test_value_area_contains() {
        let va = ValueArea {
            poc: 100.0,
            vah: 105.0,
            val: 95.0,
        };

        assert!(va.contains(100.0));
        assert!(va.contains(95.0));
        assert!(va.contains(105.0));
        assert!(!va.contains(94.9));
        assert!(!va.contains(105.1));
    }

    #[test]
    fn test_value_area_range() {
        let va = ValueArea {
            poc: 100.0,
            vah: 105.0,
            val: 95.0,
        };

        assert_eq!(va.range(), 10.0);
    }

    #[test]
    fn test_compute_value_area_convenience() {
        let prices = vec![
            (100.0, 100.0),
            (150.0, 300.0),
            (200.0, 200.0),
            (250.0, 100.0),
        ];

        let va = compute_value_area(&prices, 50.0, 0.70);

        // POC should be at bucket 3 (150/50 = 3)
        assert!(va.poc >= 100.0 && va.poc <= 200.0);
    }

    #[test]
    fn test_profile_total_volume() {
        let mut profile = VolumeProfile::new();
        profile.add_volume(100, 100.0);
        profile.add_volume(101, 200.0);
        profile.add_volume(102, 300.0);

        assert_eq!(profile.total_volume(), 600.0);
    }

    #[test]
    fn test_profile_clear() {
        let mut profile = VolumeProfile::new();
        profile.add_volume(100, 100.0);
        profile.clear();

        assert_eq!(profile.total_volume(), 0.0);
        assert_eq!(profile.num_levels(), 0);
    }
}
