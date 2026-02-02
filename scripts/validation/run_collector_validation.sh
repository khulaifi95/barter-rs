#!/bin/bash
# Barter Data Server - Connection & Data Validation Script
# Run a 5-minute test to verify all data feeds are working

set -euo pipefail

# Configuration (defaults aligned with collector-only, Binance PERP, BTC)
export PARQUET_ENABLED="${PARQUET_ENABLED:-1}"
export PARQUET_OUTPUT_DIR="${PARQUET_OUTPUT_DIR:-/tmp/barter_validation_$(date +%Y%m%d_%H%M%S)}"
export PARQUET_FLUSH_INTERVAL_SECS="${PARQUET_FLUSH_INTERVAL_SECS:-60}"
export PARQUET_WRITE_TRADES="${PARQUET_WRITE_TRADES:-0}"
export PARQUET_WRITE_BARS="${PARQUET_WRITE_BARS:-1}"
export PARQUET_WRITE_EXTENDED="${PARQUET_WRITE_EXTENDED:-1}"

# Stream configuration - enable required feeds for PERP validation
export STREAM_ASSETS="${STREAM_ASSETS:-BTC}"
export STREAM_VENUES="${STREAM_VENUES:-BINANCE}"
export STREAM_L1="${STREAM_L1:-1}"
export STREAM_L2="${STREAM_L2:-0}"  # Disable L2 for lean test (optional)
export STREAM_TRADES="${STREAM_TRADES:-1}"
export STREAM_OI="${STREAM_OI:-1}"
export STREAM_LIQ="${STREAM_LIQ:-1}"
export STREAM_CVD="${STREAM_CVD:-1}"
export STREAM_FUNDING="${STREAM_FUNDING:-1}"
export STREAM_PERP="${STREAM_PERP:-1}"
export STREAM_SPOT="${STREAM_SPOT:-0}"

# Parquet filters (match stream filters by default)
export PARQUET_ASSETS="${PARQUET_ASSETS:-$STREAM_ASSETS}"
export PARQUET_VENUES="${PARQUET_VENUES:-$STREAM_VENUES}"

# Logging
export RUST_LOG="${RUST_LOG:-info,barter_data=debug}"

# Duration (default 15 minutes; override for quick runs)
DURATION_SECS="${VALIDATION_DURATION_SECS:-900}"

# Create output dir
mkdir -p "$PARQUET_OUTPUT_DIR"

echo "=============================================="
echo "BARTER DATA SERVER - VALIDATION TEST"
echo "=============================================="
echo "Output dir: $PARQUET_OUTPUT_DIR"
echo "Duration: ${DURATION_SECS}s"
echo "Assets: $STREAM_ASSETS"
echo "Exchanges: $STREAM_VENUES"
echo "Market: PERP only"
echo "Data: Trades, L1, OI, Liquidations, CVD, Funding"
echo "Parquet: trades=$PARQUET_WRITE_TRADES, bars=$PARQUET_WRITE_BARS, extended=$PARQUET_WRITE_EXTENDED"
echo "=============================================="
echo ""
echo "Starting server..."
echo "Press Ctrl+C to stop early"
echo ""

# Run for duration (portable: use timeout if available, otherwise background + sleep)
if command -v timeout >/dev/null 2>&1; then
  timeout "$DURATION_SECS" ./target/release/barter-data-server 2>&1 | tee "$PARQUET_OUTPUT_DIR/server.log" || true
elif command -v gtimeout >/dev/null 2>&1; then
  gtimeout "$DURATION_SECS" ./target/release/barter-data-server 2>&1 | tee "$PARQUET_OUTPUT_DIR/server.log" || true
else
  ./target/release/barter-data-server 2>&1 | tee "$PARQUET_OUTPUT_DIR/server.log" &
  SERVER_PID=$!
  sleep "$DURATION_SECS" || true
  kill "$SERVER_PID" >/dev/null 2>&1 || true
  wait "$SERVER_PID" || true
fi

echo ""
echo "=============================================="
echo "TEST COMPLETE - Checking output..."
echo "=============================================="

# Count parquet files
BARS=$(find "$PARQUET_OUTPUT_DIR" -name "*.parquet" -path "*/bars_1m/*" | wc -l)
EXTENDED=$(find "$PARQUET_OUTPUT_DIR" -name "*.parquet" -path "*/extended_bars_1m/*" | wc -l)
TRADES=$(find "$PARQUET_OUTPUT_DIR" -name "*.parquet" -path "*/trades/*" | wc -l)

echo "Parquet files generated:"
echo "  - bars_1m: $BARS"
echo "  - extended_bars_1m: $EXTENDED"
echo "  - trades: $TRADES"
echo ""
echo "Output directory: $PARQUET_OUTPUT_DIR"
echo ""
echo "To validate data, run:"
echo "  python3 scripts/validation/validate_parquet.py $PARQUET_OUTPUT_DIR"
