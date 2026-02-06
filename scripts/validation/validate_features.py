#!/usr/bin/env python3
"""
Validate barter-features output against expected invariants.

Usage:
  python scripts/validation/validate_features.py /data/features/BTCUSDT-PERP.BINANCE/2026-02-02/

Tests:
  - Schema correctness (required columns present)
  - Value relationships (VAL <= POC <= VAH)
  - Label sequencing (A, B, C, ...)
  - Metadata consistency (source_precision, output_precision)
  - Large trade category thresholds
"""

import sys
import os
from pathlib import Path

try:
    import pyarrow.parquet as pq
except ImportError:
    print("ERROR: pyarrow not installed. Run: uv pip install pyarrow")
    sys.exit(2)


def decode_1e9(values):
    """Decode Int64 × 1e9 to float."""
    return [v / 1e9 if v is not None else None for v in values]


def validate_tpo_brackets(path):
    """Validate TPO brackets output."""
    errors = []

    try:
        table = pq.read_table(path)
    except Exception as e:
        return [f"Failed to read file: {e}"]

    # Check required columns
    required = [
        "ts_event", "ts_init", "schema_version", "config_hash",
        "source_precision", "output_precision", "instrument_id",
        "session_id", "session_date", "label", "bracket_index",
        "bracket_start_ts", "bracket_end_ts", "bracket_open",
        "bracket_high", "bracket_low", "bracket_close", "bracket_volume",
        "vol_poc", "vol_vah", "vol_val", "vol_total",
        "tpo_poc", "tpo_vah", "tpo_val", "ib_high", "ib_low", "ib_complete"
    ]

    missing = [col for col in required if col not in table.schema.names]
    if missing:
        errors.append(f"Missing columns: {missing}")
        return errors  # Can't continue without required columns

    # Decode values
    vol_poc = decode_1e9(table.column("vol_poc").to_pylist())
    vol_vah = decode_1e9(table.column("vol_vah").to_pylist())
    vol_val = decode_1e9(table.column("vol_val").to_pylist())

    # Check VAL <= POC <= VAH relationship
    for i in range(len(vol_poc)):
        if vol_val[i] is None or vol_poc[i] is None or vol_vah[i] is None:
            continue
        if not (vol_val[i] <= vol_poc[i] <= vol_vah[i]):
            errors.append(
                f"Row {i}: VAL <= POC <= VAH violated: "
                f"{vol_val[i]:.2f} <= {vol_poc[i]:.2f} <= {vol_vah[i]:.2f}"
            )
            if len(errors) > 5:
                errors.append("... (truncated)")
                break

    # Check labels are sequential
    labels = table.column("label").to_pylist()
    expected_labels = (
        list("ABCDEFGHIJKLMNOPQRSTUVWXYZ") +
        [f"A{c}" for c in "ABCDEFGHIJKLMNOPQRSTUV"]
    )

    for i, label in enumerate(labels):
        if i >= len(expected_labels):
            break
        if label != expected_labels[i]:
            errors.append(f"Row {i}: Expected label '{expected_labels[i]}', got '{label}'")
            break

    # Check bracket_index matches labels
    indices = table.column("bracket_index").to_pylist()
    for i, (label, idx) in enumerate(zip(labels, indices)):
        expected_idx = expected_labels.index(label) if label in expected_labels else -1
        if expected_idx != idx:
            errors.append(f"Row {i}: bracket_index {idx} doesn't match label '{label}' (expected {expected_idx})")
            break

    # Check metadata
    source_prec = table.column("source_precision").to_pylist()
    if source_prec and source_prec[0] != "standard":
        errors.append(f"source_precision should be 'standard', got '{source_prec[0]}'")

    output_prec = table.column("output_precision").to_pylist()
    if output_prec and output_prec[0] != 9:
        errors.append(f"output_precision should be 9, got {output_prec[0]}")

    # Check timestamps are ordered
    ts_events = table.column("ts_event").to_pylist()
    for i in range(1, len(ts_events)):
        if ts_events[i] < ts_events[i-1]:
            errors.append(f"Row {i}: ts_event not monotonically increasing")
            break

    # Check volumes are non-negative
    volumes = decode_1e9(table.column("bracket_volume").to_pylist())
    for i, vol in enumerate(volumes):
        if vol is not None and vol < 0:
            errors.append(f"Row {i}: Negative volume: {vol}")
            break

    return errors


def validate_profile_events(path):
    """Validate profile events output."""
    errors = []

    try:
        table = pq.read_table(path)
    except FileNotFoundError:
        return []  # Events file is optional
    except Exception as e:
        return [f"Failed to read file: {e}"]

    # Check required columns
    required = [
        "ts_event", "ts_init", "schema_version", "config_hash",
        "source_precision", "output_precision", "instrument_id",
        "session_id", "event_type"
    ]

    missing = [col for col in required if col not in table.schema.names]
    if missing:
        errors.append(f"Missing columns: {missing}")

    # Check event types are valid
    valid_types = [
        "SessionOpen", "SessionClose", "IbComplete",
        "IbBreakUp", "IbBreakDown", "PocShift",
        "VahShift", "ValShift", "PocDivergence", "GapDetected"
    ]

    event_types = table.column("event_type").to_pylist()
    for i, et in enumerate(event_types):
        if et not in valid_types:
            errors.append(f"Row {i}: Unknown event type '{et}'")
            if len(errors) > 5:
                errors.append("... (truncated)")
                break

    return errors


