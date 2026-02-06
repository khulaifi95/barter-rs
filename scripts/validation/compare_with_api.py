#!/usr/bin/env python3
"""
Barter Data Accuracy Validator

Compares parquet data against Binance REST API to verify accuracy.
This is the source-of-truth validation for volume, delta, and buy/sell data.

Usage:
    python3 scripts/validation/compare_with_api.py /path/to/parquet_dir

Output:
    - Per-candle comparison with Binance API
    - Error metrics and accuracy score
    - Pass/fail status
"""

import sys
import json
import urllib.request
from pathlib import Path
from datetime import datetime, timezone
import argparse


def fetch_binance_klines(symbol: str, start_ms: int, end_ms: int) -> list:
    """Fetch klines from Binance Futures API."""
    url = f"https://fapi.binance.com/fapi/v1/klines?symbol={symbol}&interval=1m&startTime={start_ms}&endTime={end_ms}"
    req = urllib.request.Request(url, headers={"User-Agent": "barter-validator/1.0"})
    with urllib.request.urlopen(req, timeout=15) as resp:
        return json.loads(resp.read())


def decode_i128(b: bytes) -> int:
    """Decode 16-byte fixed-point binary (i128 little-endian)."""
    low = int.from_bytes(b[:8], 'little', signed=False)
    high = int.from_bytes(b[8:], 'little', signed=True)
    return (high << 64) | low


def load_parquet_bars(parquet_dir: Path) -> dict:
    """Load extended bars from parquet and return as dict keyed by open time."""
    try:
        import pyarrow.parquet as pq
        import pandas as pd
    except ImportError:
        print("ERROR: pyarrow not installed. Run: uv pip install pyarrow pandas")
        sys.exit(1)

    bars_dir = parquet_dir / "extended_bars_1m"
    if not bars_dir.exists():
        print(f"ERROR: No extended_bars_1m directory in {parquet_dir}")
        sys.exit(1)

    dfs = []
    for f in bars_dir.rglob("*.parquet"):
        dfs.append(pq.read_table(str(f)).to_pandas())

    if not dfs:
        print("ERROR: No parquet files found")
        sys.exit(1)

    df = pd.concat(dfs).sort_values("ts_event")

    result = {}
    for _, row in df.iterrows():
        # ts_event is close time, open time is ts_event - 60s
        close_ns = row["ts_event"]
        open_ns = close_ns - 60_000_000_000
        open_ms = open_ns // 1_000_000

        result[open_ms] = {
            "volume": row["volume"] / 1e9,
            "buy_volume": row["buy_volume"] / 1e9,
            "sell_volume": row["sell_volume"] / 1e9,
            "delta": row["delta"] / 1e9,
            "open": row["open"] / 1e9,
            "high": row["high"] / 1e9,
            "low": row["low"] / 1e9,
            "close": row["close"] / 1e9,
        }

    return result


def compare_data(our_data: dict, api_data: list) -> tuple:
    """Compare our data with API data and return metrics."""
    results = []
    total_error = 0
    total_comparisons = 0

    for kline in api_data:
        open_ms = kline[0]

        if open_ms not in our_data:
            continue

        ours = our_data[open_ms]

        # API values
        api_volume = float(kline[5])
        api_buy = float(kline[9])
        api_sell = api_volume - api_buy
        api_delta = api_buy - api_sell
        api_close = float(kline[4])

        # Calculate errors
        vol_err = abs(ours["volume"] - api_volume)
        buy_err = abs(ours["buy_volume"] - api_buy)
        sell_err = abs(ours["sell_volume"] - api_sell)
        delta_err = abs(ours["delta"] - api_delta)
        price_err = abs(ours["close"] - api_close)

        open_time = datetime.fromtimestamp(open_ms / 1000, tz=timezone.utc)

        results.append({
            "time": open_time.strftime("%H:%M"),
            "api_volume": api_volume,
            "our_volume": ours["volume"],
            "vol_err": vol_err,
            "api_buy": api_buy,
            "our_buy": ours["buy_volume"],
            "buy_err": buy_err,
            "api_sell": api_sell,
            "our_sell": ours["sell_volume"],
            "sell_err": sell_err,
            "api_delta": api_delta,
            "our_delta": ours["delta"],
            "delta_err": delta_err,
            "api_close": api_close,
            "our_close": ours["close"],
            "price_err": price_err,
        })

        total_error += vol_err + buy_err + sell_err + delta_err
        total_comparisons += 4

    avg_error = total_error / total_comparisons if total_comparisons > 0 else 0
    return results, avg_error


