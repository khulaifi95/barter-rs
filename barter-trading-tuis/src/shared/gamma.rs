//! Gamma Calculation Engine
//!
//! Implements the GammaEngine trait for calculating gamma flip prices and
//! identifying significant options walls from Deribit options chain data.
//!
//! # Gamma Flip Calculation
//!
//! The gamma flip price is where net market maker gamma exposure crosses zero.
//! - Above this price: positive gamma (dealers hedge by selling into rallies)
//! - Below this price: negative gamma (dealers accelerate moves)
//!
//! # Wall Identification
//!
//! - Put Wall: Highest OI put strike below current price (support)
//! - Call Wall: Highest OI call strike above current price (resistance)
//! - Notional = strike * open_interest * 100 (options are 100 contracts)

use crate::shared::market_state::{GammaEngine, OptionsChain, Wall};

/// Gamma calculator implementing the GammaEngine trait.
///
/// Calculates gamma flip prices and identifies significant options walls
/// from options chain data.
pub struct GammaCalculator {
    /// Minimum open interest threshold to consider as a wall.
    /// Strikes with OI below this threshold are ignored when identifying walls.
    min_wall_oi: f64,
}

impl Default for GammaCalculator {
    fn default() -> Self {
        Self {
            min_wall_oi: 1000.0, // At least 1000 contracts
        }
    }
}

impl GammaCalculator {
    /// Create a new GammaCalculator with custom minimum wall OI threshold.
    ///
    /// # Arguments
    /// * `min_wall_oi` - Minimum open interest to consider a strike as a wall
    pub fn new(min_wall_oi: f64) -> Self {
        Self { min_wall_oi }
    }

