//! TPO (Time Price Opportunity) bracket computation.
//!
//! Aggregates 1-minute bars into 30-minute brackets with:
//! - OHLCV, delta, trade count
//! - Bracket labels (A-Z, AA-AV for 48 brackets/day)
//! - Gap detection

use crate::config::TpoConfig;
use crate::error::Result;
use crate::features::volume_profile::{ValueArea, VolumeProfile};
use crate::reader::ExtendedBar;
use chrono::{DateTime, NaiveDate, TimeZone, Timelike, Utc};
use std::collections::HashMap;
use tracing::debug;

/// Number of brackets per day (48 for 30-minute brackets).
pub const BRACKETS_PER_DAY: u8 = 48;

/// Expected bar interval for gap detection (60 seconds = 1 minute).
const EXPECTED_BAR_INTERVAL_NS: i64 = 60_000_000_000;

/// Gap tolerance in nanoseconds (2 seconds).
const GAP_TOLERANCE_NS: i64 = 2_000_000_000;

/// A processed TPO bracket (30-minute period).
#[derive(Debug, Clone)]
pub struct TpoBracket {
    // Timestamps
    pub ts_event: i64,
    pub ts_init: i64,

    // Identifiers
    pub instrument_id: String,
    pub session_id: String,
    pub session_date: String,

    // Bracket info
    pub label: String,
    pub bracket_index: u8,
    pub bracket_start_ts: i64,
    pub bracket_end_ts: i64,

    // OHLCV for this bracket
    pub bracket_open: f64,
    pub bracket_high: f64,
    pub bracket_low: f64,
    pub bracket_close: f64,
    pub bracket_volume: f64,
    pub bracket_buy_volume: f64,
    pub bracket_sell_volume: f64,
    pub bracket_delta: f64,
    pub bracket_trade_count: u32,
    pub bracket_bar_count: u8,
    pub bracket_has_gap: bool,

    // Running volume profile
    pub vol_poc: f64,
    pub vol_vah: f64,
    pub vol_val: f64,
    pub vol_total: f64,

    // Running TPO profile
    pub tpo_poc: f64,
    pub tpo_vah: f64,
    pub tpo_val: f64,
    pub tpo_total_periods: u16,

    // Initial Balance
    pub ib_high: f64,
    pub ib_low: f64,
    pub ib_complete: bool,

    // Analysis
    pub poc_divergence: f64,
    pub session_type: String,
}

/// Generate bracket label from index (0-47).
///
/// - 0-25: A-Z
/// - 26-47: AA-AV
pub fn bracket_label(index: u8) -> String {
    if index < 26 {
        ((b'A' + index) as char).to_string()
    } else {
        format!("A{}", (b'A' + index - 26) as char)
    }
}

/// TPO processor that aggregates bars into brackets.
pub struct TpoProcessor {
    config: TpoConfig,
    price_bucket_size: f64,

    // Session state
    current_session_date: Option<String>,
    session_volume_profile: VolumeProfile,
    session_tpo_counts: HashMap<i64, u16>, // price bucket -> TPO count
    initial_balance_high: f64,
    initial_balance_low: f64,
    session_high: f64,
    session_low: f64,

    // Current bracket state
    current_bracket_bars: Vec<ExtendedBar>,
    current_bracket_index: u8,
    last_bar_ts: Option<i64>,
    gaps_detected: Vec<(i64, i64, u32)>, // (start, end, missing_bars)
}

impl TpoProcessor {
    /// Create a new TPO processor with the given configuration.
    pub fn new(config: TpoConfig) -> Self {
        Self {
            price_bucket_size: config.price_bucket_usd,
            config,
            current_session_date: None,
            session_volume_profile: VolumeProfile::new(),
            session_tpo_counts: HashMap::new(),
            initial_balance_high: f64::MIN,
            initial_balance_low: f64::MAX,
            session_high: f64::MIN,
            session_low: f64::MAX,
            current_bracket_bars: Vec::new(),
            current_bracket_index: 0,
            last_bar_ts: None,
            gaps_detected: Vec::new(),
        }
    }

