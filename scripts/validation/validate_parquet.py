#!/usr/bin/env python3
"""
Barter Data Server - Parquet Validation Script
Validates data quality, consistency, and optional API parity checks.
"""

import sys
from pathlib import Path
import argparse
import json
import urllib.request
import urllib.parse
from datetime import datetime, timezone

def main():
    parser = argparse.ArgumentParser(description="Validate Barter parquet data.")
    parser.add_argument("parquet_dir", help="Parquet output directory")
    parser.add_argument("--api", action="store_true", help="Enable Binance API parity checks")
    parser.add_argument("--api-symbol", default="BTCUSDT", help="Binance symbol for API parity (default BTCUSDT)")
    args = parser.parse_args()

    parquet_dir = Path(args.parquet_dir)

    try:
        import pyarrow.parquet as pq
        import pandas as pd
    except ImportError:
        print("ERROR: pyarrow not installed. Run: pip install pyarrow pandas")
        sys.exit(1)

    print("=" * 60)
    print("BARTER PARQUET VALIDATION")
    print("=" * 60)
    print(f"Directory: {parquet_dir}")
    print()

    # Find all parquet files
    extended_bars = list(parquet_dir.glob("**/extended_bars_1m/**/*.parquet"))
    bars = list(parquet_dir.glob("**/bars_1m/**/*.parquet"))
    trades = list(parquet_dir.glob("**/trades/**/*.parquet"))

    print(f"Found files:")
    print(f"  - extended_bars_1m: {len(extended_bars)}")
    print(f"  - bars_1m: {len(bars)}")
    print(f"  - trades: {len(trades)}")
    print()

    if not extended_bars and not bars:
        print("ERROR: No parquet files found!")
        sys.exit(1)

    # Validate extended bars
    if extended_bars:
        print("=" * 60)
        print("EXTENDED BARS VALIDATION")
        print("=" * 60)

        all_dfs = []
        for f in extended_bars:
            try:
                df = pq.read_table(f).to_pandas()
                all_dfs.append(df)
            except Exception as e:
                print(f"  ERROR reading {f}: {e}")

        if all_dfs:
            df = pd.concat(all_dfs, ignore_index=True)
            print(f"Total rows: {len(df)}")
            print()

            # Ensure ordering for time-series checks
            if "ts_event" in df.columns:
                df = df.sort_values(["instrument_id", "ts_event"])

            # Group by instrument
            if 'instrument_id' in df.columns:
                for inst, group in df.groupby('instrument_id'):
                    print(f"--- {inst} ---")
                    print(f"  Rows: {len(group)}")

                    # Check L1 data
                    if 'bid_price' in group.columns:
                        bid_ok = (group['bid_price'] > 0).sum()
                        ask_ok = (group['ask_price'] > 0).sum()
                        print(f"  L1 (bid>0): {bid_ok}/{len(group)} {'✅' if bid_ok == len(group) else '⚠️'}")
                        print(f"  L1 (ask>0): {ask_ok}/{len(group)} {'✅' if ask_ok == len(group) else '⚠️'}")

                    # Check trades
                    if 'volume' in group.columns:
                        vol_ok = (group['volume'] > 0).sum()
                        print(f"  Trades (vol>0): {vol_ok}/{len(group)} {'✅' if vol_ok > 0 else '⚠️'}")

                    # Check delta
                    if 'delta' in group.columns and 'buy_volume' in group.columns:
                        delta_match = (group['delta'] == group['buy_volume'] - group['sell_volume']).all()
                        print(f"  Delta check: {'✅ PASS' if delta_match else '❌ FAIL'}")

                    # Check CVD continuity: cvd_t = cvd_{t-1} + delta
                    if 'cvd' in group.columns and 'delta' in group.columns:
                        prev_cvd = group['cvd'].shift(1)
                        cvd_match = (group['cvd'] == (prev_cvd + group['delta'])) | prev_cvd.isna()
                        print(f"  CVD continuity: {'✅ PASS' if cvd_match.all() else '❌ FAIL'}")

                    # Check OI
                    if 'open_interest' in group.columns:
                        oi_ok = (group['open_interest'] > 0).sum()
                        print(f"  OI (>0): {oi_ok}/{len(group)} {'✅' if oi_ok > 0 else '⚠️'}")

                    # Check OI change continuity (only for contiguous 1m bars)
                    if 'open_interest' in group.columns and 'oi_change' in group.columns and 'ts_event' in group.columns:
                        prev_oi = group['open_interest'].shift(1)
                        prev_ts = group['ts_event'].shift(1)
                        contiguous = (group['ts_event'] - prev_ts) == 60_000_000_000
                        oi_match = (group['oi_change'] == (group['open_interest'] - prev_oi)) | ~contiguous
                        print(f"  OI change continuity: {'✅ PASS' if oi_match.all() else '❌ FAIL'}")

                    # Check funding
                    if 'funding_rate' in group.columns:
                        fr_set = (group['funding_rate'] != 0).sum()
                        print(f"  Funding set: {fr_set}/{len(group)}")

                    # Check liquidations
                    if 'liq_total_usd' in group.columns:
                        liq_sum = group['liq_total_usd'].sum()
                        liq_match = (group['liq_total_usd'] == group['liq_buy_usd'] + group['liq_sell_usd']).all()
                        print(f"  Liquidations: ${liq_sum:,.0f} total {'✅' if liq_match else '❌'}")

                    # Check spread
                    if 'spread_bps' in group.columns:
                        spread_ok = (group['spread_bps'] >= 0).all()
                        print(f"  Spread ≥0: {'✅' if spread_ok else '❌'}")

                    # Check book imbalance
                    if 'book_imbalance' in group.columns:
                        imb_ok = ((group['book_imbalance'] >= -1) & (group['book_imbalance'] <= 1)).all()
                        print(f"  Imbalance [-1,1]: {'✅' if imb_ok else '❌'}")

                    # Check depth band monotonicity
                    if 'bid_depth_10bps_usd' in group.columns:
                        bid_ok = (group['bid_depth_10bps_usd'] <= group['bid_depth_50bps_usd']) & \
                                 (group['bid_depth_50bps_usd'] <= group['bid_depth_100bps_usd'])
                        ask_ok = (group['ask_depth_10bps_usd'] <= group['ask_depth_50bps_usd']) & \
                                 (group['ask_depth_50bps_usd'] <= group['ask_depth_100bps_usd'])
                        print(f"  Depth monotonic (bid): {'✅' if bid_ok.all() else '❌'}")
                        print(f"  Depth monotonic (ask): {'✅' if ask_ok.all() else '❌'}")

                    # Check OHLC integrity
                    if all(c in group.columns for c in ["open", "high", "low", "close"]):
                        ohlc_ok = (group["high"] >= group[["open", "close"]].max(axis=1)) & \
                                  (group["low"] <= group[["open", "close"]].min(axis=1)) & \
                                  (group["high"] >= group["low"])
                        print(f"  OHLC integrity: {'✅' if ohlc_ok.all() else '❌'}")

                    # Check quote_volume vs volume * close (5% tolerance)
                    if all(c in group.columns for c in ["quote_volume", "volume", "close"]):
                        # volume and close are fixed-point (1e9), so quote_volume ≈ (volume*close)/1e9
                        est_qv = (group["volume"] * group["close"]) / 1_000_000_000
                        diff = (group["quote_volume"] - est_qv).abs()
                        tol = group["quote_volume"].abs() * 0.05
                        qv_ok = (diff <= tol) | (group["quote_volume"] == 0)
                        print(f"  Quote vol vs close: {'✅' if qv_ok.all() else '⚠️'}")

                    # Check minute continuity (no gaps)
                    if 'ts_event' in group.columns:
                        prev_ts = group['ts_event'].shift(1)
                        gaps = ((group['ts_event'] - prev_ts) != 60_000_000_000) & prev_ts.notna()
                        print(f"  Minute continuity: {'✅' if not gaps.any() else '⚠️'}")

                    print()

            # Optional API parity check (Binance only)
            if args.api:
                print("=" * 60)
                print("BINANCE API PARITY (TIME-ALIGNED)")
                print("=" * 60)
                api_parity(df, args.api_symbol)

    print("=" * 60)
    print("VALIDATION COMPLETE")
    print("=" * 60)