def validate_large_trades(path):
    """Validate large trades output."""
    errors = []

    try:
        table = pq.read_table(path)
    except FileNotFoundError:
        return []  # Large trades file is optional (may be no large trades)
    except Exception as e:
        return [f"Failed to read file: {e}"]

    # Check required columns
    required = [
        "ts_event", "ts_init", "schema_version", "config_hash",
        "source_precision", "output_precision", "instrument_id",
        "trade_id", "price", "size", "side", "notional_usd",
        "category", "threshold_used", "threshold_mode"
    ]

    missing = [col for col in required if col not in table.schema.names]
    if missing:
        errors.append(f"Missing columns: {missing}")
        return errors

    # Check category thresholds
    notional = decode_1e9(table.column("notional_usd").to_pylist())
    categories = table.column("category").to_pylist()

    for i, (n, cat) in enumerate(zip(notional, categories)):
        if n is None:
            continue

        if cat == "LARGE":
            if not (2_000_000 <= n < 5_000_000):
                errors.append(f"Row {i}: LARGE trade notional ${n:,.0f} outside 2M-5M range")
        elif cat == "WHALE":
            if not (5_000_000 <= n < 10_000_000):
                errors.append(f"Row {i}: WHALE trade notional ${n:,.0f} outside 5M-10M range")
        elif cat == "MEGA":
            if n < 10_000_000:
                errors.append(f"Row {i}: MEGA trade notional ${n:,.0f} below 10M threshold")
        else:
            errors.append(f"Row {i}: Unknown category '{cat}'")

        if len(errors) > 5:
            errors.append("... (truncated)")
            break

    # Check sides are valid
    sides = table.column("side").to_pylist()
    for i, side in enumerate(sides):
        if side not in ["BUY", "SELL", "UNKNOWN"]:
            errors.append(f"Row {i}: Invalid side '{side}'")
            break

    return errors


def main():
    if len(sys.argv) != 2:
        print("Usage: validate_features.py <output_dir>")
        print()
        print("Example:")
        print("  python validate_features.py /data/features/BTCUSDT-PERP.BINANCE/2026-02-02/")
        return 2

    output_dir = Path(sys.argv[1])

    if not output_dir.exists():
        print(f"ERROR: Directory not found: {output_dir}")
        return 1

    all_errors = {}
    all_ok = True

    # Validate TPO brackets (required)
    tpo_path = output_dir / "tpo_brackets.parquet"
    print(f"Validating {tpo_path}...")
    if not tpo_path.exists():
        print("  ERROR: tpo_brackets.parquet not found")
        all_ok = False
    else:
        errors = validate_tpo_brackets(tpo_path)
        if errors:
            all_errors["tpo_brackets"] = errors
            all_ok = False
            print(f"  FAILED: {len(errors)} error(s)")
        else:
            table = pq.read_table(tpo_path)
            print(f"  OK: {table.num_rows} brackets")

    # Validate profile events (optional)
    events_path = output_dir / "profile_events.parquet"
    print(f"Validating {events_path}...")
    if events_path.exists():
        errors = validate_profile_events(events_path)
        if errors:
            all_errors["profile_events"] = errors
            all_ok = False
            print(f"  FAILED: {len(errors)} error(s)")
        else:
            table = pq.read_table(events_path)
            print(f"  OK: {table.num_rows} events")
    else:
        print("  SKIPPED: file not found (optional)")

    # Validate large trades (optional)
    trades_path = output_dir / "large_trades.parquet"
    print(f"Validating {trades_path}...")
    if trades_path.exists():
        errors = validate_large_trades(trades_path)
        if errors:
            all_errors["large_trades"] = errors
            all_ok = False
            print(f"  FAILED: {len(errors)} error(s)")
        else:
            table = pq.read_table(trades_path)
            print(f"  OK: {table.num_rows} trades")
    else:
        print("  SKIPPED: file not found (no large trades)")

    # Summary
    print()
    if all_ok:
        print("=" * 50)
        print("VALIDATION PASSED")
        print("=" * 50)
        return 0
    else:
        print("=" * 50)
        print("VALIDATION FAILED")
        print("=" * 50)
        for file_name, errors in all_errors.items():
            print(f"\n{file_name}:")
            for error in errors[:10]:
                print(f"  - {error}")
            if len(errors) > 10:
                print(f"  ... and {len(errors) - 10} more errors")
        return 1


if __name__ == "__main__":
    sys.exit(main())
