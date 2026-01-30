#!/usr/bin/env python3
"""
Setup Nautilus ParquetDataCatalog for Barter data.

This script:
1. Creates the required directory structure for Nautilus
2. Copies/renames Parquet files with proper timestamp-based naming
3. Tests that the catalog can load the data

Usage:
    python scripts/validation/setup_nautilus_catalog.py [--source-dir /path/to/parquet] [--catalog-dir /path/to/catalog]

Prerequisites:
    pip install pyarrow nautilus_trader
"""

import argparse
import os
import shutil
import sys
from datetime import datetime, timezone
from pathlib import Path

try:
    import pyarrow.parquet as pq
except ImportError:
    print("ERROR: pyarrow not installed. Run: pip install pyarrow")
    sys.exit(1)


def nanos_to_filename_ts(nanos: int) -> str:
    """Convert nanoseconds to Nautilus filename timestamp format."""
    dt = datetime.fromtimestamp(nanos / 1_000_000_000, tz=timezone.utc)
    ns = nanos % 1_000_000_000
    return dt.strftime(f"%Y-%m-%dT%H-%M-%S-{ns:09d}Z")


def get_timestamp_range(table) -> tuple[int, int]:
    """Get min/max ts_event from a Parquet table."""
    ts_events = table.column("ts_event").to_pylist()
    return min(ts_events), max(ts_events)


def setup_catalog(source_dir: Path, catalog_dir: Path, verbose: bool = True):
    """Set up Nautilus catalog from Barter Parquet files."""

    # Create catalog directory structure
    bar_dir = catalog_dir / "data" / "bar"
    trade_dir = catalog_dir / "data" / "trade_tick"

    bar_dir.mkdir(parents=True, exist_ok=True)
    trade_dir.mkdir(parents=True, exist_ok=True)

    if verbose:
        print(f"Catalog directory: {catalog_dir}")
        print(f"  Bars: {bar_dir}")
        print(f"  Trades: {trade_dir}")

    # Process bar files
    bars_source = source_dir / "test_bars.parquet"
    if bars_source.exists():
        table = pq.read_table(str(bars_source))
        ts_min, ts_max = get_timestamp_range(table)
        filename = f"{nanos_to_filename_ts(ts_min)}_{nanos_to_filename_ts(ts_max)}.parquet"
        dest = bar_dir / filename
        shutil.copy(bars_source, dest)
        if verbose:
            print(f"\nCopied bars: {bars_source.name} -> {filename}")
            print(f"  Rows: {table.num_rows}")
            print(f"  Time range: {ts_min} - {ts_max}")

    # Process trade files
    trades_source = source_dir / "test_trades.parquet"
    if trades_source.exists():
        table = pq.read_table(str(trades_source))
        ts_min, ts_max = get_timestamp_range(table)
        filename = f"{nanos_to_filename_ts(ts_min)}_{nanos_to_filename_ts(ts_max)}.parquet"
        dest = trade_dir / filename
        shutil.copy(trades_source, dest)
        if verbose:
            print(f"\nCopied trades: {trades_source.name} -> {filename}")
            print(f"  Rows: {table.num_rows}")
            print(f"  Time range: {ts_min} - {ts_max}")

    return catalog_dir


def test_catalog(catalog_dir: Path, verbose: bool = True):
    """Test that Nautilus can load the catalog."""
    try:
        from nautilus_trader.persistence.catalog import ParquetDataCatalog
    except ImportError:
        print("\nWARNING: nautilus_trader not installed, skipping catalog test")
        return True

    if verbose:
        print(f"\n=== Testing Nautilus Catalog ===")

    catalog = ParquetDataCatalog(str(catalog_dir))

    # Check data types
    data_types = catalog.list_data_types()
    if verbose:
        print(f"Available data types: {data_types}")

    # Load bars
    try:
        bars = catalog.bars()
        if verbose:
            print(f"Loaded {len(bars)} bars")
            if bars:
                print(f"  First: {bars[0]}")
    except Exception as e:
        print(f"ERROR loading bars: {e}")
        return False

    # Load trades
    try:
        trades = catalog.trade_ticks()
        if verbose:
            print(f"Loaded {len(trades)} trades")
            if trades:
                print(f"  First: {trades[0]}")
    except Exception as e:
        print(f"ERROR loading trades: {e}")
        return False

    if verbose:
        print("\n✓ Catalog test PASSED")
    return True


def main():
    parser = argparse.ArgumentParser(description="Setup Nautilus catalog from Barter Parquet files")
    parser.add_argument("--source-dir", type=Path, default=Path("/tmp"),
                       help="Directory containing source Parquet files")
    parser.add_argument("--catalog-dir", type=Path, default=Path("/tmp/nautilus_catalog"),
                       help="Directory for Nautilus catalog")
    parser.add_argument("--clean", action="store_true",
                       help="Remove existing catalog directory first")
    parser.add_argument("--quiet", action="store_true",
                       help="Suppress verbose output")

    args = parser.parse_args()
    verbose = not args.quiet

    if verbose:
        print("=" * 60)
        print("Nautilus Catalog Setup")
        print("=" * 60)

    # Clean if requested
    if args.clean and args.catalog_dir.exists():
        if verbose:
            print(f"Removing existing catalog: {args.catalog_dir}")
        shutil.rmtree(args.catalog_dir)

    # Setup catalog
    setup_catalog(args.source_dir, args.catalog_dir, verbose)

    # Test catalog
    success = test_catalog(args.catalog_dir, verbose)

    if verbose:
        print("\n" + "=" * 60)
        if success:
            print("Catalog setup complete!")
            print(f"\nUse in Python:")
            print(f'  from nautilus_trader.persistence.catalog import ParquetDataCatalog')
            print(f'  catalog = ParquetDataCatalog("{args.catalog_dir}")')
            print(f'  bars = catalog.bars()')
            print(f'  trades = catalog.trade_ticks()')
        else:
            print("Catalog setup failed!")

    sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()
