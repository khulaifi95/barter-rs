//! Profile event detection.
//!
//! Detects significant market structure events:
//! - Session open/close
//! - Initial Balance (IB) completion and breaks
//! - POC and Value Area shifts
//! - Gap detection

use crate::config::ProfileConfig;
use crate::features::tpo::TpoBracket;
use chrono::Utc;

/// Types of profile events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileEventType {
    /// First bracket of a new session
    SessionOpen,
    /// Initial Balance period complete (after bracket B)
    IbComplete,
    /// Price breaks above IB high
    IbBreakUp,
    /// Price breaks below IB low
    IbBreakDown,
    /// POC shifts significantly
    PocShift,
    /// Value Area High shifts significantly
    VahShift,
    /// Value Area Low shifts significantly
    ValShift,
    /// Gap detected in data
    GapDetected,
    /// Last bracket of session
    SessionClose,
}

impl ProfileEventType {
    /// Get string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            ProfileEventType::SessionOpen => "SessionOpen",
            ProfileEventType::IbComplete => "IbComplete",
            ProfileEventType::IbBreakUp => "IbBreakUp",
            ProfileEventType::IbBreakDown => "IbBreakDown",
            ProfileEventType::PocShift => "PocShift",
            ProfileEventType::VahShift => "VahShift",
            ProfileEventType::ValShift => "ValShift",
            ProfileEventType::GapDetected => "GapDetected",
            ProfileEventType::SessionClose => "SessionClose",
        }
    }
}

impl std::fmt::Display for ProfileEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A detected profile event.
#[derive(Debug, Clone)]
pub struct ProfileEvent {
    /// Event timestamp (nanoseconds)
    pub ts_event: i64,
    /// Processing timestamp (nanoseconds)
    pub ts_init: i64,
    /// Instrument identifier
    pub instrument_id: String,
    /// Session identifier
    pub session_id: String,
    /// Event type
    pub event_type: ProfileEventType,

    /// Current volume POC
    pub vol_poc: f64,
    /// Current volume VAH
    pub vol_vah: f64,
    /// Current volume VAL
    pub vol_val: f64,
    /// Current TPO POC
    pub tpo_poc: f64,
    /// Current TPO VAH
    pub tpo_vah: f64,
    /// Current TPO VAL
    pub tpo_val: f64,
    /// Initial Balance high
    pub ib_high: f64,
    /// Initial Balance low
    pub ib_low: f64,

    // Event-specific fields
    /// Break direction ("UP" or "DOWN") for IB breaks
    pub break_direction: Option<String>,
    /// Price at which break occurred
    pub break_price: Option<f64>,
    /// Amount of shift for POC/VA shift events
    pub shift_amount: Option<f64>,
    /// Previous value before shift
    pub previous_value: Option<f64>,

    // Gap-specific fields
    /// Gap start timestamp
    pub gap_start_ts: Option<i64>,
    /// Gap end timestamp
    pub gap_end_ts: Option<i64>,
    /// Number of missing bars
    pub missing_bars: Option<u32>,
}

impl ProfileEvent {
    /// Create a new profile event with basic fields.
    pub fn new(
        ts_event: i64,
        instrument_id: &str,
        session_id: &str,
        event_type: ProfileEventType,
        bracket: &TpoBracket,
    ) -> Self {
        Self {
            ts_event,
            ts_init: Utc::now().timestamp_nanos_opt().unwrap_or(0),
            instrument_id: instrument_id.to_string(),
            session_id: session_id.to_string(),
            event_type,
            vol_poc: bracket.vol_poc,
            vol_vah: bracket.vol_vah,
            vol_val: bracket.vol_val,
            tpo_poc: bracket.tpo_poc,
            tpo_vah: bracket.tpo_vah,
            tpo_val: bracket.tpo_val,
            ib_high: bracket.ib_high,
            ib_low: bracket.ib_low,
            break_direction: None,
            break_price: None,
            shift_amount: None,
            previous_value: None,
            gap_start_ts: None,
            gap_end_ts: None,
            missing_bars: None,
        }
    }

