#!/usr/bin/env python3
"""Row-level gap audit for barter parquet + features data.

Audits:
  1. bars_1m — row count per day, missing minute detection
  2. trades  — cross-reference to classify gaps (WS drop vs pipeline bug)
  3. extended_bars_1m — row parity with bars_1m
  4. features — completeness (3 files/day, non-zero rows)

Not audited (deferred):
  - L2 order book: raw L2 not stored (PARQUET_WRITE_L2=0).
    Only depth-band columns in extended_bars are computed live.
    To audit L2 gaps, enable PARQUET_WRITE_L2=1 and add L2 delta checks.

Runs inside the jupyter container (requires pyarrow).

Usage: python3 gap_audit.py [max_days] [gap_threshold_minutes]
"""

import pyarrow.parquet as pq
import os
import sys
from datetime import datetime, timezone, timedelta

max_days = int(sys.argv[1]) if len(sys.argv) > 1 else 1
gap_threshold = int(sys.argv[2]) if len(sys.argv) > 2 else 10

BARS_BASE = "/data/parquet/bars_1m"
EXTENDED_BASE = "/data/parquet/extended_bars_1m"
TRADES_BASE = "/data/parquet/trades"
FEATURES_BASE = "/data/features"

EXPECTED_FEATURE_FILES = {"large_trades.parquet", "profile_events.parquet", "tpo_brackets.parquet"}

today = datetime.now(timezone.utc).date()
cutoff = today - timedelta(days=max_days)


