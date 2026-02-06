#!/usr/bin/env python3
"""
Setup Nautilus ParquetDataCatalog for Barter data.

This script:
1. Creates the required directory structure for Nautilus
2. Copies/renames Parquet files with proper timestamp-based naming
3. Tests that the catalog can load the data

Supports both:
- Real collected data: raw/bars_1m/*/*.parquet, raw/trades/*/*.parquet
- Test sample files: test_bars.parquet, test_trades.parquet

Usage:
    python scripts/validation/setup_nautilus_catalog.py --source-dir test_output/tpo_65min_2309/raw

Prerequisites:
    uv pip install pyarrow nautilus_trader
"""

import argparse
import shutil
import sys
from decimal import Decimal
from datetime import datetime, timezone
from pathlib import Path

try:
    import pyarrow.parquet as pq
except ImportError:
    print("ERROR: pyarrow not installed. Run: uv pip install pyarrow")
    sys.exit(1)

try:
    from nautilus_trader.model.currencies import BTC, ETH, SOL, USDT
    from nautilus_trader.model.identifiers import InstrumentId, Symbol, Venue
    from nautilus_trader.model.instruments import CryptoPerpetual
    from nautilus_trader.model.objects import Money, Price, Quantity
    from nautilus_trader.persistence.catalog.parquet import ParquetDataCatalog
    _HAS_NAUTILUS = True
except Exception:
    _HAS_NAUTILUS = False


def nanos_to_filename_ts(nanos: int) -> str:
    """Convert nanoseconds to Nautilus filename timestamp format."""
    dt = datetime.fromtimestamp(nanos / 1_000_000_000, tz=timezone.utc)
    ns = nanos % 1_000_000_000
    return dt.strftime(f"%Y-%m-%dT%H-%M-%S-{ns:09d}Z")


def get_timestamp_range(table) -> tuple:
    """Get min/max ts_event from a Parquet table."""
    ts_events = table.column("ts_event").to_pylist()
    return min(ts_events), max(ts_events)


def process_parquet_files(source_files: list, dest_dir: Path, data_type: str, verbose: bool = True):
    """Process multiple parquet files, merge and copy to catalog."""
    if not source_files:
        if verbose:
            print(f"  No {data_type} files found")
        return 0

    # Read all tables and get global time range
    all_tables = []
    global_min = float('inf')
    global_max = 0
    total_rows = 0

    for f in source_files:
        try:
            table = pq.read_table(str(f))
            ts_min, ts_max = get_timestamp_range(table)
            global_min = min(global_min, ts_min)
            global_max = max(global_max, ts_max)
            total_rows += table.num_rows
            all_tables.append(table)
        except Exception as e:
            if verbose:
                print(f"  Warning: Failed to read {f}: {e}")

    if not all_tables:
        return 0

    # Concatenate all tables
    import pyarrow as pa
    merged = pa.concat_tables(all_tables)

    # Write to catalog with timestamp-based filename
    filename = f"{nanos_to_filename_ts(int(global_min))}_{nanos_to_filename_ts(int(global_max))}.parquet"
    dest = dest_dir / filename
    pq.write_table(merged, str(dest))

    if verbose:
        print(f"  {data_type}: {total_rows} rows from {len(source_files)} files -> {filename}")

    return total_rows


def read_metadata_instrument(path: Path):
    # Prefer read_schema for Arrow schema metadata (pyarrow 20+)
    meta = {}
    try:
        schema = pq.read_schema(str(path))
        meta = schema.metadata or {}
    except Exception:
        # Fallback for older pyarrow versions
        try:
            meta = pq.ParquetFile(str(path)).metadata.metadata or {}
        except Exception:
            meta = {}
    instrument_id = meta.get(b"instrument_id")
    price_precision = meta.get(b"price_precision")
    size_precision = meta.get(b"size_precision")
    if instrument_id is not None:
        instrument_id = instrument_id.decode("utf-8")
    if price_precision is not None:
        price_precision = int(price_precision.decode("utf-8"))
    if size_precision is not None:
        size_precision = int(size_precision.decode("utf-8"))
    return instrument_id, price_precision, size_precision