    /// Process a batch of bars and emit completed brackets.
    pub fn process_bars(
        &mut self,
        bars: &[ExtendedBar],
        instrument_id: &str,
    ) -> Result<Vec<TpoBracket>> {
        let mut brackets = Vec::new();

        for bar in bars {
            // Check for session change
            let bar_date = self.extract_session_date(bar.ts_event);
            if self.current_session_date.as_ref() != Some(&bar_date) {
                // Flush any pending bracket from previous session
                if !self.current_bracket_bars.is_empty()
                    && let Some(bracket) = self.finalize_bracket(instrument_id)?
                {
                    brackets.push(bracket);
                }
                self.start_new_session(&bar_date);
            }

            // Check for gaps
            if let Some(last_ts) = self.last_bar_ts {
                let expected_ts = last_ts + EXPECTED_BAR_INTERVAL_NS;
                let gap = bar.ts_event - expected_ts;
                if gap > GAP_TOLERANCE_NS {
                    let missing_bars = (gap / EXPECTED_BAR_INTERVAL_NS) as u32;
                    self.gaps_detected
                        .push((last_ts, bar.ts_event, missing_bars));
                    debug!(
                        "Gap detected: {} missing bars from {} to {}",
                        missing_bars, last_ts, bar.ts_event
                    );
                }
            }
            self.last_bar_ts = Some(bar.ts_event);

            // Determine which bracket this bar belongs to
            let bar_bracket_index = self.bracket_index_for_timestamp(bar.ts_event);

            // If bar belongs to a new bracket, finalize the current one
            while self.current_bracket_index < bar_bracket_index {
                if let Some(bracket) = self.finalize_bracket(instrument_id)? {
                    brackets.push(bracket);
                }
                self.current_bracket_index += 1;
            }

            // Add bar to current bracket
            self.current_bracket_bars.push(bar.clone());
        }

        Ok(brackets)
    }

    /// Finalize the current session and emit any remaining bracket.
    pub fn finalize_session(&mut self, instrument_id: &str) -> Result<Option<TpoBracket>> {
        if self.current_bracket_bars.is_empty() {
            return Ok(None);
        }
        self.finalize_bracket(instrument_id)
    }

    /// Get gaps detected during processing.
    pub fn get_gaps(&self) -> &[(i64, i64, u32)] {
        &self.gaps_detected
    }

    /// Clear gaps after processing.
    pub fn clear_gaps(&mut self) {
        self.gaps_detected.clear();
    }

    fn start_new_session(&mut self, date: &str) {
        debug!("Starting new session: {}", date);
        self.current_session_date = Some(date.to_string());
        self.session_volume_profile = VolumeProfile::new();
        self.session_tpo_counts.clear();
        self.initial_balance_high = f64::MIN;
        self.initial_balance_low = f64::MAX;
        self.session_high = f64::MIN;
        self.session_low = f64::MAX;
        self.current_bracket_bars.clear();
        self.current_bracket_index = 0;
        self.last_bar_ts = None;
        self.gaps_detected.clear();
    }