    /// Approximate gamma flip using highest put OI strike below spot.
    ///
    /// This is a simplified approach that uses the strike with the highest
    /// put open interest below the current spot price as an estimate of
    /// the gamma flip level.
    ///
    /// # Arguments
    /// * `chain` - Options chain data
    /// * `spot` - Current spot price
    ///
    /// # Returns
    /// Approximate gamma flip price, or spot * 0.95 if no data available
    pub fn approximate_gamma_flip(&self, chain: &OptionsChain, spot: f64) -> f64 {
        chain
            .contracts
            .iter()
            .filter(|c| !c.is_call && c.strike < spot)
            .max_by(|a, b| {
                a.open_interest
                    .partial_cmp(&b.open_interest)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|c| c.strike)
            .unwrap_or(spot * 0.95)
    }

    /// Calculate gamma flip using net gamma exposure method.
    ///
    /// This is a more sophisticated approach that calculates the cumulative
    /// gamma exposure at each strike and finds where it crosses zero.
    ///
    /// # Dynamic Gamma Flip Logic
    ///
    /// 1. For each option contract, calculate dealer gamma exposure:
    ///    - Calls: Dealers are short gamma when selling calls to retail
    ///    - Puts: Dealers are short gamma when selling puts to retail
    ///    - Net gamma at strike = sum(contract.gamma * contract.open_interest * 100)
    ///
    /// 2. Find price where cumulative gamma exposure = 0
    ///
    /// # Arguments
    /// * `chain` - Options chain data
    /// * `spot` - Current spot price
    ///
    /// # Returns
    /// Calculated gamma flip price, falls back to approximate method if no data
    pub fn calculate_gamma_flip_advanced(&self, chain: &OptionsChain, spot: f64) -> f64 {
        if chain.contracts.is_empty() {
            return spot * 0.95;
        }

        // Only consider strikes within ±30% of spot (realistic gamma flip range)
        let min_strike = spot * 0.70;
        let max_strike = spot * 1.30;

        // Collect unique strikes within range and sort them
        // Note: f64 doesn't implement Hash/Eq, so we use sorting to deduplicate
        let mut strikes: Vec<f64> = chain
            .contracts
            .iter()
            .filter(|c| c.strike >= min_strike && c.strike <= max_strike)
            .map(|c| c.strike)
            .collect();

        if strikes.is_empty() {
            return spot * 0.95;
        }

        strikes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        strikes.dedup_by(|a, b| (*a - *b).abs() < f64::EPSILON);

        // Calculate net gamma at each strike
        // Dealers are typically short gamma (they sold options to retail)
        // Calls: negative gamma exposure (short calls)
        // Puts: positive gamma exposure (short puts - but negative delta, so positive gamma effect)
        let mut strike_gamma: Vec<(f64, f64)> = Vec::new();

        for &strike in &strikes {
            let mut net_gamma = 0.0;

            for contract in &chain.contracts {
                // Only consider contracts within our strike range
                if contract.strike < min_strike || contract.strike > max_strike {
                    continue;
                }
                if (contract.strike - strike).abs() < f64::EPSILON {
                    // Only count contracts that have actual gamma data
                    if contract.gamma <= 0.0 {
                        continue;
                    }
                    let gamma_exposure = contract.gamma * contract.open_interest * 100.0;

                    if contract.is_call {
                        // Short call gamma (dealers sold calls)
                        net_gamma -= gamma_exposure;
                    } else {
                        // Short put gamma (dealers sold puts)
                        // Puts have positive gamma effect when spot is below strike
                        net_gamma += gamma_exposure;
                    }
                }
            }

            strike_gamma.push((strike, net_gamma));
        }

        // Find the strike where cumulative gamma crosses zero
        let mut cumulative_gamma = 0.0;
        let mut last_positive_strike = strikes[0];
        let mut last_negative_strike = strikes[0];

        for (strike, gamma) in &strike_gamma {
            cumulative_gamma += gamma;

            if cumulative_gamma > 0.0 {
                last_positive_strike = *strike;
            } else if cumulative_gamma < 0.0 {
                last_negative_strike = *strike;
            }
        }

        // If we found a zero crossing, interpolate
        if last_positive_strike != last_negative_strike {
            // Simple midpoint interpolation
            return (last_positive_strike + last_negative_strike) / 2.0;
        }

        // Fall back to approximate method
        self.approximate_gamma_flip(chain, spot)
    }

    /// Find the put wall (support level) below current price.
    ///
    /// The put wall is the strike with the highest put open interest
    /// below the current spot price, meeting the minimum OI threshold.
    ///
    /// # Arguments
    /// * `chain` - Options chain data
    /// * `spot` - Current spot price
    ///
    /// # Returns
    /// Option containing the Wall if found, None if no qualifying strikes
    fn find_put_wall(&self, chain: &OptionsChain, spot: f64) -> Option<Wall> {
        chain
            .contracts
            .iter()
            .filter(|c| !c.is_call && c.strike < spot && c.open_interest >= self.min_wall_oi)
            .max_by(|a, b| {
                a.open_interest
                    .partial_cmp(&b.open_interest)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|c| {
                let distance_pct = ((spot - c.strike) / spot).abs() * 100.0;
                Wall {
                    strike: c.strike,
                    notional_usd: c.strike * c.open_interest * 100.0,
                    distance_pct,
                }
            })
    }

    /// Find the call wall (resistance level) above current price.
    ///
    /// The call wall is the strike with the highest call open interest
    /// above the current spot price, meeting the minimum OI threshold.
    ///
    /// # Arguments
    /// * `chain` - Options chain data
    /// * `spot` - Current spot price
    ///
    /// # Returns
    /// Option containing the Wall if found, None if no qualifying strikes
    fn find_call_wall(&self, chain: &OptionsChain, spot: f64) -> Option<Wall> {
        chain
            .contracts
            .iter()
            .filter(|c| c.is_call && c.strike > spot && c.open_interest >= self.min_wall_oi)
            .max_by(|a, b| {
                a.open_interest
                    .partial_cmp(&b.open_interest)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|c| {
                let distance_pct = ((c.strike - spot) / spot).abs() * 100.0;
                Wall {
                    strike: c.strike,
                    notional_usd: c.strike * c.open_interest * 100.0,
                    distance_pct,
                }
            })
    }
}

impl GammaEngine for GammaCalculator {
    /// Calculate the gamma flip price for the given options chain.
    ///
    /// Uses the dynamic method that accounts for dealer gamma exposure and
    /// IV-adjusted gamma values from the options chain. Falls back to
    /// approximate method if insufficient data.
    ///
    /// # Dynamic Calculation
    /// The gamma values from Deribit already reflect current IV, making
    /// this method responsive to volatility changes.
    ///
    /// # Arguments
    /// * `chain` - Options chain data from Deribit (with IV-adjusted greeks)
    /// * `spot` - Current spot price
    ///
    /// # Returns
    /// Gamma flip price where net dealer gamma exposure crosses zero
    fn calculate_flip(&self, chain: &OptionsChain, spot: f64) -> f64 {
        // Use the advanced method that accounts for dynamic gamma exposure
        // The chain's gamma values already reflect current IV from Deribit
        self.calculate_gamma_flip_advanced(chain, spot)
    }

    /// Find the nearest put wall (support) and call wall (resistance).
    ///
    /// # Arguments
    /// * `chain` - Options chain data from Deribit
    /// * `spot` - Current spot price
    ///
    /// # Returns
    /// Tuple of (put_wall, call_wall) where either can be None if not found
    fn nearest_walls(&self, chain: &OptionsChain, spot: f64) -> (Option<Wall>, Option<Wall>) {
        let put_wall = self.find_put_wall(chain, spot);
        let call_wall = self.find_call_wall(chain, spot);
        (put_wall, call_wall)
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::market_state::OptionContract;

    fn create_test_chain() -> OptionsChain {
        OptionsChain {
            contracts: vec![
                // Puts below spot (95000)
                OptionContract {
                    instrument_name: "BTC-90000-P".to_string(),
                    strike: 90000.0,
                    expiry: 1700000000,
                    is_call: false,
                    open_interest: 5000.0, // Highest put OI
                    mark_iv: 0.5,
                    delta: -0.3,
                    gamma: 0.00001,
                    vega: 50.0,
                },
                OptionContract {
                    instrument_name: "BTC-92000-P".to_string(),
                    strike: 92000.0,
                    expiry: 1700000000,
                    is_call: false,
                    open_interest: 3000.0,
                    mark_iv: 0.5,
                    delta: -0.4,
                    gamma: 0.00001,
                    vega: 50.0,
                },
                OptionContract {
                    instrument_name: "BTC-94000-P".to_string(),
                    strike: 94000.0,
                    expiry: 1700000000,
                    is_call: false,
                    open_interest: 2000.0,
                    mark_iv: 0.5,
                    delta: -0.45,
                    gamma: 0.00001,
                    vega: 50.0,
                },
                // Calls above spot (95000)
                OptionContract {
                    instrument_name: "BTC-96000-C".to_string(),
                    strike: 96000.0,
                    expiry: 1700000000,
                    is_call: true,
                    open_interest: 2500.0,
                    mark_iv: 0.5,
                    delta: 0.45,
                    gamma: 0.00001,
                    vega: 50.0,
                },
                OptionContract {
                    instrument_name: "BTC-98000-C".to_string(),
                    strike: 98000.0,
                    expiry: 1700000000,
                    is_call: true,
                    open_interest: 4000.0, // Highest call OI
                    mark_iv: 0.5,
                    delta: 0.3,
                    gamma: 0.00001,
                    vega: 50.0,
                },
                OptionContract {
                    instrument_name: "BTC-100000-C".to_string(),
                    strike: 100000.0,
                    expiry: 1700000000,
                    is_call: true,
                    open_interest: 3500.0,
                    mark_iv: 0.5,
                    delta: 0.2,
                    gamma: 0.00001,
                    vega: 50.0,
                },
            ],
            timestamp: 1700000000,
        }
    }

    fn create_empty_chain() -> OptionsChain {
        OptionsChain {
            contracts: vec![],
            timestamp: 1700000000,
        }
    }

    fn create_low_oi_chain() -> OptionsChain {
        OptionsChain {
            contracts: vec![
                OptionContract {
                    instrument_name: "BTC-90000-P".to_string(),
                    strike: 90000.0,
                    expiry: 1700000000,
                    is_call: false,
                    open_interest: 500.0, // Below min_wall_oi threshold
                    mark_iv: 0.5,
                    delta: -0.3,
                    gamma: 0.00001,
                    vega: 50.0,
                },
                OptionContract {
                    instrument_name: "BTC-100000-C".to_string(),
                    strike: 100000.0,
                    expiry: 1700000000,
                    is_call: true,
                    open_interest: 500.0, // Below min_wall_oi threshold
                    mark_iv: 0.5,
                    delta: 0.3,
                    gamma: 0.00001,
                    vega: 50.0,
                },
            ],
            timestamp: 1700000000,
        }
    }

    #[test]
    fn test_default_gamma_calculator() {
        let calc = GammaCalculator::default();
        assert_eq!(calc.min_wall_oi, 1000.0);
    }

    #[test]
    fn test_custom_gamma_calculator() {
        let calc = GammaCalculator::new(500.0);
        assert_eq!(calc.min_wall_oi, 500.0);
    }

    #[test]
    fn test_approximate_gamma_flip() {
        let calc = GammaCalculator::default();
        let chain = create_test_chain();
        let spot = 95000.0;

        // Should return highest put OI strike below spot (90000)
        let flip = calc.approximate_gamma_flip(&chain, spot);
        assert_eq!(flip, 90000.0);
    }

    #[test]
    fn test_approximate_gamma_flip_empty_chain() {
        let calc = GammaCalculator::default();
        let chain = create_empty_chain();
        let spot = 95000.0;

        // Should return spot * 0.95 as fallback
        let flip = calc.approximate_gamma_flip(&chain, spot);
        assert_eq!(flip, spot * 0.95);
    }

    #[test]
    fn test_calculate_flip_trait() {
        // This test verifies the dynamic gamma flip calculation via the GammaEngine trait.
        // The advanced method calculates cumulative dealer gamma exposure across strikes
        // and finds where it crosses zero (or uses interpolation).
        let calc = GammaCalculator::default();
        let chain = create_test_chain();
        let spot = 95000.0;

        let flip = calc.calculate_flip(&chain, spot);

        // With the test chain's gamma structure:
        // - Puts (positive dealer gamma): 90000, 92000, 94000
        // - Calls (negative dealer gamma): 96000, 98000, 100000
        // The advanced method finds where cumulative gamma transitions
        // Result depends on OI distribution and gamma values
        assert!(flip > 0.0, "Flip should be positive");
        assert!(
            flip >= 90000.0 && flip <= 100000.0,
            "Flip {} should be within strike range",
            flip
        );
    }

    #[test]
    fn test_find_put_wall() {
        let calc = GammaCalculator::default();
        let chain = create_test_chain();
        let spot = 95000.0;

        let put_wall = calc.find_put_wall(&chain, spot);
        assert!(put_wall.is_some());

        let wall = put_wall.unwrap();
        assert_eq!(wall.strike, 90000.0); // Highest OI put below spot
        assert_eq!(wall.notional_usd, 90000.0 * 5000.0 * 100.0);

        // Distance should be approximately 5.26%
        let expected_distance = ((spot - 90000.0) / spot) * 100.0;
        assert!((wall.distance_pct - expected_distance).abs() < 0.01);
    }

    #[test]
    fn test_find_call_wall() {
        let calc = GammaCalculator::default();
        let chain = create_test_chain();
        let spot = 95000.0;

        let call_wall = calc.find_call_wall(&chain, spot);
        assert!(call_wall.is_some());

        let wall = call_wall.unwrap();
        assert_eq!(wall.strike, 98000.0); // Highest OI call above spot
        assert_eq!(wall.notional_usd, 98000.0 * 4000.0 * 100.0);

        // Distance should be approximately 3.16%
        let expected_distance = ((98000.0 - spot) / spot) * 100.0;
        assert!((wall.distance_pct - expected_distance).abs() < 0.01);
    }

    #[test]
    fn test_nearest_walls() {
        let calc = GammaCalculator::default();
        let chain = create_test_chain();
        let spot = 95000.0;

        let (put_wall, call_wall) = calc.nearest_walls(&chain, spot);

        assert!(put_wall.is_some());
        assert!(call_wall.is_some());

        assert_eq!(put_wall.unwrap().strike, 90000.0);
        assert_eq!(call_wall.unwrap().strike, 98000.0);
    }

    #[test]
    fn test_nearest_walls_empty_chain() {
        let calc = GammaCalculator::default();
        let chain = create_empty_chain();
        let spot = 95000.0;

        let (put_wall, call_wall) = calc.nearest_walls(&chain, spot);

        assert!(put_wall.is_none());
        assert!(call_wall.is_none());
    }

    #[test]
    fn test_nearest_walls_low_oi() {
        let calc = GammaCalculator::default();
        let chain = create_low_oi_chain();
        let spot = 95000.0;

        // With min_wall_oi = 1000, low OI contracts should not qualify
        let (put_wall, call_wall) = calc.nearest_walls(&chain, spot);

        assert!(put_wall.is_none());
        assert!(call_wall.is_none());
    }

    #[test]
    fn test_nearest_walls_low_oi_custom_threshold() {
        let calc = GammaCalculator::new(100.0); // Lower threshold
        let chain = create_low_oi_chain();
        let spot = 95000.0;

        // With min_wall_oi = 100, low OI contracts should now qualify
        let (put_wall, call_wall) = calc.nearest_walls(&chain, spot);

        assert!(put_wall.is_some());
        assert!(call_wall.is_some());
    }

    #[test]
    fn test_wall_notional_calculation() {
        let calc = GammaCalculator::default();
        let chain = create_test_chain();
        let spot = 95000.0;

        let (put_wall, call_wall) = calc.nearest_walls(&chain, spot);

        // Put wall: 90000 strike * 5000 OI * 100 = $45,000,000,000
        let put = put_wall.unwrap();
        assert_eq!(put.notional_usd, 90000.0 * 5000.0 * 100.0);

        // Call wall: 98000 strike * 4000 OI * 100 = $39,200,000,000
        let call = call_wall.unwrap();
        assert_eq!(call.notional_usd, 98000.0 * 4000.0 * 100.0);
    }

    #[test]
    fn test_distance_pct_calculation() {
        let calc = GammaCalculator::default();
        let chain = create_test_chain();
        let spot = 95000.0;

        let (put_wall, call_wall) = calc.nearest_walls(&chain, spot);

        let put = put_wall.unwrap();
        let call = call_wall.unwrap();

        // Put wall at 90000: distance = (95000 - 90000) / 95000 * 100 = 5.263%
        let expected_put_distance = (spot - 90000.0) / spot * 100.0;
        assert!((put.distance_pct - expected_put_distance).abs() < 0.001);

        // Call wall at 98000: distance = (98000 - 95000) / 95000 * 100 = 3.158%
        let expected_call_distance = (98000.0 - spot) / spot * 100.0;
        assert!((call.distance_pct - expected_call_distance).abs() < 0.001);
    }

    #[test]
    fn test_gamma_flip_advanced() {
        let calc = GammaCalculator::default();
        let chain = create_test_chain();
        let spot = 95000.0;

        // Advanced method should return a reasonable value
        let flip = calc.calculate_gamma_flip_advanced(&chain, spot);

        // Should be somewhere between lowest put and highest call strike
        assert!(flip >= 90000.0);
        assert!(flip <= 100000.0);
    }

    #[test]
    fn test_gamma_flip_advanced_empty_chain() {
        let calc = GammaCalculator::default();
        let chain = create_empty_chain();
        let spot = 95000.0;

        // Should fallback to spot * 0.95
        let flip = calc.calculate_gamma_flip_advanced(&chain, spot);
        assert_eq!(flip, spot * 0.95);
    }

    #[test]
    fn test_spot_at_put_wall() {
        // Test when spot is exactly at a put strike
        let calc = GammaCalculator::default();
        let chain = create_test_chain();
        let spot = 90000.0; // At the put wall

        // No puts below spot at this price
        let flip = calc.approximate_gamma_flip(&chain, spot);
        assert_eq!(flip, spot * 0.95);

        let (put_wall, _) = calc.nearest_walls(&chain, spot);
        assert!(put_wall.is_none()); // No puts below 90000
    }

    #[test]
    fn test_spot_at_call_wall() {
        // Test when spot is exactly at a call strike
        let calc = GammaCalculator::default();
        let chain = create_test_chain();
        let spot = 100000.0; // At the highest call wall

        // Only 100000 call exists, no calls above
        let (_, call_wall) = calc.nearest_walls(&chain, spot);
        assert!(call_wall.is_none()); // No calls above 100000
    }

    #[test]
    fn test_gamma_engine_send_sync() {
        // Verify GammaCalculator implements Send + Sync
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GammaCalculator>();
    }

    #[test]
    fn test_multiple_contracts_same_strike() {
        // Test handling of multiple contracts at the same strike
        let chain = OptionsChain {
            contracts: vec![
                OptionContract {
                    instrument_name: "BTC-90000-P-28DEC".to_string(),
                    strike: 90000.0,
                    expiry: 1700000000,
                    is_call: false,
                    open_interest: 2000.0,
                    mark_iv: 0.5,
                    delta: -0.3,
                    gamma: 0.00001,
                    vega: 50.0,
                },
                OptionContract {
                    instrument_name: "BTC-90000-P-27DEC".to_string(),
                    strike: 90000.0,
                    expiry: 1699900000,
                    is_call: false,
                    open_interest: 3000.0, // This one has higher OI
                    mark_iv: 0.5,
                    delta: -0.3,
                    gamma: 0.00001,
                    vega: 50.0,
                },
            ],
            timestamp: 1700000000,
        };

        let calc = GammaCalculator::default();
        let spot = 95000.0;

        // Should pick the one with highest OI
        let (put_wall, _) = calc.nearest_walls(&chain, spot);
        assert!(put_wall.is_some());

        let wall = put_wall.unwrap();
        assert_eq!(wall.strike, 90000.0);
        // Notional based on higher OI contract
        assert_eq!(wall.notional_usd, 90000.0 * 3000.0 * 100.0);
    }

    #[test]
    fn test_nan_handling_in_comparisons() {
        // Test that NaN values don't cause panics
        let chain = OptionsChain {
            contracts: vec![OptionContract {
                instrument_name: "BTC-90000-P".to_string(),
                strike: 90000.0,
                expiry: 1700000000,
                is_call: false,
                open_interest: f64::NAN, // NaN OI
                mark_iv: 0.5,
                delta: -0.3,
                gamma: 0.00001,
                vega: 50.0,
            }],
            timestamp: 1700000000,
        };

        let calc = GammaCalculator::default();
        let spot = 95000.0;

        // Should not panic - that's the main thing we're testing
        let flip = calc.approximate_gamma_flip(&chain, spot);
        // The result is implementation-defined with NaN comparisons,
        // but it should be a valid f64 and not panic
        assert!(flip.is_finite() || flip == spot * 0.95);

        // Test walls with NaN - should not panic
        let (put_wall, call_wall) = calc.nearest_walls(&chain, spot);
        // With NaN OI below threshold (NaN >= 1000.0 is false), wall should be None
        // But behavior is implementation-defined, main goal is no panic
        let _ = (put_wall, call_wall);
    }

    #[test]
    fn test_zero_spot_price() {
        let calc = GammaCalculator::default();
        let chain = create_test_chain();
        let spot = 0.0;

        // Should handle zero spot gracefully (no puts below 0)
        let flip = calc.approximate_gamma_flip(&chain, spot);
        assert_eq!(flip, 0.0); // 0 * 0.95 = 0

        let (put_wall, call_wall) = calc.nearest_walls(&chain, spot);
        assert!(put_wall.is_none()); // No puts below 0
        assert!(call_wall.is_some()); // All calls are above 0
    }

    #[test]
    fn test_very_high_spot_price() {
        let calc = GammaCalculator::default();
        let chain = create_test_chain();
        let spot = 1_000_000.0; // Much higher than all strikes

        // All strikes are below spot
        let flip = calc.approximate_gamma_flip(&chain, spot);
        assert_eq!(flip, 90000.0); // Highest put OI

        let (put_wall, call_wall) = calc.nearest_walls(&chain, spot);
        assert!(put_wall.is_some()); // Puts exist below
        assert!(call_wall.is_none()); // No calls above 1M
    }
}