    /// Create a gap event.
    pub fn gap(
        ts_event: i64,
        instrument_id: &str,
        session_id: &str,
        gap_start: i64,
        gap_end: i64,
        missing_bars: u32,
        bracket: &TpoBracket,
    ) -> Self {
        let mut event = Self::new(
            ts_event,
            instrument_id,
            session_id,
            ProfileEventType::GapDetected,
            bracket,
        );
        event.gap_start_ts = Some(gap_start);
        event.gap_end_ts = Some(gap_end);
        event.missing_bars = Some(missing_bars);
        event
    }
}

/// Detector for profile events.
pub struct EventDetector {
    poc_shift_threshold: f64,
    va_shift_threshold: f64,

    // State tracking
    last_poc: Option<f64>,
    last_vah: Option<f64>,
    last_val: Option<f64>,
    ib_high: Option<f64>,
    ib_low: Option<f64>,
    ib_broken_up: bool,
    ib_broken_down: bool,
    current_session: Option<String>,
}

impl EventDetector {
    /// Create a new event detector.
    pub fn new(config: &ProfileConfig) -> Self {
        Self {
            poc_shift_threshold: config.poc_shift_threshold_usd,
            va_shift_threshold: config.va_shift_threshold_usd,
            last_poc: None,
            last_vah: None,
            last_val: None,
            ib_high: None,
            ib_low: None,
            ib_broken_up: false,
            ib_broken_down: false,
            current_session: None,
        }
    }

    /// Create with explicit thresholds.
    pub fn with_thresholds(poc_shift: f64, va_shift: f64) -> Self {
        Self {
            poc_shift_threshold: poc_shift,
            va_shift_threshold: va_shift,
            last_poc: None,
            last_vah: None,
            last_val: None,
            ib_high: None,
            ib_low: None,
            ib_broken_up: false,
            ib_broken_down: false,
            current_session: None,
        }
    }

    /// Reset state for a new session.
    pub fn reset_session(&mut self, session_id: &str) {
        self.last_poc = None;
        self.last_vah = None;
        self.last_val = None;
        self.ib_high = None;
        self.ib_low = None;
        self.ib_broken_up = false;
        self.ib_broken_down = false;
        self.current_session = Some(session_id.to_string());
    }

    /// Process a bracket and detect events.
    pub fn process_bracket(&mut self, bracket: &TpoBracket) -> Vec<ProfileEvent> {
        let mut events = Vec::new();

        // Check for session change
        if self.current_session.as_ref() != Some(&bracket.session_id) {
            // Emit session close for previous session if any
            if self.current_session.is_some() {
                // Note: We'd need the last bracket of previous session for this
                // For now, skip session close events
            }

            self.reset_session(&bracket.session_id);

            // Emit session open
            events.push(ProfileEvent::new(
                bracket.ts_event,
                &bracket.instrument_id,
                &bracket.session_id,
                ProfileEventType::SessionOpen,
                bracket,
            ));
        }

        // Check for IB complete
        if bracket.ib_complete && self.ib_high.is_none() {
            self.ib_high = Some(bracket.ib_high);
            self.ib_low = Some(bracket.ib_low);

            events.push(ProfileEvent::new(
                bracket.ts_event,
                &bracket.instrument_id,
                &bracket.session_id,
                ProfileEventType::IbComplete,
                bracket,
            ));
        }

        // Check for IB breaks (only after IB is complete)
        if let (Some(ib_high), Some(ib_low)) = (self.ib_high, self.ib_low) {
            // Check for upside break
            if !self.ib_broken_up && bracket.bracket_close > ib_high {
                self.ib_broken_up = true;
                let mut event = ProfileEvent::new(
                    bracket.ts_event,
                    &bracket.instrument_id,
                    &bracket.session_id,
                    ProfileEventType::IbBreakUp,
                    bracket,
                );
                event.break_direction = Some("UP".to_string());
                event.break_price = Some(bracket.bracket_close);
                events.push(event);
            }

            // Check for downside break
            if !self.ib_broken_down && bracket.bracket_close < ib_low {
                self.ib_broken_down = true;
                let mut event = ProfileEvent::new(
                    bracket.ts_event,
                    &bracket.instrument_id,
                    &bracket.session_id,
                    ProfileEventType::IbBreakDown,
                    bracket,
                );
                event.break_direction = Some("DOWN".to_string());
                event.break_price = Some(bracket.bracket_close);
                events.push(event);
            }
        }

        // Check for POC shift
        if let Some(last_poc) = self.last_poc {
            let shift = (bracket.vol_poc - last_poc).abs();
            if shift >= self.poc_shift_threshold {
                let mut event = ProfileEvent::new(
                    bracket.ts_event,
                    &bracket.instrument_id,
                    &bracket.session_id,
                    ProfileEventType::PocShift,
                    bracket,
                );
                event.shift_amount = Some(shift);
                event.previous_value = Some(last_poc);
                events.push(event);
            }
        }
        self.last_poc = Some(bracket.vol_poc);

        // Check for VAH shift
        if let Some(last_vah) = self.last_vah {
            let shift = (bracket.vol_vah - last_vah).abs();
            if shift >= self.va_shift_threshold {
                let mut event = ProfileEvent::new(
                    bracket.ts_event,
                    &bracket.instrument_id,
                    &bracket.session_id,
                    ProfileEventType::VahShift,
                    bracket,
                );
                event.shift_amount = Some(shift);
                event.previous_value = Some(last_vah);
                events.push(event);
            }
        }
        self.last_vah = Some(bracket.vol_vah);

        // Check for VAL shift
        if let Some(last_val) = self.last_val {
            let shift = (bracket.vol_val - last_val).abs();
            if shift >= self.va_shift_threshold {
                let mut event = ProfileEvent::new(
                    bracket.ts_event,
                    &bracket.instrument_id,
                    &bracket.session_id,
                    ProfileEventType::ValShift,
                    bracket,
                );
                event.shift_amount = Some(shift);
                event.previous_value = Some(last_val);
                events.push(event);
            }
        }
        self.last_val = Some(bracket.vol_val);

        // Check if this is the last bracket of the day
        if bracket.bracket_index == 47 {
            events.push(ProfileEvent::new(
                bracket.ts_event,
                &bracket.instrument_id,
                &bracket.session_id,
                ProfileEventType::SessionClose,
                bracket,
            ));
        }

        events
    }

