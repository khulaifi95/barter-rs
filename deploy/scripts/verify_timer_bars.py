#!/usr/bin/env python3
"""Verify timer bars output against trade-driven bars."""

import pyarrow.parquet as pq
import os
from datetime import datetime, timezone

NANOS_PER_MIN = 60_000_000_000

# === Verification 2: spot-check the empty bar at 04:19 ===
bars = pq.read_table("/tmp/timer_bars_2026-03-08.parquet").to_pandas()
target_ns = int(datetime(2026, 3, 8, 4, 19, tzinfo=timezone.utc).timestamp()) * 1_000_000_000
match = bars[bars["ts_event"] == target_ns]
print("=== Verification 2: Empty bar at 04:19 ===")
if len(match) == 1:
    row = match.iloc[0]
    dt = datetime.fromtimestamp(int(row["ts_event"]) / 1e9, tz=timezone.utc)
    print(f"  ts_event:    {dt}")
    print(f"  OHLC:        {row['open']:.2f} / {row['high']:.2f} / {row['low']:.2f} / {row['close']:.2f}")
    print(f"  volume:      {row['volume']}")
    print(f"  trade_count: {int(row['trade_count'])}")
    print(f"  has_trades:  {row['has_trades']}")
    ok = True
    if row["volume"] != 0.0:
        print("  FAIL: volume should be 0")
        ok = False
    if row["trade_count"] != 0:
        print("  FAIL: trade_count should be 0")
        ok = False
    if row["has_trades"] != False:
        print("  FAIL: has_trades should be False")
        ok = False
    if not (row["open"] == row["high"] == row["low"] == row["close"]):
        print("  FAIL: OHLC should be equal (forward-filled)")
        ok = False
    if ok:
        print("  PASS: empty bar correctly filled")
else:
    print(f"  FAIL: expected 1 row, got {len(match)}")

# === Verification 3: continuity and comparison ===
print()
print("=== Verification 3: Timer vs Trade-driven bars ===")
total = len(bars)
with_trades = int(bars["has_trades"].sum())
print(f"Timer bars: {total} total, {with_trades} with trades, {total - with_trades} empty")

# Check continuity
ts_sorted = bars["ts_event"].sort_values().values
diffs = (ts_sorted[1:] - ts_sorted[:-1]) / NANOS_PER_MIN
if (diffs == 1.0).all():
    print(f"Continuity: PASS — all {total} bars are exactly 1 minute apart")
else:
    bad = (diffs != 1.0).sum()
    print(f"Continuity: FAIL — {bad} gaps found")

# Compare with trade-driven bars
bars_1m_dir = "/data/parquet/bars_1m/BTCUSDT_PERP_BINANCE_1_MINUTE_LAST_EXTERNAL/2026-03-08"
trade_bar_minutes = set()
for f in os.listdir(bars_1m_dir):
    if not f.endswith(".parquet"):
        continue
    t = pq.read_table(os.path.join(bars_1m_dir, f), columns=["ts_event"])
    for ts in t["ts_event"].to_pylist():
        trade_bar_minutes.add(ts)

timer_minutes = set(int(x) for x in bars["ts_event"].values)
timer_with_trades = set(int(x) for x in bars[bars["has_trades"]]["ts_event"].values)

missing_from_timer = trade_bar_minutes - timer_minutes
extra_in_timer = timer_with_trades - trade_bar_minutes

print(f"Trade-driven bars: {len(trade_bar_minutes)}")
print(f"Timer bars with trades: {len(timer_with_trades)}")
print(f"Trade bars missing from timer: {len(missing_from_timer)}")
print(f"Timer has_trades not in trade-driven: {len(extra_in_timer)}")

if missing_from_timer:
    print("  WARNING: some trade-driven bars not found in timer output")
    for ts in sorted(missing_from_timer)[:3]:
        dt = datetime.fromtimestamp(ts / 1e9, tz=timezone.utc)
        print(f"    missing: {dt}")
elif extra_in_timer:
    for ts in sorted(extra_in_timer)[:3]:
        dt = datetime.fromtimestamp(ts / 1e9, tz=timezone.utc)
        print(f"  Extra timer bar: {dt}")
else:
    print("  MATCH: trade-driven bar set = timer bars with has_trades=True")

timer_empty = bars[~bars["has_trades"]]
print(f"\nTimer fills {len(timer_empty)} gap minute(s) that trade-driven bars skip")
print("These are WS disconnect minutes — now have bars with volume=0, has_trades=False")
