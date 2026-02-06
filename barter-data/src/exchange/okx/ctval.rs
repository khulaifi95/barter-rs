//! OKX contract value (ctVal) helpers.
//!
//! OKX perpetual swaps report size in contracts. Convert to base units with ctVal.

use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use tracing::warn;

static DYNAMIC_CTVAL_F64: OnceLock<HashMap<String, f64>> = OnceLock::new();
static DYNAMIC_CTVAL_DEC: OnceLock<HashMap<String, Decimal>> = OnceLock::new();

/// Set dynamic ctVal values fetched at startup.
/// Keys are OKX instrument ids (e.g., "BTC-USDT-SWAP").
pub fn set_dynamic_ctval(f64_map: HashMap<String, f64>, dec_map: HashMap<String, Decimal>) {
    if DYNAMIC_CTVAL_F64.set(f64_map).is_err() {
        warn!("OKX ctVal dynamic map already set (f64); ignoring update");
    }
    if DYNAMIC_CTVAL_DEC.set(dec_map).is_err() {
        warn!("OKX ctVal dynamic map already set (Decimal); ignoring update");
    }
}

/// Return the ctVal multiplier for a given OKX instrument id.
/// Only applies to SWAP instruments (perpetuals). Spot remains 1.0.
pub fn ctval_multiplier_f64(inst_id: &str) -> f64 {
    let upper = inst_id.to_uppercase();
    if !upper.ends_with("-SWAP") {
        return 1.0;
    }
    let base = upper.split('-').next().unwrap_or("");
    if let Some(override_val) = ctval_overrides_f64().get(base) {
        return *override_val;
    }
    if let Some(dynamic_val) = dynamic_ctval_f64(&upper) {
        return dynamic_val;
    }
    match base {
        "BTC" => 0.01,
        "ETH" => 0.1,
        "SOL" => 1.0,
        _ => {
            warn_unknown_ctval_base(base);
            1.0
        }
    }
}

/// Decimal variant of ctVal multiplier.
pub fn ctval_multiplier_dec(inst_id: &str) -> Decimal {
    let upper = inst_id.to_uppercase();
    if !upper.ends_with("-SWAP") {
        return Decimal::ONE;
    }
    let base = upper.split('-').next().unwrap_or("");
    if let Some(override_val) = ctval_overrides_dec().get(base) {
        return *override_val;
    }
    if let Some(dynamic_val) = dynamic_ctval_dec(&upper) {
        return dynamic_val;
    }
    match base {
        "BTC" => Decimal::new(1, 2), // 0.01
        "ETH" => Decimal::new(1, 1), // 0.1
        "SOL" => Decimal::ONE,
        _ => {
            warn_unknown_ctval_base(base);
            Decimal::ONE
        }
    }
}

fn warn_unknown_ctval_base(base: &str) {
    static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let warned = WARNED.get_or_init(|| Mutex::new(HashSet::new()));
    let mut guard = warned.lock().unwrap();
    if guard.insert(base.to_string()) {
        warn!(
            "OKX ctVal multiplier not configured for base={} (defaulting to 1.0). \
             Set OKX_CTVAL_{} to override.",
            base, base
        );
    }
}

fn ctval_overrides_f64() -> &'static HashMap<String, f64> {
    static OVERRIDES: OnceLock<HashMap<String, f64>> = OnceLock::new();
    OVERRIDES.get_or_init(|| {
        let mut map = HashMap::new();
        for (key, val) in std::env::vars() {
            if let Some(base) = key.strip_prefix("OKX_CTVAL_")
                && let Ok(parsed) = val.parse::<f64>()
            {
                map.insert(base.to_uppercase(), parsed);
            }
        }
        map
    })
}

fn ctval_overrides_dec() -> &'static HashMap<String, Decimal> {
    static OVERRIDES: OnceLock<HashMap<String, Decimal>> = OnceLock::new();
    OVERRIDES.get_or_init(|| {
        let mut map = HashMap::new();
        for (key, val) in std::env::vars() {
            if let Some(base) = key.strip_prefix("OKX_CTVAL_")
                && let Ok(parsed) = val.parse::<Decimal>()
            {
                map.insert(base.to_uppercase(), parsed);
            }
        }
        map
    })
}

fn dynamic_ctval_f64(inst_id: &str) -> Option<f64> {
    DYNAMIC_CTVAL_F64
        .get()
        .and_then(|map| map.get(inst_id).copied())
}

fn dynamic_ctval_dec(inst_id: &str) -> Option<Decimal> {
    DYNAMIC_CTVAL_DEC
        .get()
        .and_then(|map| map.get(inst_id).copied())
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