def read_minutes(base_dir, instrument, day):
    """Read all ts_event values from parquet files, return set of minute buckets and row count."""
    day_dir = os.path.join(base_dir, instrument, day)
    if not os.path.isdir(day_dir):
        return None, 0
    minutes = set()
    rows = 0
    for f in os.listdir(day_dir):
        if not f.endswith(".parquet"):
            continue
        try:
            table = pq.read_table(os.path.join(day_dir, f), columns=["ts_event"])
            rows += len(table)
            for ts in table["ts_event"].to_pylist():
                minutes.add(ts // 60_000_000_000)
        except Exception:
            pass
    return minutes, rows


def count_rows(base_dir, instrument, day):
    """Count total rows across all parquet files for a day."""
    day_dir = os.path.join(base_dir, instrument, day)
    if not os.path.isdir(day_dir):
        return None
    rows = 0
    for f in os.listdir(day_dir):
        if not f.endswith(".parquet"):
            continue
        try:
            meta = pq.read_metadata(os.path.join(day_dir, f))
            rows += meta.num_rows
        except Exception:
            pass
    return rows


def count_feature_rows(instrument, day):
    """Check feature files: return dict of {filename: row_count}."""
    day_dir = os.path.join(FEATURES_BASE, instrument, day)
    if not os.path.isdir(day_dir):
        return None
    result = {}
    for f in os.listdir(day_dir):
        if not f.endswith(".parquet"):
            continue
        try:
            meta = pq.read_metadata(os.path.join(day_dir, f))
            result[f] = meta.num_rows
        except Exception:
            result[f] = -1
    return result


def format_gap_ranges(missing_minutes):
    """Convert sorted list of minute-buckets to human-readable time ranges."""
    if not missing_minutes:
        return []
    ranges = []
    start = prev = missing_minutes[0]
    for m in missing_minutes[1:]:
        if m == prev + 1:
            prev = m
        else:
            ranges.append((start, prev))
            start = prev = m
    ranges.append((start, prev))

    formatted = []
    for s, e in ranges:
        ts_s = datetime.fromtimestamp(s * 60, tz=timezone.utc).strftime("%H:%M")
        ts_e = datetime.fromtimestamp((e + 1) * 60, tz=timezone.utc).strftime("%H:%M")
        dur = e - s + 1
        if dur == 1:
            formatted.append(f"{ts_s} (1 min)")
        else:
            formatted.append(f"{ts_s}-{ts_e} ({dur} min)")
    return formatted


def bars_instrument_to_short(name):
    """Map bars_1m instrument name to short form used by extended_bars/features.

    e.g. BTCUSDT_PERP_BINANCE_1_MINUTE_LAST_EXTERNAL -> BTCUSDT_PERP_BINANCE
    """
    # Strip the _1_MINUTE_LAST_EXTERNAL suffix if present
    suffix = "_1_MINUTE_LAST_EXTERNAL"
    if name.endswith(suffix):
        return name[:-len(suffix)]
    return name


# ── Main ──────────────────────────────────────────────────────

if not os.path.isdir(BARS_BASE):
    print("STATUS:ERROR")
    print("MSG:bars_1m directory not found")
    sys.exit(0)

instruments = sorted(
    d for d in os.listdir(BARS_BASE)
    if os.path.isdir(os.path.join(BARS_BASE, d))
)

total_ws_gaps = 0
total_pipeline_gaps = 0
total_ext_mismatches = 0
total_feature_issues = 0
alert_lines = []
summary_lines = []

for instrument in instruments:
    inst_short = instrument.split("_")[0]  # e.g. BTCUSDT
    inst_ext = bars_instrument_to_short(instrument)  # for extended_bars / features

    inst_dir = os.path.join(BARS_BASE, instrument)
    dates = sorted(
        d for d in os.listdir(inst_dir)
        if os.path.isdir(os.path.join(inst_dir, d))
        and d >= str(cutoff) and d <= str(today)
    )

    for day in dates:
        is_today = (day == str(today))
        bar_minutes, bar_rows = read_minutes(BARS_BASE, instrument, day)

        if bar_minutes is None or bar_rows == 0:
            continue

        if is_today:
            now = datetime.now(timezone.utc)
            expected = now.hour * 60 + now.minute
            if expected < 60:
                continue  # too early to judge
        else:
            expected = 1440

        # ── Bar gap analysis ──────────────────────────────────
        min_m, max_m = min(bar_minutes), max(bar_minutes)
        all_minutes = set(range(min_m, max_m + 1))
        missing_bar_minutes = sorted(all_minutes - bar_minutes)
        gap_count = len(missing_bar_minutes)

        # ── Extended bars parity ──────────────────────────────
        ext_rows = count_rows(EXTENDED_BASE, inst_ext, day)
        ext_note = ""
        if ext_rows is None:
            ext_note = ", ext=MISSING"
            total_ext_mismatches += 1
            alert_lines.append(f"  EXT MISSING: {inst_short} {day} — no extended_bars dir")
        elif ext_rows != bar_rows:
            ext_note = f", ext={ext_rows}"
            total_ext_mismatches += 1
            alert_lines.append(
                f"  EXT MISMATCH: {inst_short} {day} — bars={bar_rows} ext={ext_rows} (delta={bar_rows - ext_rows})"
            )

        # ── Features completeness ─────────────────────────────
        feat = count_feature_rows(inst_ext, day)
        feat_note = ""
        if feat is None:
            feat_note = ", feat=MISSING"
            total_feature_issues += 1
            alert_lines.append(f"  FEAT MISSING: {inst_short} {day} — no features dir")
        else:
            missing_files = EXPECTED_FEATURE_FILES - set(feat.keys())
            empty_files = [f for f, r in feat.items() if r == 0]
            if missing_files:
                feat_note = f", feat missing: {','.join(sorted(missing_files))}"
                total_feature_issues += 1
                alert_lines.append(
                    f"  FEAT MISSING: {inst_short} {day} — {','.join(sorted(f.replace('.parquet','') for f in missing_files))}"
                )
            elif empty_files:
                feat_note = f", feat empty: {','.join(sorted(empty_files))}"
                total_feature_issues += 1
                alert_lines.append(
                    f"  FEAT EMPTY: {inst_short} {day} — {','.join(sorted(f.replace('.parquet','') for f in empty_files))}"
                )

        # ── Summary line ──────────────────────────────────────
        if gap_count == 0:
            summary_lines.append(
                f"{inst_short} {day}: {bar_rows} bars, {len(bar_minutes)} min — 100%{ext_note}{feat_note}"
            )
            continue

        pct = (1 - gap_count / expected) * 100

        # Cross-reference with trades (trades use short instrument name)
        trade_minutes, trade_rows = read_minutes(TRADES_BASE, inst_ext, day)

        ws_gaps = []
        pipeline_gaps = []
        trades_lost = 0

        for m in missing_bar_minutes:
            if trade_minutes and m in trade_minutes:
                pipeline_gaps.append(m)
            else:
                ws_gaps.append(m)

        # Count trade rows in WS gap windows (how many trades were lost)
        if trade_minutes is not None and ws_gaps:
            # trades_lost stays 0 — by definition, WS gaps have no trades
            pass

        total_ws_gaps += len(ws_gaps)
        total_pipeline_gaps += len(pipeline_gaps)

        summary_lines.append(
            f"{inst_short} {day}: {bar_rows} bars, {len(bar_minutes)} min — {pct:.1f}% "
            f"({len(ws_gaps)} WS + {len(pipeline_gaps)} pipeline){ext_note}{feat_note}"
        )

        if ws_gaps:
            for r in format_gap_ranges(ws_gaps):
                alert_lines.append(f"  WS drop: {inst_short} {day} {r}")

        if pipeline_gaps:
            for r in format_gap_ranges(pipeline_gaps):
                alert_lines.append(f"  PIPELINE: {inst_short} {day} {r}")

# Output structured results for shell to parse
print(f"HEADLINE:WS lost={total_ws_gaps} min | pipeline={total_pipeline_gaps} | ext mismatch={total_ext_mismatches} | feat issues={total_feature_issues}")
print(f"STATUS:OK")
print(f"WS_GAPS:{total_ws_gaps}")
print(f"PIPELINE_GAPS:{total_pipeline_gaps}")
print(f"EXT_MISMATCHES:{total_ext_mismatches}")
print(f"FEATURE_ISSUES:{total_feature_issues}")
print(f"THRESHOLD:{gap_threshold}")

for line in summary_lines:
    print(f"SUMMARY:{line}")
for line in alert_lines:
    print(f"ALERT:{line}")