def process_order_book_deltas(source_files: list, dest_root: Path, verbose: bool = True):
    """Process order book deltas, partitioned by instrument_id."""
    if not source_files:
        if verbose:
            print("  No OrderBookDeltas files found")
        return 0, {}

    groups = {}
    instrument_meta = {}
    for f in source_files:
        instrument_id, price_precision, size_precision = read_metadata_instrument(f)
        if not instrument_id:
            if verbose:
                print(f"  Warning: Missing instrument_id metadata in {f}")
            continue
        groups.setdefault(instrument_id, []).append(f)
        instrument_meta.setdefault(
            instrument_id,
            {"price_precision": price_precision, "size_precision": size_precision},
        )

    files_processed = 0
    for instrument_id, files in groups.items():
        dest_dir = dest_root / instrument_id
        dest_dir.mkdir(parents=True, exist_ok=True)
        rows = process_parquet_files(files, dest_dir, f"OrderBookDeltas[{instrument_id}]", verbose)
        files_processed += len(files)

    return files_processed, instrument_meta


def build_crypto_perp(instrument_id: str, price_precision: int | None, size_precision: int | None):
    symbol_str, venue_str = instrument_id.split(".", 1)
    base = BTC if symbol_str.startswith("BTC") else ETH if symbol_str.startswith("ETH") else SOL
    price_precision = price_precision if price_precision is not None else 2
    size_precision = size_precision if size_precision is not None else 3
    price_inc = Price.from_str(f"{10 ** -price_precision:.{price_precision}f}")
    size_inc = Quantity.from_str(f"{10 ** -size_precision:.{size_precision}f}")
    return CryptoPerpetual(
        instrument_id=InstrumentId(
            symbol=Symbol(symbol_str),
            venue=Venue(venue_str),
        ),
        raw_symbol=Symbol(symbol_str.replace("-PERP", "")),
        base_currency=base,
        quote_currency=USDT,
        settlement_currency=USDT,
        is_inverse=False,
        price_precision=price_precision,
        price_increment=price_inc,
        size_precision=size_precision,
        size_increment=size_inc,
        max_quantity=Quantity.from_str("1000.000"),
        min_quantity=Quantity.from_str("0.001"),
        max_notional=None,
        min_notional=Money(10.00, USDT),
        max_price=Price.from_str("809484.0"),
        min_price=Price.from_str("261.1"),
        margin_init=Decimal("0.0500"),
        margin_maint=Decimal("0.0250"),
        maker_fee=Decimal("0.000200"),
        taker_fee=Decimal("0.000180"),
        ts_event=0,
        ts_init=0,
    )


def read_instrument_and_range(path: Path):
    """Read instrument_id and ts_event range from a parquet file."""
    table = pq.read_table(str(path), columns=["instrument_id", "ts_event"])
    ts_events = table.column("ts_event").to_pylist()
    instrument_ids = table.column("instrument_id").to_pylist()
    instrument_id = instrument_ids[0] if instrument_ids else None
    return instrument_id, min(ts_events), max(ts_events)


def process_custom_files(source_files: list, dest_base: Path, label: str, verbose: bool = True):
    """Copy custom data parquet files into Nautilus catalog structure."""
    if not source_files:
        if verbose:
            print(f"  No {label} files found")
        return 0, 0

    files_processed = 0
    rows_processed = 0

    for file_path in source_files:
        try:
            instrument_id, ts_min, ts_max = read_instrument_and_range(file_path)
            if not instrument_id:
                if verbose:
                    print(f"  Warning: Missing instrument_id in {file_path}")
                continue
            table = pq.read_table(str(file_path))
            rows = table.num_rows
            rows_processed += rows

            dest_dir = dest_base / instrument_id
            dest_dir.mkdir(parents=True, exist_ok=True)
            filename = f"{nanos_to_filename_ts(int(ts_min))}_{nanos_to_filename_ts(int(ts_max))}.parquet"
            dest = dest_dir / filename
            pq.write_table(table, str(dest))
            files_processed += 1
        except Exception as e:
            if verbose:
                print(f"  Warning: Failed to process {file_path}: {e}")

    if verbose:
        print(f"  {label}: {rows_processed} rows from {files_processed} files")

    return files_processed, rows_processed


