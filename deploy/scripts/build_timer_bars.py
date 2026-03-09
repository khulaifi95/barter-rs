#!/usr/bin/env python3
"""Build continuous timer-driven 1-minute bars from trade parquet data.

Reads trade parquet files and produces a continuous minute grid where
every minute has a bar. Minutes without trades carry forward the last
close price with volume=0 and has_trades=False. Minutes before the
first trade in the range are omitted (no fabricated price history).

Runs inside the jupyter container (requires pyarrow, pandas).

Usage:
    python build_timer_bars.py \\
        --instrument BTCUSDT_PERP_BINANCE \\
        --start 2026-03-01 --end 2026-03-07 \\
        --output /data/tmp/timer_bars.parquet

    # Print summary only (no file written):
    python build_timer_bars.py \\
        --instrument BTCUSDT_PERP_BINANCE \\
        --start 2026-03-08

Schema (output):
    open        float64   OHLC open (or last close if empty)
    high        float64   OHLC high (or last close if empty)
    low         float64   OHLC low  (or last close if empty)
    close       float64   OHLC close (or last close if empty)
    volume      float64   Total trade volume in the minute
    trade_count int64     Number of trades in the minute
    has_trades  bool      True if at least one trade occurred
    ts_event    uint64    Minute close boundary (nanoseconds, UTC)

Nautilus convention: ts_event for the 17:00-17:01 bar = 17:01:00.000 UTC.
"""

import argparse
import os
import sys
from datetime import datetime, date, timedelta, timezone

import numpy as np
import pandas as pd
import pyarrow as pa
import pyarrow.parquet as pq

NAUTILUS_MULTIPLIER = 1e16  # High-precision mode: i128 × 1e16
NANOS_PER_SEC = 1_000_000_000
NANOS_PER_MIN = 60 * NANOS_PER_SEC
TRADES_BASE = "/data/parquet/trades"


def decode_nautilus_column(binary_col):
    """Decode a column of Nautilus fixed_size_binary[16] to float64 array."""
    values = binary_col.to_pylist()
    result = np.empty(len(values), dtype=np.float64)
    for i, b in enumerate(values):
        raw = int.from_bytes(b, byteorder="little", signed=True)
        result[i] = raw / NAUTILUS_MULTIPLIER
    return result


def load_trades_for_day(instrument, day_str):
    """Load all trades for a single day, return DataFrame with decoded columns."""
    day_dir = os.path.join(TRADES_BASE, instrument, day_str)
    if not os.path.isdir(day_dir):
        return None

    files = sorted(f for f in os.listdir(day_dir) if f.endswith(".parquet"))
    if not files:
        return None

    tables = []
    for f in files:
        try:
            t = pq.read_table(
                os.path.join(day_dir, f),
                columns=["price", "size", "ts_event"],
            )
            tables.append(t)
        except Exception as e:
            print(f"  WARNING: skipping {f}: {e}", file=sys.stderr)

    if not tables:
        return None

    combined = pa.concat_tables(tables)

    df = pd.DataFrame({
        "price": decode_nautilus_column(combined["price"]),
        "volume": decode_nautilus_column(combined["size"]),
        "ts_event": combined["ts_event"].to_pandas(),
    })
    return df