    fn finalize_bracket(&mut self, instrument_id: &str) -> Result<Option<TpoBracket>> {
        if self.current_bracket_bars.is_empty() {
            return Ok(None);
        }

        let bars = std::mem::take(&mut self.current_bracket_bars);
        let bracket_index = self.current_bracket_index;
        let session_date = self.current_session_date.clone().unwrap_or_default();

        // Calculate bracket OHLCV
        let bracket_open = bars.first().map(|b| b.open).unwrap_or(0.0);
        let bracket_close = bars.last().map(|b| b.close).unwrap_or(0.0);
        let bracket_high = bars.iter().map(|b| b.high).fold(f64::MIN, f64::max);
        let bracket_low = bars.iter().map(|b| b.low).fold(f64::MAX, f64::min);
        let bracket_volume: f64 = bars.iter().map(|b| b.volume).sum();
        let bracket_buy_volume: f64 = bars.iter().map(|b| b.buy_volume).sum();
        let bracket_sell_volume: f64 = bars.iter().map(|b| b.sell_volume).sum();
        let bracket_delta: f64 = bars.iter().map(|b| b.delta).sum();
        let bracket_trade_count: u32 = bars.iter().map(|b| b.trade_count as u32).sum();
        let bracket_bar_count = bars.len() as u8;

        // Check for gaps within this bracket
        let bracket_has_gap = self.gaps_detected.iter().any(|(start, _end, _)| {
            let bracket_start = self.bracket_start_timestamp(bracket_index, &session_date);
            let bracket_end = bracket_start + (self.config.bracket_minutes as i64 * 60_000_000_000);
            *start >= bracket_start && *start < bracket_end
        });

        // Update session state
        self.session_high = self.session_high.max(bracket_high);
        self.session_low = self.session_low.min(bracket_low);

        // Update Initial Balance (first 2 brackets)
        let ib_brackets = self.config.initial_balance_brackets;
        if bracket_index < ib_brackets {
            self.initial_balance_high = self.initial_balance_high.max(bracket_high);
            self.initial_balance_low = self.initial_balance_low.min(bracket_low);
        }
        let ib_complete = bracket_index >= ib_brackets - 1;

        // Update volume profile with this bracket's volume at each price level
        for bar in &bars {
            let bucket = self.price_to_bucket(bar.close);
            self.session_volume_profile.add_volume(bucket, bar.volume);
        }

        // Update TPO counts (each bracket touches price levels)
        let low_bucket = self.price_to_bucket(bracket_low);
        let high_bucket = self.price_to_bucket(bracket_high);
        for bucket in low_bucket..=high_bucket {
            *self.session_tpo_counts.entry(bucket).or_insert(0) += 1;
        }

        // Compute running value areas (with price conversion)
        let vol_va = self
            .session_volume_profile
            .compute_value_area_with_price(self.config.value_area_pct, self.price_bucket_size);
        let tpo_va = self.compute_tpo_value_area();

        // Timestamps
        let ts_event = bars.last().map(|b| b.ts_event).unwrap_or(0);
        let ts_init = Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let bracket_start_ts = self.bracket_start_timestamp(bracket_index, &session_date);
        let bracket_end_ts =
            bracket_start_ts + (self.config.bracket_minutes as i64 * 60_000_000_000);

        let session_id = format!("{}T00:00:00Z", session_date);

        Ok(Some(TpoBracket {
            ts_event,
            ts_init,
            instrument_id: instrument_id.to_string(),
            session_id,
            session_date,
            label: bracket_label(bracket_index),
            bracket_index,
            bracket_start_ts,
            bracket_end_ts,
            bracket_open,
            bracket_high,
            bracket_low,
            bracket_close,
            bracket_volume,
            bracket_buy_volume,
            bracket_sell_volume,
            bracket_delta,
            bracket_trade_count,
            bracket_bar_count,
            bracket_has_gap,
            vol_poc: vol_va.poc,
            vol_vah: vol_va.vah,
            vol_val: vol_va.val,
            vol_total: self.session_volume_profile.total_volume(),
            tpo_poc: tpo_va.poc,
            tpo_vah: tpo_va.vah,
            tpo_val: tpo_va.val,
            tpo_total_periods: bracket_index as u16 + 1,
            ib_high: if ib_complete {
                self.initial_balance_high
            } else {
                bracket_high
            },
            ib_low: if ib_complete {
                self.initial_balance_low
            } else {
                bracket_low
            },
            ib_complete,
            poc_divergence: vol_va.poc - tpo_va.poc,
            session_type: "normal".to_string(),
        }))
    }

    fn extract_session_date(&self, ts_nanos: i64) -> String {
        let secs = ts_nanos / 1_000_000_000;
        let dt = DateTime::from_timestamp(secs, 0).unwrap_or_default();
        dt.format("%Y-%m-%d").to_string()
    }

    fn bracket_index_for_timestamp(&self, ts_nanos: i64) -> u8 {
        let secs = ts_nanos / 1_000_000_000;
        let dt = DateTime::from_timestamp(secs, 0).unwrap_or_default();
        let minutes_since_midnight = dt.hour() * 60 + dt.minute();
        let bracket = minutes_since_midnight / self.config.bracket_minutes;
        (bracket as u8).min(BRACKETS_PER_DAY - 1)
    }

    fn bracket_start_timestamp(&self, index: u8, date: &str) -> i64 {
        // Parse date and compute start of bracket
        if let Ok(date) = NaiveDate::parse_from_str(date, "%Y-%m-%d") {
            let minutes = index as u32 * self.config.bracket_minutes;
            let hours = minutes / 60;
            let mins = minutes % 60;
            if let Some(dt) = date.and_hms_opt(hours, mins, 0) {
                return Utc
                    .from_utc_datetime(&dt)
                    .timestamp_nanos_opt()
                    .unwrap_or(0);
            }
        }
        0
    }

    fn price_to_bucket(&self, price: f64) -> i64 {
        (price / self.price_bucket_size).floor() as i64
    }

    fn bucket_to_price(&self, bucket: i64) -> f64 {
        bucket as f64 * self.price_bucket_size
    }