def print_report(results: list, avg_error: float):
    """Print formatted comparison report."""
    print("=" * 90)
    print("BARTER vs BINANCE API - DATA ACCURACY VALIDATION")
    print("=" * 90)
    print()

    print(f"{'Time':<8} {'Metric':<10} {'API':>12} {'Ours':>12} {'Error':>10} {'Status':>8}")
    print("-" * 90)

    all_pass = True
    for r in results:
        metrics = [
            ("Volume", r["api_volume"], r["our_volume"], r["vol_err"]),
            ("Buy Vol", r["api_buy"], r["our_buy"], r["buy_err"]),
            ("Sell Vol", r["api_sell"], r["our_sell"], r["sell_err"]),
            ("Delta", r["api_delta"], r["our_delta"], r["delta_err"]),
            ("Close", r["api_close"], r["our_close"], r["price_err"]),
        ]

        for metric, api_val, our_val, err in metrics:
            # Determine status based on error percentage
            if api_val != 0:
                err_pct = abs(err / api_val) * 100
            else:
                err_pct = 0 if err == 0 else 100

            if err_pct < 0.1:
                status = "EXACT"
            elif err_pct < 1:
                status = "PASS"
            elif err_pct < 5:
                status = "WARN"
            else:
                status = "FAIL"
                all_pass = False

            print(f"{r['time']:<8} {metric:<10} {api_val:>12.2f} {our_val:>12.2f} {err:>10.4f} {status:>8}")
        print()

    print("=" * 90)
    print(f"Average Error: {avg_error:.4f}")
    print(f"Overall Status: {'PASS' if all_pass else 'FAIL'}")
    print("=" * 90)
    print()
    print("Legend:")
    print("  EXACT  = Error < 0.1% (perfect match)")
    print("  PASS   = Error < 1% (excellent)")
    print("  WARN   = Error 1-5% (acceptable, likely partial candle)")
    print("  FAIL   = Error > 5% (investigate)")
    print()
    print("Note: First/last candles may fail if capture started/stopped mid-minute.")
    print("      This is expected - only complete candles should be EXACT.")
    print()

    return all_pass


def main():
    parser = argparse.ArgumentParser(description="Validate parquet data against Binance API")
    parser.add_argument("parquet_dir", help="Directory containing parquet data")
    parser.add_argument("--symbol", default="BTCUSDT", help="Trading symbol (default: BTCUSDT)")
    args = parser.parse_args()

    parquet_dir = Path(args.parquet_dir)
    if not parquet_dir.exists():
        print(f"ERROR: Directory not found: {parquet_dir}")
        sys.exit(1)

    print(f"Loading parquet data from {parquet_dir}...")
    our_data = load_parquet_bars(parquet_dir)

    if not our_data:
        print("ERROR: No data loaded from parquet")
        sys.exit(1)

    # Get time range
    min_ms = min(our_data.keys())
    max_ms = max(our_data.keys())

    print(f"Fetching Binance API data for {args.symbol}...")
    print(f"Time range: {datetime.fromtimestamp(min_ms/1000, tz=timezone.utc)} to {datetime.fromtimestamp(max_ms/1000, tz=timezone.utc)}")
    print()

    api_data = fetch_binance_klines(args.symbol, min_ms, max_ms + 60000)

    results, avg_error = compare_data(our_data, api_data)

    if not results:
        print("ERROR: No matching candles found for comparison")
        sys.exit(1)

    all_pass = print_report(results, avg_error)

    sys.exit(0 if all_pass else 1)


if __name__ == "__main__":
    main()