def build_timer_bars(instrument, start_date, end_date):
    """Build continuous timer-driven 1-minute bars for a date range.

    Returns a DataFrame with one row per minute from the first trade
    to the end of the range, with empty bars filled forward.
    """
    all_trades = []
    current = start_date
    while current <= end_date:
        day_str = current.strftime("%Y-%m-%d")
        print(f"  Loading trades for {day_str}...", file=sys.stderr)
        df = load_trades_for_day(instrument, day_str)
        if df is not None:
            all_trades.append(df)
            print(f"    {len(df):,} trades loaded", file=sys.stderr)
        else:
            print(f"    no trade data", file=sys.stderr)
        current += timedelta(days=1)

    if not all_trades:
        print("ERROR: no trade data found in range", file=sys.stderr)
        return None

    trades = pd.concat(all_trades, ignore_index=True)
    trades.sort_values("ts_event", inplace=True)

    # Assign each trade to its minute close boundary.
    # A trade at 17:00:30 belongs to the bar closing at 17:01:00.
    # minute_close = ceil(ts_event to next minute boundary)
    trades["minute_close_ns"] = (
        (trades["ts_event"] // NANOS_PER_MIN + 1) * NANOS_PER_MIN
    )

    # Aggregate trades per minute
    grouped = trades.groupby("minute_close_ns").agg(
        open=("price", "first"),
        high=("price", "max"),
        low=("price", "min"),
        close=("price", "last"),
        volume=("volume", "sum"),
        trade_count=("price", "count"),
    ).reset_index()

    # Build the continuous minute grid.
    #
    # Nautilus convention: ts_event is the minute close boundary.
    # The bar closing at 00:01 covers trades from 00:00:00 to 00:00:59.
    # So a full day has bars from 00:01 (first close) to 00:00+1d (last close).
    # That's exactly 1440 bars per day.
    #
    # Clamp to the requested date range so boundary trades don't bleed in.
    day_start_ns = int(
        datetime(start_date.year, start_date.month, start_date.day,
                 tzinfo=timezone.utc).timestamp()
    ) * NANOS_PER_SEC + NANOS_PER_MIN  # 00:01:00 = first close boundary

    end_next = end_date + timedelta(days=1)
    day_end_ns = int(
        datetime(end_next.year, end_next.month, end_next.day,
                 tzinfo=timezone.utc).timestamp()
    ) * NANOS_PER_SEC  # 00:00:00 next day = last close boundary

    # Filter grouped bars to the date range
    grouped = grouped[
        (grouped["minute_close_ns"] >= day_start_ns) &
        (grouped["minute_close_ns"] <= day_end_ns)
    ]

    if grouped.empty:
        print("ERROR: no bars in requested date range", file=sys.stderr)
        return None

    # Start grid from first minute with trades (skip pre-trade minutes)
    first_trade_minute = int(grouped["minute_close_ns"].min())
    grid_start = first_trade_minute
    grid_end = day_end_ns

    grid = pd.DataFrame({
        "minute_close_ns": np.arange(grid_start, grid_end + 1, NANOS_PER_MIN,
                                     dtype=np.int64),
    })

    # Merge trades onto grid
    bars = grid.merge(grouped, on="minute_close_ns", how="left")

    # Fill forward close prices for empty minutes
    bars["has_trades"] = bars["close"].notna()
    bars["close"] = bars["close"].ffill()
    bars["open"] = bars["open"].fillna(bars["close"])
    bars["high"] = bars["high"].fillna(bars["close"])
    bars["low"] = bars["low"].fillna(bars["close"])
    bars["volume"] = bars["volume"].fillna(0.0)
    bars["trade_count"] = bars["trade_count"].fillna(0).astype(np.int64)

    # Rename to output schema
    bars.rename(columns={"minute_close_ns": "ts_event"}, inplace=True)
    bars = bars[["open", "high", "low", "close", "volume",
                 "trade_count", "has_trades", "ts_event"]]

    return bars


def write_parquet(bars, output_path, instrument):
    """Write timer bars DataFrame to parquet with metadata."""
    parent = os.path.dirname(output_path)
    if parent:
        os.makedirs(parent, exist_ok=True)

    schema = pa.schema([
        ("open", pa.float64()),
        ("high", pa.float64()),
        ("low", pa.float64()),
        ("close", pa.float64()),
        ("volume", pa.float64()),
        ("trade_count", pa.int64()),
        ("has_trades", pa.bool_()),
        ("ts_event", pa.uint64()),
    ], metadata={
        b"instrument_id": instrument.encode(),
        b"bar_type": b"timer-1-minute",
        b"source": b"build_timer_bars.py (from trade parquet)",
    })

    table = pa.Table.from_pandas(bars, schema=schema, preserve_index=False)
    pq.write_table(table, output_path)


def print_summary(bars, instrument, start_date, end_date):
    """Print a human-readable summary of the timer bars."""
    total = len(bars)
    with_trades = int(bars["has_trades"].sum())
    empty = total - with_trades
    pct = with_trades / total * 100 if total > 0 else 0

    first_ts = bars["ts_event"].iloc[0]
    last_ts = bars["ts_event"].iloc[-1]
    first_dt = datetime.fromtimestamp(first_ts / 1e9, tz=timezone.utc)
    last_dt = datetime.fromtimestamp(last_ts / 1e9, tz=timezone.utc)

    print(f"\n=== Timer Bars: {instrument} ===")
    print(f"Range:       {start_date} to {end_date}")
    print(f"First bar:   {first_dt.strftime('%Y-%m-%d %H:%M')} UTC")
    print(f"Last bar:    {last_dt.strftime('%Y-%m-%d %H:%M')} UTC")
    print(f"Total bars:  {total:,}")
    print(f"With trades: {with_trades:,} ({pct:.1f}%)")
    print(f"Empty bars:  {empty:,} ({100 - pct:.1f}%)")

    # Show a few empty bar samples
    empty_bars = bars[~bars["has_trades"]]
    if len(empty_bars) > 0:
        print(f"\nSample empty bars (first 5):")
        for _, row in empty_bars.head(5).iterrows():
            dt = datetime.fromtimestamp(row["ts_event"] / 1e9, tz=timezone.utc)
            print(f"  {dt.strftime('%Y-%m-%d %H:%M')} close={row['close']:.2f} "
                  f"vol={row['volume']:.3f} trades={row['trade_count']}")

    # Price range
    print(f"\nPrice range: {bars['low'].min():.2f} — {bars['high'].max():.2f}")
    print(f"Total volume: {bars['volume'].sum():,.3f}")


def parse_args():
    parser = argparse.ArgumentParser(
        description="Build continuous timer-driven 1-minute bars from trade parquet."
    )
    parser.add_argument(
        "--instrument", default="BTCUSDT_PERP_BINANCE",
        help="Instrument directory name under /data/parquet/trades/",
    )
    parser.add_argument(
        "--start", required=True,
        help="Start date (YYYY-MM-DD)",
    )
    parser.add_argument(
        "--end", default=None,
        help="End date inclusive (YYYY-MM-DD). Defaults to start date.",
    )
    parser.add_argument(
        "--output", default=None,
        help="Output parquet path. If omitted, prints summary only.",
    )
    return parser.parse_args()


def main():
    args = parse_args()

    start_date = date.fromisoformat(args.start)
    end_date = date.fromisoformat(args.end) if args.end else start_date

    if end_date < start_date:
        print("ERROR: end date must be >= start date", file=sys.stderr)
        sys.exit(1)

    print(f"Building timer bars: {args.instrument} "
          f"{start_date} to {end_date}", file=sys.stderr)

    bars = build_timer_bars(args.instrument, start_date, end_date)
    if bars is None:
        sys.exit(1)

    print_summary(bars, args.instrument, start_date, end_date)

    if args.output:
        write_parquet(bars, args.output, args.instrument)
        print(f"\nWritten to: {args.output}")
        print(f"File size: {os.path.getsize(args.output) / 1024:.1f} KB")


if __name__ == "__main__":
    main()
