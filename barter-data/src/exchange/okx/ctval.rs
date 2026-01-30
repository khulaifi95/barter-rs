//! OKX contract value (ctVal) helpers.
//!
//! OKX perpetual swaps report size in contracts. Convert to base units with ctVal.

use rust_decimal::Decimal;

/// Return the ctVal multiplier for a given OKX instrument id.
/// Only applies to SWAP instruments (perpetuals). Spot remains 1.0.
pub fn ctval_multiplier_f64(inst_id: &str) -> f64 {
    let upper = inst_id.to_uppercase();
    if !upper.ends_with("-SWAP") {
        return 1.0;
    }
    match upper.split('-').next().unwrap_or("") {
        "BTC" => 0.01,
        "ETH" => 0.1,
        "SOL" => 1.0,
        _ => 1.0,
    }
}

/// Decimal variant of ctVal multiplier.
pub fn ctval_multiplier_dec(inst_id: &str) -> Decimal {
    let upper = inst_id.to_uppercase();
    if !upper.ends_with("-SWAP") {
        return Decimal::ONE;
    }
    match upper.split('-').next().unwrap_or("") {
        "BTC" => Decimal::new(1, 2), // 0.01
        "ETH" => Decimal::new(1, 1), // 0.1
        "SOL" => Decimal::ONE,
        _ => Decimal::ONE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_ctval_multiplier_f64() {
        assert_eq!(ctval_multiplier_f64("BTC-USDT-SWAP"), 0.01);
        assert_eq!(ctval_multiplier_f64("ETH-USDT-SWAP"), 0.1);
        assert_eq!(ctval_multiplier_f64("SOL-USDT-SWAP"), 1.0);
        assert_eq!(ctval_multiplier_f64("BTC-USDT"), 1.0); // spot
    }

    #[test]
    fn test_ctval_multiplier_dec() {
        assert_eq!(ctval_multiplier_dec("BTC-USDT-SWAP"), dec!(0.01));
        assert_eq!(ctval_multiplier_dec("ETH-USDT-SWAP"), dec!(0.1));
        assert_eq!(ctval_multiplier_dec("SOL-USDT-SWAP"), dec!(1.0));
        assert_eq!(ctval_multiplier_dec("BTC-USDT"), dec!(1.0));
    }
}
