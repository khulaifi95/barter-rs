//! L2 depth band calculations (base + notional).

use barter_data::books::OrderBook;
use rust_decimal::prelude::ToPrimitive;

use super::extended_bar::DepthBands;

/// Compute L2 depth bands (base + notional) from the latest order book snapshot.
pub fn compute_depth_bands(book: &OrderBook) -> Option<DepthBands> {
    let best_bid = book.bids().best()?.price.to_f64()?;
    let best_ask = book.asks().best()?.price.to_f64()?;
    let mid = (best_bid + best_ask) / 2.0;
    if mid <= 0.0 {
        return None;
    }

    fn compute_band(book: &OrderBook, mid: f64, bps: f64) -> (f64, f64, f64, f64, f64) {
        let band = bps / 10_000.0;
        let bid_min = mid * (1.0 - band);
        let ask_max = mid * (1.0 + band);

        let mut bid_base = 0.0;
        let mut bid_usd = 0.0;
        for lvl in book.bids().levels() {
            let price = match lvl.price.to_f64() {
                Some(v) => v,
                None => continue,
            };
            if price < bid_min {
                break;
            }
            let amount = match lvl.amount.to_f64() {
                Some(v) => v,
                None => continue,
            };
            bid_base += amount;
            bid_usd += amount * price;
        }

        let mut ask_base = 0.0;
        let mut ask_usd = 0.0;
        for lvl in book.asks().levels() {
            let price = match lvl.price.to_f64() {
                Some(v) => v,
                None => continue,
            };
            if price > ask_max {
                break;
            }
            let amount = match lvl.amount.to_f64() {
                Some(v) => v,
                None => continue,
            };
            ask_base += amount;
            ask_usd += amount * price;
        }

        let denom = bid_usd + ask_usd;
        let imb = if denom > 0.0 { (bid_usd - ask_usd) / denom } else { 0.0 };
        (bid_base, ask_base, bid_usd, ask_usd, imb)
    }

    let (b10_base, a10_base, b10_usd, a10_usd, i10) = compute_band(book, mid, 10.0);
    let (b50_base, a50_base, b50_usd, a50_usd, i50) = compute_band(book, mid, 50.0);
    let (b100_base, a100_base, b100_usd, a100_usd, i100) = compute_band(book, mid, 100.0);

    Some(DepthBands {
        bid_depth_10bps_base: b10_base,
        ask_depth_10bps_base: a10_base,
        bid_depth_10bps_usd: b10_usd,
        ask_depth_10bps_usd: a10_usd,
        depth_imb_10bps: i10,
        bid_depth_50bps_base: b50_base,
        ask_depth_50bps_base: a50_base,
        bid_depth_50bps_usd: b50_usd,
        ask_depth_50bps_usd: a50_usd,
        depth_imb_50bps: i50,
        bid_depth_100bps_base: b100_base,
        ask_depth_100bps_base: a100_base,
        bid_depth_100bps_usd: b100_usd,
        ask_depth_100bps_usd: a100_usd,
        depth_imb_100bps: i100,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use barter_data::books::{Level, OrderBook};
    use rust_decimal_macros::dec;

    fn make_book() -> OrderBook {
        let bids = vec![
            Level::new(dec!(100.0), dec!(1.0)),
            Level::new(dec!(99.9), dec!(2.0)),
            Level::new(dec!(99.8), dec!(3.0)),
        ];
        let asks = vec![
            Level::new(dec!(100.0), dec!(1.5)),
            Level::new(dec!(100.1), dec!(2.5)),
            Level::new(dec!(100.2), dec!(1.0)),
        ];
        OrderBook::new(1, None, bids, asks)
    }

    #[test]
    fn test_depth_bands_10bps() {
        let book = make_book();
        let bands = compute_depth_bands(&book).expect("bands");

        // mid = 100.0, 10 bps band => bids >= 99.9, asks <= 100.1
        let expected_bid_base = 1.0 + 2.0;
        let expected_ask_base = 1.5 + 2.5;
        let expected_bid_usd = 1.0 * 100.0 + 2.0 * 99.9;
        let expected_ask_usd = 1.5 * 100.0 + 2.5 * 100.1;

        assert!((bands.bid_depth_10bps_base - expected_bid_base).abs() < 1e-9);
        assert!((bands.ask_depth_10bps_base - expected_ask_base).abs() < 1e-9);
        assert!((bands.bid_depth_10bps_usd - expected_bid_usd).abs() < 1e-9);
        assert!((bands.ask_depth_10bps_usd - expected_ask_usd).abs() < 1e-9);
    }
}