    fn compute_tpo_value_area(&self) -> ValueArea {
        if self.session_tpo_counts.is_empty() {
            return ValueArea::default();
        }

        // Find POC (price level with most TPO touches)
        let (poc_bucket, _) = self
            .session_tpo_counts
            .iter()
            .max_by_key(|(_, count)| *count)
            .unwrap();

        let total_tpo: u16 = self.session_tpo_counts.values().sum();
        let target_tpo = (total_tpo as f64 * self.config.value_area_pct) as u16;

        // Expanding value area algorithm
        let mut va_low = *poc_bucket;
        let mut va_high = *poc_bucket;
        let mut va_tpo = *self.session_tpo_counts.get(poc_bucket).unwrap_or(&0);

        while va_tpo < target_tpo {
            let vol_above = self
                .session_tpo_counts
                .get(&(va_high + 1))
                .copied()
                .unwrap_or(0);
            let vol_below = self
                .session_tpo_counts
                .get(&(va_low - 1))
                .copied()
                .unwrap_or(0);

            if vol_above == 0 && vol_below == 0 {
                break;
            }

            if vol_above >= vol_below {
                va_high += 1;
                va_tpo += vol_above;
            } else {
                va_low -= 1;
                va_tpo += vol_below;
            }
        }

        ValueArea {
            poc: self.bucket_to_price(*poc_bucket),
            vah: self.bucket_to_price(va_high),
            val: self.bucket_to_price(va_low),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bracket_label_a_to_z() {
        assert_eq!(bracket_label(0), "A");
        assert_eq!(bracket_label(1), "B");
        assert_eq!(bracket_label(25), "Z");
    }

    #[test]
    fn test_bracket_label_aa_to_av() {
        assert_eq!(bracket_label(26), "AA");
        assert_eq!(bracket_label(27), "AB");
        assert_eq!(bracket_label(47), "AV");
    }

    #[test]
    fn test_bracket_label_all_48() {
        for i in 0..48 {
            let label = bracket_label(i);
            assert!(!label.is_empty());
        }
    }

    fn make_test_bar(ts_event: i64, price: f64, volume: f64) -> ExtendedBar {
        ExtendedBar {
            ts_event,
            ts_init: ts_event,
            ts_open: ts_event - 60_000_000_000, // 1 minute before close
            instrument_id: "TEST".to_string(),
            open: price,
            high: price + 10.0,
            low: price - 10.0,
            close: price,
            volume,
            quote_volume: price * volume,
            trade_count: 100,
            buy_volume: volume * 0.6,
            sell_volume: volume * 0.4,
            delta: volume * 0.2,
            cvd: 0.0,
            open_interest: 0.0,
            oi_change: 0.0,
            funding_rate: 0.0,
        }
    }

    #[test]
    fn test_tpo_processor_single_bracket() {
        let config = TpoConfig::default();
        let mut processor = TpoProcessor::new(config);

        // Create 30 bars (one bracket's worth)
        let base_ts: i64 = 1_706_832_000_000_000_000; // 2024-02-02 00:00:00 UTC
        let bars: Vec<ExtendedBar> = (0..30)
            .map(|i| make_test_bar(base_ts + (i as i64 * 60_000_000_000), 100.0, 10.0))
            .collect();

        let _brackets = processor.process_bars(&bars, "BTCUSDT").unwrap();

        // Should produce one bracket when we finalize
        let final_bracket = processor.finalize_session("BTCUSDT").unwrap();
        assert!(final_bracket.is_some());

        let bracket = final_bracket.unwrap();
        assert_eq!(bracket.label, "A");
        assert_eq!(bracket.bracket_index, 0);
        assert_eq!(bracket.bracket_bar_count, 30);
    }

    #[test]
    fn test_gap_detection() {
        let config = TpoConfig::default();
        let mut processor = TpoProcessor::new(config);

        let base_ts: i64 = 1_706_832_000_000_000_000;
        let bars = vec![
            make_test_bar(base_ts, 100.0, 10.0),
            make_test_bar(base_ts + 60_000_000_000, 100.0, 10.0),
            // Gap: 2 minutes missing
            make_test_bar(base_ts + 240_000_000_000, 100.0, 10.0),
        ];

        processor.process_bars(&bars, "BTCUSDT").unwrap();

        let gaps = processor.get_gaps();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].2, 2); // 2 missing bars
    }
}