def api_parity(df, symbol):
    if df.empty:
        print("No data for API parity check.")
        return

    # Only proceed if instrument is Binance PERP
    # instrument_id format: BTCUSDT-PERP.BINANCE
    insts = df["instrument_id"].unique().tolist() if "instrument_id" in df.columns else []
    if not any(inst.endswith(".BINANCE") and "-PERP" in inst for inst in insts):
        print("API parity skipped: no Binance PERP instruments found.")
        return

    # Time range (use ts_event close time; map to kline open time)
    min_ts_ns = int(df["ts_event"].min())
    max_ts_ns = int(df["ts_event"].max())
    min_open_ms = (min_ts_ns // 1_000_000) - 60_000
    max_open_ms = (max_ts_ns // 1_000_000)

    klines = fetch_binance_klines(symbol, min_open_ms, max_open_ms)
    oi_hist = fetch_binance_oi_hist(symbol, min_open_ms, max_open_ms)
    funding = fetch_binance_funding(symbol, min_open_ms, max_open_ms)

    kline_map = {int(k[0]): k for k in klines}
    oi_map = {int(x["timestamp"]): x for x in oi_hist}
    funding_map = {int(x["fundingTime"]): x for x in funding}

    # Compare each bar
    price_mismatch = 0
    oi_mismatch = 0
    funding_seen = 0
    total = 0

    for _, row in df.iterrows():
        if not str(row.get("instrument_id", "")).endswith(".BINANCE"):
            continue
        ts_event_ns = int(row["ts_event"])
        open_ms = (ts_event_ns // 1_000_000) - 60_000
        total += 1

        # Price parity
        if open_ms in kline_map:
            k = kline_map[open_ms]
            k_close = float(k[4])
            bar_close = row["close"] / 1_000_000_000
            if k_close > 0:
                diff_pct = abs(bar_close - k_close) / k_close
                if diff_pct > 0.01:
                    price_mismatch += 1

        # OI parity (use nearest timestamp within minute if exact missing)
        oi_row = oi_map.get(open_ms)
        if oi_row is None:
            # try nearest within 1m window
            oi_row = oi_map.get(open_ms + 60_000)
        if oi_row is not None:
            api_oi = float(oi_row["sumOpenInterest"])
            bar_oi = row["open_interest"] / 1_000_000_000
            if api_oi > 0:
                diff_pct = abs(bar_oi - api_oi) / api_oi
                if diff_pct > 0.01:
                    oi_mismatch += 1

        # Funding parity (non-strict; just confirm we got a recent value)
        # Funding updates every 8h; presence is enough for validation
        if funding_map:
            funding_seen = 1

    print(f"Bars checked: {total}")
    print(f"Price parity >1% mismatches: {price_mismatch}")
    print(f"OI parity >1% mismatches: {oi_mismatch}")
    print(f"Funding data present: {'✅' if funding_seen else '⚠️'}")

def fetch_json(url, params):
    query = urllib.parse.urlencode(params)
    full = f"{url}?{query}"
    req = urllib.request.Request(full, headers={"User-Agent": "barter-validator/1.0"})
    with urllib.request.urlopen(req, timeout=10) as resp:
        return json.loads(resp.read().decode("utf-8"))

def fetch_binance_klines(symbol, start_ms, end_ms):
    return fetch_json(
        "https://fapi.binance.com/fapi/v1/klines",
        {"symbol": symbol, "interval": "1m", "startTime": start_ms, "endTime": end_ms},
    )

def fetch_binance_oi_hist(symbol, start_ms, end_ms):
    return fetch_json(
        "https://fapi.binance.com/futures/data/openInterestHist",
        {"symbol": symbol, "period": "1m", "startTime": start_ms, "endTime": end_ms},
    )

def fetch_binance_funding(symbol, start_ms, end_ms):
    return fetch_json(
        "https://fapi.binance.com/fapi/v1/fundingRate",
        {"symbol": symbol, "startTime": start_ms, "endTime": end_ms},
    )

if __name__ == "__main__":
    main()
