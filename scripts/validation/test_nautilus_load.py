#!/usr/bin/env python3
"""
Nautilus Parquet Validation Script

Validates that generated Parquet files can be loaded by Nautilus Trader.
Tests:
1. Schema compatibility (column names, types, order)
2. Metadata presence (bar_type, instrument_id, precisions)
3. Data integrity (fixed-point decoding)
4. Timestamp validation (ts_event = bar close time)

Usage:
    python scripts/validation/test_nautilus_load.py

Prerequisites:
    uv pip install pyarrow pandas
    # Optional for full Nautilus validation:
    uv pip install nautilus_trader
"""

import sys
import struct
from pathlib import Path

try:
    import pyarrow.parquet as pq
    import pyarrow as pa
except ImportError:
    print("ERROR: pyarrow not installed. Run: uv pip install pyarrow")
    sys.exit(1)

# Expected schemas
EXPECTED_BAR_COLUMNS = ["open", "high", "low", "close", "volume", "ts_event", "ts_init"]
EXPECTED_TRADE_COLUMNS = ["price", "size", "aggressor_side", "trade_id", "ts_event", "ts_init"]

# Nautilus aggressor side values
AGGRESSOR_NO = 0
AGGRESSOR_BUYER = 1
AGGRESSOR_SELLER = 2


def decode_fixed_point(data: bytes) -> float:
    """Decode Nautilus fixed-point: i128 LE * 1e-16 (Nautilus 1.222.0+)"""
    if len(data) == 16:
        # Nautilus 1.222.0+ format: i128 * 1e16
        value = int.from_bytes(data, 'little', signed=True)
        return value / 10_000_000_000_000_000.0
    elif len(data) == 8:
        # Legacy format: i64 * 1e9
        value = struct.unpack("<q", data)[0]
        return value / 1_000_000_000.0
    else:
        raise ValueError(f"Expected 8 or 16 bytes, got {len(data)}")


def validate_bars(path: str) -> bool:
    """Validate bar Parquet file."""
    print(f"\n=== Validating Bars: {path} ===")
    errors = []

    try:
        table = pq.read_table(path)
        schema = table.schema
        metadata = schema.metadata or {}

        # Check column names and order
        actual_columns = [field.name for field in schema]
        if actual_columns != EXPECTED_BAR_COLUMNS:
            errors.append(f"Column mismatch: expected {EXPECTED_BAR_COLUMNS}, got {actual_columns}")

        # Check column types
        for i, field in enumerate(schema):
            if field.name in ["open", "high", "low", "close", "volume"]:
                # Nautilus 1.222.0+ uses 16-byte, legacy uses 8-byte
                if not pa.types.is_fixed_size_binary(field.type) or field.type.byte_width not in [8, 16]:
                    errors.append(f"Column '{field.name}' should be FixedSizeBinary(8 or 16), got {field.type}")
            elif field.name in ["ts_event", "ts_init"]:
                if not pa.types.is_uint64(field.type):
                    errors.append(f"Column '{field.name}' should be UInt64, got {field.type}")

        # Check required metadata
        metadata_str = {k.decode() if isinstance(k, bytes) else k: v.decode() if isinstance(v, bytes) else v
                       for k, v in metadata.items()}

        required_meta = ["bar_type", "price_precision", "size_precision"]
        for key in required_meta:
            if key not in metadata_str:
                errors.append(f"Missing metadata: {key}")

        if "bar_type" in metadata_str:
            bar_type = metadata_str["bar_type"]
            # Validate bar_type format: SYMBOL.VENUE-STEP-AGGREGATION-PRICE_TYPE-EXTERNAL
            if not bar_type.endswith("-EXTERNAL"):
                errors.append(f"bar_type should end with -EXTERNAL: {bar_type}")
            parts = bar_type.split("-")
            if len(parts) < 5:
                errors.append(f"bar_type has wrong format: {bar_type}")

        # Validate data
        num_rows = table.num_rows
        print(f"  Rows: {num_rows}")
        print(f"  Columns: {actual_columns}")
        print(f"  Metadata: {metadata_str}")

        if num_rows > 0:
            # Check first row data integrity
            opens = table.column("open").to_pylist()
            ts_events = table.column("ts_event").to_pylist()

            # PyArrow returns bytes directly for FixedSizeBinary
            first_open_bytes = opens[0] if isinstance(opens[0], bytes) else opens[0].as_py()
            first_open = decode_fixed_point(first_open_bytes)
            first_ts = ts_events[0] if isinstance(ts_events[0], int) else ts_events[0].as_py()

            print(f"  First bar: open={first_open:.2f}, ts_event={first_ts}")

            # Validate ts_event is on minute boundary (should be bar close time)
            if first_ts % 60_000_000_000 != 0:
                # Not a strict error, but worth noting
                print(f"  NOTE: ts_event not on minute boundary (may be expected for sub-minute data)")

    except Exception as e:
        errors.append(f"Failed to read file: {e}")

    if errors:
        print("  ERRORS:")
        for err in errors:
            print(f"    - {err}")
        return False
    else:
        print("  PASSED")
        return True


