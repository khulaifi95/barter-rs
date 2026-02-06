#!/bin/bash
# =============================================================================
# 10-Minute Forward Test - Barter Data Collection
# =============================================================================
# This script runs barter-data-server for 10 minutes and validates output
#
# Data types collected:
#   - Trades (tick-by-tick)
#   - 1-minute Bars
#   - Extended Bars (OHLCV + VWAP + trade count)
#   - L2 OrderBook Deltas (sampled)
# =============================================================================

set -e

# Configuration
TEST_DURATION_SECS=600  # 10 minutes
OUTPUT_DIR="/tmp/barter_forward_test_$(date +%Y%m%d_%H%M%S)"
FLUSH_INTERVAL=30       # Flush every 30 seconds for faster feedback

echo "=============================================="
echo "  BARTER 10-MINUTE FORWARD TEST"
echo "=============================================="
echo ""
echo "Output directory: $OUTPUT_DIR"
echo "Test duration: $TEST_DURATION_SECS seconds (10 minutes)"
echo "Data types: Trades, Bars, Extended Bars, L2 Deltas"
echo ""

# Create output directory
mkdir -p "$OUTPUT_DIR"

# Export environment variables for barter-data-server
export PARQUET_ENABLED=1
export PARQUET_OUTPUT_DIR="$OUTPUT_DIR"
export PARQUET_FLUSH_INTERVAL_SECS=$FLUSH_INTERVAL

# Data type toggles
export PARQUET_WRITE_TRADES=true
export PARQUET_WRITE_BARS=true
export PARQUET_WRITE_EXTENDED=true
export PARQUET_WRITE_L2=true

# L2 settings
export PARQUET_L2_MAX_DEPTH=20
export PARQUET_L2_SAMPLE_MS=100  # Sample L2 every 100ms

# Filter settings (BTC on Binance)
export PARQUET_ASSETS=BTC
export PARQUET_VENUES=BINANCE

echo "Environment configured:"
echo "  PARQUET_ENABLED=$PARQUET_ENABLED"
echo "  PARQUET_OUTPUT_DIR=$PARQUET_OUTPUT_DIR"
echo "  PARQUET_WRITE_TRADES=$PARQUET_WRITE_TRADES"
echo "  PARQUET_WRITE_BARS=$PARQUET_WRITE_BARS"
echo "  PARQUET_WRITE_EXTENDED=$PARQUET_WRITE_EXTENDED"
echo "  PARQUET_WRITE_L2=$PARQUET_WRITE_L2"
echo "  PARQUET_L2_MAX_DEPTH=$PARQUET_L2_MAX_DEPTH"
echo "  PARQUET_L2_SAMPLE_MS=$PARQUET_L2_SAMPLE_MS"
echo ""

# Change to barter-rs directory
cd /Users/screener-m3/projects/barter-rs

echo "Starting barter-data-server..."
echo "Press Ctrl+C to stop early, or wait $TEST_DURATION_SECS seconds"
echo ""
echo "=============================================="
echo "  LIVE DATA COLLECTION STARTED"
echo "=============================================="
echo ""

# Run with timeout
timeout $TEST_DURATION_SECS cargo run -p barter-data-server --release 2>&1 | tee "$OUTPUT_DIR/server.log" || true

echo ""
echo "=============================================="
echo "  DATA COLLECTION COMPLETE"
echo "=============================================="
echo ""

# Validate output
echo "Checking collected data..."
echo ""

# List all parquet files
echo "=== Parquet Files Created ==="
find "$OUTPUT_DIR" -name "*.parquet" -type f | while read f; do
    size=$(ls -lh "$f" | awk '{print $5}')
    rows=$(python3 -c "import pyarrow.parquet as pq; print(pq.read_table('$f').num_rows)" 2>/dev/null || echo "?")
    echo "  $(basename $f): $size ($rows rows)"
done
echo ""

# Summary by type
echo "=== Data Summary ==="
for type in trade bar extended_bar order_book_delta; do
    count=$(find "$OUTPUT_DIR" -name "*${type}*.parquet" | wc -l | tr -d ' ')
    if [ "$count" -gt 0 ]; then
        total_rows=$(find "$OUTPUT_DIR" -name "*${type}*.parquet" -exec python3 -c "import pyarrow.parquet as pq; print(pq.read_table('{}').num_rows)" \; 2>/dev/null | awk '{sum+=$1} END {print sum}')
        echo "  $type: $count files, $total_rows total rows"
    else
        echo "  $type: 0 files"
    fi
done
echo ""

echo "=== Output Directory ==="
echo "$OUTPUT_DIR"
echo ""
ls -la "$OUTPUT_DIR"
echo ""

echo "=============================================="
echo "  NEXT STEPS"
echo "=============================================="
echo ""
echo "1. Convert to Nautilus catalog:"
echo "   python scripts/validation/setup_nautilus_catalog.py --source $OUTPUT_DIR --dest /tmp/nautilus_forward_test"
echo ""
echo "2. Run backtest on collected data:"
echo "   cd /Users/screener-m3/projects/nautilus_trader"
echo "   uv run python examples/backtest/crypto_orderbook_imbalance_parquet.py --catalog-dir /tmp/nautilus_forward_test"
echo ""