def setup_catalog(source_dir: Path, catalog_dir: Path, verbose: bool = True):
    """Set up Nautilus catalog from Barter Parquet files."""

    # Create catalog directory structure
    bar_dir = catalog_dir / "data" / "bar"
    trade_dir = catalog_dir / "data" / "trade_tick"
    order_book_dir = catalog_dir / "data" / "order_book_deltas"
    custom_extended_dir = catalog_dir / "data" / "custom_extended_bar1m"
    custom_tpo_dir = catalog_dir / "data" / "custom_tpo_bracket"
    custom_large_dir = catalog_dir / "data" / "custom_large_trade"
    custom_profile_dir = catalog_dir / "data" / "custom_profile_event"

    bar_dir.mkdir(parents=True, exist_ok=True)
    trade_dir.mkdir(parents=True, exist_ok=True)
    order_book_dir.mkdir(parents=True, exist_ok=True)
    custom_extended_dir.mkdir(parents=True, exist_ok=True)
    custom_tpo_dir.mkdir(parents=True, exist_ok=True)
    custom_large_dir.mkdir(parents=True, exist_ok=True)
    custom_profile_dir.mkdir(parents=True, exist_ok=True)

    if verbose:
        print(f"Catalog directory: {catalog_dir}")
        print(f"  Bars: {bar_dir}")
        print(f"  Trades: {trade_dir}")
        print(f"  Order book deltas: {order_book_dir}")
        print(f"  Custom extended bars: {custom_extended_dir}")
        print(f"  Custom TPO brackets: {custom_tpo_dir}")
        print(f"  Custom large trades: {custom_large_dir}")
        print(f"  Custom profile events: {custom_profile_dir}")
        print()

    bars_processed = 0
    trades_processed = 0
    order_book_processed = 0
    order_book_instruments = {}

    # Option 1: Real collected data structure (raw/bars_1m/*/.../*.parquet)
    bars_1m_dir = source_dir / "bars_1m"
    trades_dir = source_dir / "trades"
    order_book_deltas_dir = source_dir / "order_book_deltas"

    if bars_1m_dir.exists():
        bar_files = list(bars_1m_dir.rglob("*.parquet"))
        bars_processed = process_parquet_files(bar_files, bar_dir, "Bars", verbose)

    if trades_dir.exists():
        trade_files = list(trades_dir.rglob("*.parquet"))
        trades_processed = process_parquet_files(trade_files, trade_dir, "Trades", verbose)

    if order_book_deltas_dir.exists():
        order_book_files = list(order_book_deltas_dir.rglob("*.parquet"))
        order_book_processed, order_book_instruments = process_order_book_deltas(
            order_book_files, order_book_dir, verbose
        )

    # Custom data (Extended Bars + Features)
    extended_dir = source_dir / "extended_bars_1m"
    feature_root = source_dir.parent / "features"

    if extended_dir.exists():
        extended_files = list(extended_dir.rglob("*.parquet"))
        process_custom_files(extended_files, custom_extended_dir, "ExtendedBar1m", verbose)

    if feature_root.exists():
        tpo_files = list(feature_root.rglob("tpo_brackets.parquet"))
        large_files = list(feature_root.rglob("large_trades.parquet"))
        profile_files = list(feature_root.rglob("profile_events.parquet"))
        process_custom_files(tpo_files, custom_tpo_dir, "TpoBracket", verbose)
        process_custom_files(large_files, custom_large_dir, "LargeTrade", verbose)
        process_custom_files(profile_files, custom_profile_dir, "ProfileEvent", verbose)

    # Option 2: Test sample files (test_bars.parquet, test_trades.parquet)
    if bars_processed == 0:
        test_bars = source_dir / "test_bars.parquet"
        if test_bars.exists():
            bars_processed = process_parquet_files([test_bars], bar_dir, "Bars", verbose)

    if trades_processed == 0:
        test_trades = source_dir / "test_trades.parquet"
        if test_trades.exists():
            trades_processed = process_parquet_files([test_trades], trade_dir, "Trades", verbose)

    if _HAS_NAUTILUS and order_book_instruments:
        try:
            catalog = ParquetDataCatalog(str(catalog_dir))
            instruments = []
            for instrument_id, meta in order_book_instruments.items():
                instruments.append(
                    build_crypto_perp(
                        instrument_id,
                        meta.get("price_precision"),
                        meta.get("size_precision"),
                    )
                )
            if instruments:
                catalog.write_data(instruments)
                if verbose:
                    print(f"  Wrote {len(instruments)} instrument definitions to catalog")
        except Exception as e:
            if verbose:
                print(f"  Warning: Failed to write instrument definitions: {e}")

    return bars_processed, trades_processed, order_book_processed


