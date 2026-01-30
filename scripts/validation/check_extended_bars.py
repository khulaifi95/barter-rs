#!/usr/bin/env python3
"""
Validate extended bars Parquet schema (Barter-only).

Usage:
  python scripts/validation/check_extended_bars.py /tmp/test_extended_bars.parquet
"""

import sys
import pyarrow.parquet as pq


REQUIRED_COLUMNS = [
    "ts_event",
    "ts_init",
    "ts_open",
    "instrument_id",
    "open",
    "high",
    "low",
    "close",
    "volume",
    "quote_volume",
    "trade_count",
    "buy_volume",
    "sell_volume",
    "delta",
    "cvd",
    "open_interest",
    "oi_change",
    "funding_rate",
    "bid_price",
    "bid_size",
    "ask_price",
    "ask_size",
    "spread_bps",
    "book_imbalance",
    "liq_buy_usd",
    "liq_sell_usd",
    "liq_total_usd",
    "liq_count",
    "bid_depth_10bps_base",
    "ask_depth_10bps_base",
    "bid_depth_10bps_usd",
    "ask_depth_10bps_usd",
    "depth_imb_10bps",
    "bid_depth_50bps_base",
    "ask_depth_50bps_base",
    "bid_depth_50bps_usd",
    "ask_depth_50bps_usd",
    "depth_imb_50bps",
    "bid_depth_100bps_base",
    "ask_depth_100bps_base",
    "bid_depth_100bps_usd",
    "ask_depth_100bps_usd",
    "depth_imb_100bps",
]


def main() -> int:
    if len(sys.argv) != 2:
        print("Usage: check_extended_bars.py <path>")
        return 2

    path = sys.argv[1]
    table = pq.read_table(path)
    schema = table.schema
    cols = schema.names

    missing = [c for c in REQUIRED_COLUMNS if c not in cols]
    if missing:
        print("Missing columns:", missing)
        return 1

    # Basic non-zero sanity checks
    liq_total = table.column("liq_total_usd").to_pylist()
    depth_usd = table.column("bid_depth_10bps_usd").to_pylist()
    if all(v == 0 for v in liq_total):
        print("liq_total_usd is all zeros")
        return 1
    if all(v == 0 for v in depth_usd):
        print("bid_depth_10bps_usd is all zeros")
        return 1

    meta = schema.metadata or {}
    schema_version = meta.get(b"schema_version", b"").decode("utf-8")
    print("Schema version:", schema_version)
    print("Row count:", table.num_rows)
    print("Extended bars validation OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
