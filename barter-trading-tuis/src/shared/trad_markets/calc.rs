//! Correlation, divergence, and lead/lag calculation functions
//!
//! All calculations use bar-to-bar percentage returns.

use std::collections::VecDeque;

/// Pearson correlation coefficient on returns
/// Returns value from -1.0 to +1.0
pub fn calc_correlation(returns_a: &[f64], returns_b: &[f64]) -> Option<f64> {
    if returns_a.len() != returns_b.len() || returns_a.len() < 5 {
        return None;
    }

    let n = returns_a.len() as f64;
    let mean_a: f64 = returns_a.iter().sum::<f64>() / n;
    let mean_b: f64 = returns_b.iter().sum::<f64>() / n;

    let mut cov = 0.0;
    let mut var_a = 0.0;
    let mut var_b = 0.0;

    for i in 0..returns_a.len() {
        let diff_a = returns_a[i] - mean_a;
        let diff_b = returns_b[i] - mean_b;
        cov += diff_a * diff_b;
        var_a += diff_a * diff_a;
        var_b += diff_b * diff_b;
    }

    if var_a < 1e-10 || var_b < 1e-10 {
        return None;
    }

    Some(cov / (var_a.sqrt() * var_b.sqrt()))
}

/// Z-score of current spread vs rolling history
/// High |z| = significant divergence from normal
pub fn calc_divergence_zscore(
    current_spread: f64,
    spread_history: &VecDeque<f64>,
) -> Option<f64> {
    if spread_history.len() < 20 {
        return None;
    }

    let n = spread_history.len() as f64;
    let mean: f64 = spread_history.iter().sum::<f64>() / n;
    let variance: f64 = spread_history.iter()
        .map(|x| (x - mean).powi(2))
        .sum::<f64>() / n;
    let std = variance.sqrt();

    if std < 1e-10 {
        return None;
    }

    Some((current_spread - mean) / std)
}

/// Find which asset leads by testing correlation at different lags
/// Returns (lag_bars, correlation) where positive lag = ES leads BTC
pub fn calc_lead_lag(
    es_returns: &[f64],
    btc_returns: &[f64],
    max_lag: usize,  // 6 bars = 30 seconds
) -> (i32, f64) {
    let mut best_lag = 0i32;
    let mut best_corr = 0.0f64;

    for lag in -(max_lag as i32)..=(max_lag as i32) {
        let corr = if lag < 0 {
            // BTC leads: shift BTC back
            let abs_lag = (-lag) as usize;
            // Guard: ensure both slices are valid
            if abs_lag >= btc_returns.len() || abs_lag >= es_returns.len() { continue; }
            let remaining = es_returns.len().min(btc_returns.len()) - abs_lag;
            if remaining < 5 { continue; }
            calc_correlation(
                &es_returns[abs_lag..abs_lag + remaining],
                &btc_returns[..remaining],
            )
        } else if lag > 0 {
            // ES leads: shift ES back
            let abs_lag = lag as usize;
            // Guard: ensure both slices are valid
            if abs_lag >= es_returns.len() || abs_lag >= btc_returns.len() { continue; }
            let remaining = es_returns.len().min(btc_returns.len()) - abs_lag;
            if remaining < 5 { continue; }
            calc_correlation(
                &es_returns[..remaining],
                &btc_returns[abs_lag..abs_lag + remaining],
            )
        } else {
            calc_correlation(es_returns, btc_returns)
        };

        if let Some(c) = corr {
            if c.abs() > best_corr.abs() {
                best_corr = c;
                best_lag = lag;
            }
        }
    }

    (best_lag, best_corr)
}

/// Convert lag in bars to seconds for display
pub fn lag_to_seconds(lag_bars: i32, bar_duration_secs: u64) -> i32 {
    lag_bars * bar_duration_secs as i32
}

/// Calculate percentage return over a window of bars
pub fn calc_return(bars: &[&super::aggregator::MicroBar]) -> f64 {
    if bars.len() < 2 {
        return 0.0;
    }
    let first = bars[0].close;
    let last = bars[bars.len() - 1].close;
    if first > 0.0 {
        (last - first) / first
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_correlation_perfect_positive() {
        let a = vec![0.01, 0.02, -0.01, 0.03, -0.02];
        let b = vec![0.01, 0.02, -0.01, 0.03, -0.02];
        let corr = calc_correlation(&a, &b).unwrap();
        assert!((corr - 1.0).abs() < 0.0001);
    }

    #[test]
    fn test_correlation_perfect_negative() {
        let a = vec![0.01, 0.02, -0.01, 0.03, -0.02];
        let b = vec![-0.01, -0.02, 0.01, -0.03, 0.02];
        let corr = calc_correlation(&a, &b).unwrap();
        assert!((corr + 1.0).abs() < 0.0001);
    }

    #[test]
    fn test_correlation_insufficient_data() {
        let a = vec![0.01, 0.02];
        let b = vec![0.01, 0.02];
        assert!(calc_correlation(&a, &b).is_none());
    }

    #[test]
    fn test_zscore() {
        let mut history = VecDeque::new();
        for _ in 0..30 {
            history.push_back(0.0);
        }
        // Current spread of 2 std devs should give z ~= 2
        // But with all zeros, std = 0, so it returns None
        assert!(calc_divergence_zscore(0.02, &history).is_none());

        // With variance
        history.clear();
        for i in 0..30 {
            history.push_back((i as f64 - 15.0) * 0.001); // -0.015 to +0.014
        }
        let z = calc_divergence_zscore(0.03, &history);
        assert!(z.is_some());
    }
}