def test_catalog(catalog_dir: Path, verbose: bool = True):
    """Test that Nautilus can load the catalog."""
    try:
        from nautilus_trader.persistence.catalog import ParquetDataCatalog
    except ImportError:
        print("\nWARNING: nautilus_trader not installed, skipping catalog test")
        return True, 0, 0, 0

    if verbose:
        print(f"\n=== Testing Nautilus Catalog ===")

    catalog = ParquetDataCatalog(str(catalog_dir))

    # Check data types
    data_types = catalog.list_data_types()
    if verbose:
        print(f"Available data types: {data_types}")

    bars_loaded = 0
    trades_loaded = 0
    order_book_loaded = 0

    # Load bars
    try:
        bars = catalog.bars()
        bars_loaded = len(bars)
        if verbose:
            print(f"Loaded {bars_loaded} bars")
            if bars:
                print(f"  First: {bars[0]}")
                print(f"  Last:  {bars[-1]}")
    except Exception as e:
        print(f"ERROR loading bars: {e}")
        return False, 0, 0, 0

    # Load trades
    try:
        trades = catalog.trade_ticks()
        trades_loaded = len(trades)
        if verbose:
            print(f"Loaded {trades_loaded} trades")
            if trades:
                print(f"  First: {trades[0]}")
    except Exception as e:
        print(f"ERROR loading trades: {e}")
        return False, bars_loaded, 0, 0

    # Load order book deltas (optional)
    try:
        if "order_book_deltas" in data_types:
            deltas = catalog.order_book_deltas(batched=True)
            order_book_loaded = len(deltas)
            if verbose:
                print(f"Loaded {order_book_loaded} order book delta batches")
                if deltas:
                    print(f"  First: {deltas[0]}")
    except Exception as e:
        print(f"ERROR loading order book deltas: {e}")
        return False, bars_loaded, trades_loaded, 0

    success = bars_loaded > 0 or trades_loaded > 0 or order_book_loaded > 0
    if verbose:
        if success:
            print("\n✓ Catalog test PASSED")
        else:
            print("\n✗ Catalog test FAILED - no data loaded")

    return success, bars_loaded, trades_loaded, order_book_loaded


def main():
    parser = argparse.ArgumentParser(description="Setup Nautilus catalog from Barter Parquet files")
    parser.add_argument("--source-dir", type=Path, default=Path("/tmp"),
                       help="Directory containing source Parquet files (e.g., test_output/tpo_65min_2309/raw)")
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
        print("Nautilus Catalog Setup (Barter Data)")
        print("=" * 60)

    # Clean if requested
    if args.clean and args.catalog_dir.exists():
        if verbose:
            print(f"Removing existing catalog: {args.catalog_dir}")
        shutil.rmtree(args.catalog_dir)

    # Setup catalog
    bars_processed, trades_processed, order_book_processed = setup_catalog(
        args.source_dir, args.catalog_dir, verbose
    )

    if bars_processed == 0 and trades_processed == 0 and order_book_processed == 0:
        print("\nERROR: No data found in source directory")
        print(f"  Looked in: {args.source_dir}")
        print(f"  Expected: bars_1m/*/*.parquet or test_bars.parquet")
        print(f"  Expected: trades/*/*.parquet or test_trades.parquet")
        print(f"  Expected: order_book_deltas/<instrument>/*/*.parquet")
        sys.exit(1)

    # Test catalog
    success, bars_loaded, trades_loaded, order_book_loaded = test_catalog(args.catalog_dir, verbose)

    if verbose:
        print("\n" + "=" * 60)
        print("RESULTS")
        print("=" * 60)
        print(f"  Bars processed:   {bars_processed}")
        print(f"  Trades processed: {trades_processed}")
        print(f"  L2 processed:     {order_book_processed}")
        print(f"  Bars loaded:      {bars_loaded}")
        print(f"  Trades loaded:    {trades_loaded}")
        print(f"  L2 loaded:        {order_book_loaded}")
        print(f"  Status:          {'PASS' if success else 'FAIL'}")

        if success:
            print(f"\nUse in Python:")
            print(f'  from nautilus_trader.persistence.catalog import ParquetDataCatalog')
            print(f'  catalog = ParquetDataCatalog("{args.catalog_dir}")')
            print(f'  bars = catalog.bars()')
            print(f'  trades = catalog.trade_ticks()')

    sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()