    /// Create gap events from detected gaps.
    pub fn create_gap_events(
        &self,
        gaps: &[(i64, i64, u32)],
        instrument_id: &str,
        session_id: &str,
        bracket: &TpoBracket,
    ) -> Vec<ProfileEvent> {
        gaps.iter()
            .map(|(start, end, missing)| {
                ProfileEvent::gap(
                    *end,
                    instrument_id,
                    session_id,
                    *start,
                    *end,
                    *missing,
                    bracket,
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_bracket(index: u8, close: f64, poc: f64, ib_complete: bool) -> TpoBracket {
        TpoBracket {
            ts_event: 1_706_832_000_000_000_000 + (index as i64 * 30 * 60_000_000_000),
            ts_init: 0,
            instrument_id: "BTCUSDT".to_string(),
            session_id: "2024-02-02T00:00:00Z".to_string(),
            session_date: "2024-02-02".to_string(),
            label: crate::features::tpo::bracket_label(index),
            bracket_index: index,
            bracket_start_ts: 0,
            bracket_end_ts: 0,
            bracket_open: close,
            bracket_high: close + 100.0,
            bracket_low: close - 100.0,
            bracket_close: close,
            bracket_volume: 1000.0,
            bracket_buy_volume: 600.0,
            bracket_sell_volume: 400.0,
            bracket_delta: 200.0,
            bracket_trade_count: 100,
            bracket_bar_count: 30,
            bracket_has_gap: false,
            vol_poc: poc,
            vol_vah: poc + 50.0,
            vol_val: poc - 50.0,
            vol_total: 5000.0,
            tpo_poc: poc,
            tpo_vah: poc + 50.0,
            tpo_val: poc - 50.0,
            tpo_total_periods: index as u16 + 1,
            ib_high: if ib_complete { close + 100.0 } else { close },
            ib_low: if ib_complete { close - 100.0 } else { close },
            ib_complete,
            poc_divergence: 0.0,
            session_type: "normal".to_string(),
        }
    }

    #[test]
    fn test_session_open_event() {
        let mut detector = EventDetector::with_thresholds(100.0, 150.0);
        let bracket = make_test_bracket(0, 100000.0, 100000.0, false);

        let events = detector.process_bracket(&bracket);

        assert!(
            events
                .iter()
                .any(|e| e.event_type == ProfileEventType::SessionOpen)
        );
    }

    #[test]
    fn test_ib_complete_event() {
        let mut detector = EventDetector::with_thresholds(100.0, 150.0);

        // First bracket (A)
        let bracket_a = make_test_bracket(0, 100000.0, 100000.0, false);
        detector.process_bracket(&bracket_a);

        // Second bracket (B) - IB completes
        let bracket_b = make_test_bracket(1, 100000.0, 100000.0, true);
        let events = detector.process_bracket(&bracket_b);

        assert!(
            events
                .iter()
                .any(|e| e.event_type == ProfileEventType::IbComplete)
        );
    }

    #[test]
    fn test_ib_break_up() {
        let mut detector = EventDetector::with_thresholds(100.0, 150.0);

        // IB bracket
        let bracket_b = make_test_bracket(1, 100000.0, 100000.0, true);
        detector.process_bracket(&bracket_b);

        // Break above IB high (100100)
        let mut bracket_c = make_test_bracket(2, 100200.0, 100000.0, true);
        bracket_c.ib_high = 100100.0;
        bracket_c.ib_low = 99900.0;

        // Manually set the IB values
        detector.ib_high = Some(100100.0);
        detector.ib_low = Some(99900.0);

        let events = detector.process_bracket(&bracket_c);

        assert!(
            events
                .iter()
                .any(|e| e.event_type == ProfileEventType::IbBreakUp)
        );
    }

    #[test]
    fn test_poc_shift_event() {
        let mut detector = EventDetector::with_thresholds(100.0, 150.0);

        // First bracket
        let bracket1 = make_test_bracket(0, 100000.0, 100000.0, false);
        detector.process_bracket(&bracket1);

        // Second bracket with significant POC shift (> $100)
        let bracket2 = make_test_bracket(1, 100000.0, 100200.0, false);
        let events = detector.process_bracket(&bracket2);

        let poc_shift = events
            .iter()
            .find(|e| e.event_type == ProfileEventType::PocShift);
        assert!(poc_shift.is_some());
        assert_eq!(poc_shift.unwrap().shift_amount, Some(200.0));
    }

    #[test]
    fn test_no_poc_shift_below_threshold() {
        let mut detector = EventDetector::with_thresholds(100.0, 150.0);

        // First bracket
        let bracket1 = make_test_bracket(0, 100000.0, 100000.0, false);
        detector.process_bracket(&bracket1);

        // Second bracket with small POC shift (< $100)
        let bracket2 = make_test_bracket(1, 100000.0, 100050.0, false);
        let events = detector.process_bracket(&bracket2);

        assert!(
            !events
                .iter()
                .any(|e| e.event_type == ProfileEventType::PocShift)
        );
    }

    #[test]
    fn test_session_close_event() {
        let mut detector = EventDetector::with_thresholds(100.0, 150.0);
        let bracket = make_test_bracket(47, 100000.0, 100000.0, true);

        let events = detector.process_bracket(&bracket);

        assert!(
            events
                .iter()
                .any(|e| e.event_type == ProfileEventType::SessionClose)
        );
    }

    #[test]
    fn test_gap_event_creation() {
        let detector = EventDetector::with_thresholds(100.0, 150.0);
        let bracket = make_test_bracket(5, 100000.0, 100000.0, true);

        let gaps = vec![
            (1_706_832_000_000_000_000, 1_706_832_180_000_000_000, 2), // 2 missing bars
        ];

        let events = detector.create_gap_events(&gaps, "BTCUSDT", "2024-02-02T00:00:00Z", &bracket);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, ProfileEventType::GapDetected);
        assert_eq!(events[0].missing_bars, Some(2));
    }

    #[test]
    fn test_event_type_as_str() {
        assert_eq!(ProfileEventType::SessionOpen.as_str(), "SessionOpen");
        assert_eq!(ProfileEventType::IbBreakUp.as_str(), "IbBreakUp");
        assert_eq!(ProfileEventType::GapDetected.as_str(), "GapDetected");
    }
}