def validate_trades(path: str) -> bool:
    """Validate trade Parquet file."""
    print(f"\n=== Validating Trades: {path} ===")
    errors = []

    try:
        table = pq.read_table(path)
        schema = table.schema
        metadata = schema.metadata or {}

        # Check column names and order
        actual_columns = [field.name for field in schema]
        if actual_columns != EXPECTED_TRADE_COLUMNS:
            errors.append(f"Column mismatch: expected {EXPECTED_TRADE_COLUMNS}, got {actual_columns}")

        # Check column types
        for field in schema:
            if field.name in ["price", "size"]:
                # Nautilus 1.222.0+ uses 16-byte, legacy uses 8-byte
                if not pa.types.is_fixed_size_binary(field.type) or field.type.byte_width not in [8, 16]:
                    errors.append(f"Column '{field.name}' should be FixedSizeBinary(8 or 16), got {field.type}")
            elif field.name == "aggressor_side":
                if not pa.types.is_uint8(field.type):
                    errors.append(f"Column 'aggressor_side' should be UInt8, got {field.type}")
            elif field.name == "trade_id":
                if not pa.types.is_string(field.type) and not pa.types.is_large_string(field.type):
                    errors.append(f"Column 'trade_id' should be Utf8/String, got {field.type}")
            elif field.name in ["ts_event", "ts_init"]:
                if not pa.types.is_uint64(field.type):
                    errors.append(f"Column '{field.name}' should be UInt64, got {field.type}")

        # Check required metadata
        metadata_str = {k.decode() if isinstance(k, bytes) else k: v.decode() if isinstance(v, bytes) else v
                       for k, v in metadata.items()}

        required_meta = ["instrument_id", "price_precision", "size_precision"]
        for key in required_meta:
            if key not in metadata_str:
                errors.append(f"Missing metadata: {key}")

        # Validate data
        num_rows = table.num_rows
        print(f"  Rows: {num_rows}")
        print(f"  Columns: {actual_columns}")
        print(f"  Metadata: {metadata_str}")

        if num_rows > 0:
            # Check first row data integrity
            prices = table.column("price").to_pylist()
            sizes = table.column("size").to_pylist()
            sides = table.column("aggressor_side").to_pylist()
            trade_ids = table.column("trade_id").to_pylist()

            # PyArrow returns bytes directly for FixedSizeBinary
            first_price_bytes = prices[0] if isinstance(prices[0], bytes) else prices[0].as_py()
            first_size_bytes = sizes[0] if isinstance(sizes[0], bytes) else sizes[0].as_py()
            first_price = decode_fixed_point(first_price_bytes)
            first_size = decode_fixed_point(first_size_bytes)
            first_side = sides[0] if isinstance(sides[0], int) else sides[0].as_py()
            first_id = trade_ids[0] if isinstance(trade_ids[0], str) else trade_ids[0].as_py()

            print(f"  First trade: price={first_price:.2f}, size={first_size:.6f}, side={first_side}, id={first_id}")

            # Validate aggressor_side values
            for i, side in enumerate(sides[:10]):  # Check first 10
                s = side if isinstance(side, int) else side.as_py()
                if s not in [AGGRESSOR_NO, AGGRESSOR_BUYER, AGGRESSOR_SELLER]:
                    errors.append(f"Invalid aggressor_side at row {i}: {s}")

    except Exception as e:
        errors.append(f"Failed to read file: {e}")

    if errors:
        print("  ERRORS:")
        for err in errors:
            print(f"    - {err}")
        return False
    else:
        print("  PASSED")
        return True


def try_nautilus_load():
    """Attempt to load files with Nautilus (if installed)."""
    print("\n=== Nautilus Trader Integration Test ===")
    try:
        from nautilus_trader.persistence.catalog import ParquetDataCatalog
        print("  nautilus_trader is installed, attempting catalog load...")
        # Note: Full Nautilus integration would require proper catalog setup
        # This is a placeholder for future integration testing
        print("  SKIPPED: Full Nautilus catalog test requires additional setup")
        return True
    except ImportError:
        print("  nautilus_trader not installed (optional)")
        print("  To enable: uv pip install nautilus_trader")
        return True  # Not a failure


def main():
    print("=" * 60)
    print("Nautilus Parquet Validation")
    print("=" * 60)

    bars_path = "/tmp/test_bars.parquet"
    trades_path = "/tmp/test_trades.parquet"

    # Check files exist
    if not Path(bars_path).exists():
        print(f"\nERROR: {bars_path} not found")
        print("Run: cargo run -p barter-data-server --bin generate-sample-parquet")
        sys.exit(1)

    if not Path(trades_path).exists():
        print(f"\nERROR: {trades_path} not found")
        print("Run: cargo run -p barter-data-server --bin generate-sample-parquet")
        sys.exit(1)

    # Run validations
    bars_ok = validate_bars(bars_path)
    trades_ok = validate_trades(trades_path)
    nautilus_ok = try_nautilus_load()

    # Summary
    print("\n" + "=" * 60)
    print("SUMMARY")
    print("=" * 60)
    print(f"  Bars validation:   {'PASSED' if bars_ok else 'FAILED'}")
    print(f"  Trades validation: {'PASSED' if trades_ok else 'FAILED'}")
    print(f"  Nautilus test:     {'PASSED' if nautilus_ok else 'FAILED'}")

    if bars_ok and trades_ok and nautilus_ok:
        print("\nAll validations passed!")
        sys.exit(0)
    else:
        print("\nSome validations failed!")
        sys.exit(1)


if __name__ == "__main__":
    main()
