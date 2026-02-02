#!/usr/bin/env python3
"""
Barter Data Server - Parquet Validation Script
Validates data quality and consistency of captured parquet files.
"""

import sys
import os
from pathlib import Path

def main():
    if len(sys.argv) < 2:
        print("Usage: python3 validate_parquet.py <parquet_dir>")
        sys.exit(1)

    parquet_dir = Path(sys.argv[1])

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

                    # Check OI
                    if 'open_interest' in group.columns:
                        oi_ok = (group['open_interest'] > 0).sum()
                        print(f"  OI (>0): {oi_ok}/{len(group)} {'✅' if oi_ok > 0 else '⚠️'}")

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

                    print()

    print("=" * 60)
    print("VALIDATION COMPLETE")
    print("=" * 60)

if __name__ == "__main__":
    main()
