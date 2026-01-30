#!/bin/bash
# Sync S3 trading data to local storage
#
# Usage:
#   ./sync_s3_to_local.sh [--verify] [--clean-s3]
#
# Environment variables:
#   S3_BUCKET       - S3 bucket name (default: trading-data)
#   LOCAL_DATA_DIR  - Local data directory (default: ~/trading-data/recent)
#   AWS_PROFILE     - AWS CLI profile (optional)
#
# Prerequisites:
#   - AWS CLI configured with proper credentials
#   - Sufficient local disk space

set -e

# Configuration
S3_BUCKET="${S3_BUCKET:-trading-data}"
LOCAL_DATA_DIR="${LOCAL_DATA_DIR:-$HOME/trading-data/recent}"
VERIFY="${1:-}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "=============================================="
echo "S3 to Local Data Sync"
echo "=============================================="
echo ""
echo "S3 Bucket:      s3://$S3_BUCKET"
echo "Local Dir:      $LOCAL_DATA_DIR"
echo "AWS Profile:    ${AWS_PROFILE:-default}"
echo ""

# Create local directories if they don't exist
mkdir -p "$LOCAL_DATA_DIR/bars_1m"
mkdir -p "$LOCAL_DATA_DIR/trades"
mkdir -p "$LOCAL_DATA_DIR/extended_bars"

# Sync bars
echo -e "${YELLOW}Syncing 1m bars...${NC}"
aws s3 sync "s3://$S3_BUCKET/bars_1m/" "$LOCAL_DATA_DIR/bars_1m/" --delete
BARS_COUNT=$(find "$LOCAL_DATA_DIR/bars_1m" -name "*.parquet" 2>/dev/null | wc -l | tr -d ' ')
echo -e "${GREEN}  Synced $BARS_COUNT bar files${NC}"

# Sync trades
echo -e "${YELLOW}Syncing trades...${NC}"
aws s3 sync "s3://$S3_BUCKET/trades/" "$LOCAL_DATA_DIR/trades/" --delete
TRADES_COUNT=$(find "$LOCAL_DATA_DIR/trades" -name "*.parquet" 2>/dev/null | wc -l | tr -d ' ')
echo -e "${GREEN}  Synced $TRADES_COUNT trade files${NC}"

# Sync extended bars (if they exist)
echo -e "${YELLOW}Syncing extended bars...${NC}"
aws s3 sync "s3://$S3_BUCKET/extended_bars/" "$LOCAL_DATA_DIR/extended_bars/" --delete 2>/dev/null || true
EXT_COUNT=$(find "$LOCAL_DATA_DIR/extended_bars" -name "*.parquet" 2>/dev/null | wc -l | tr -d ' ')
echo -e "${GREEN}  Synced $EXT_COUNT extended bar files${NC}"

# Calculate total size
TOTAL_SIZE=$(du -sh "$LOCAL_DATA_DIR" 2>/dev/null | cut -f1)
echo ""
echo -e "${GREEN}Total synced: $TOTAL_SIZE${NC}"

# Verify if requested
if [[ "$VERIFY" == "--verify" ]]; then
    echo ""
    echo -e "${YELLOW}Running verification...${NC}"

    # Check if verify_parquet exists
    VERIFY_BIN="$(dirname "$0")/../target/release/verify_parquet"
    if [[ -f "$VERIFY_BIN" ]]; then
        "$VERIFY_BIN" "$LOCAL_DATA_DIR"
    else
        # Fallback: basic check with pyarrow
        python3 -c "
import pyarrow.parquet as pq
import os
from pathlib import Path

data_dir = Path('$LOCAL_DATA_DIR')
errors = []

for parquet_file in data_dir.rglob('*.parquet'):
    try:
        table = pq.read_table(str(parquet_file))
        print(f'  ✓ {parquet_file.name}: {table.num_rows} rows')
    except Exception as e:
        errors.append(f'{parquet_file}: {e}')
        print(f'  ✗ {parquet_file.name}: {e}')

if errors:
    print(f'\nVerification found {len(errors)} errors')
    exit(1)
else:
    print('\nVerification passed!')
"
    fi
fi

# Clean S3 if requested (after verified sync)
if [[ "$2" == "--clean-s3" ]]; then
    echo ""
    echo -e "${YELLOW}Cleaning synced files from S3 (30+ days old)...${NC}"
    # This relies on S3 lifecycle rules - just a placeholder message
    echo "  S3 lifecycle rules handle automatic cleanup"
fi

echo ""
echo "=============================================="
echo -e "${GREEN}Sync complete!${NC}"
echo "=============================================="

# Report
echo ""
echo "Summary:"
echo "  Bars:          $BARS_COUNT files"
echo "  Trades:        $TRADES_COUNT files"
echo "  Extended bars: $EXT_COUNT files"
echo "  Total size:    $TOTAL_SIZE"
