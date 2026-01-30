#!/bin/bash
# Health check script for barter-data-server collector
#
# Checks the heartbeat file to verify the collector is running and producing data.
# Exit codes:
#   0 - Healthy
#   1 - Heartbeat file missing or stale
#   2 - Heartbeat file parse error
#
# Usage:
#   ./scripts/health-check.sh [heartbeat_file] [max_age_seconds]
#
# Example cron (every 5 minutes):
#   */5 * * * * /path/to/scripts/health-check.sh >> /var/log/collector-health.log 2>&1

set -e

HEARTBEAT_FILE="${1:-/tmp/collector-heartbeat.json}"
MAX_AGE_SECS="${2:-120}"  # Default: 2 minutes

# Check if jq is available
if ! command -v jq &> /dev/null; then
    echo "ERROR: jq is required but not installed"
    exit 2
fi

# Check if heartbeat file exists
if [ ! -f "$HEARTBEAT_FILE" ]; then
    echo "UNHEALTHY: Heartbeat file not found: $HEARTBEAT_FILE"
    exit 1
fi

# Parse heartbeat
LAST_WRITE=$(jq -r '.last_write' "$HEARTBEAT_FILE" 2>/dev/null)
if [ -z "$LAST_WRITE" ] || [ "$LAST_WRITE" = "null" ]; then
    echo "UNHEALTHY: Failed to parse last_write from heartbeat"
    exit 2
fi

# Convert last_write to epoch
LAST_EPOCH=$(date -j -f "%Y-%m-%dT%H:%M:%S" "${LAST_WRITE:0:19}" "+%s" 2>/dev/null || \
             date -d "${LAST_WRITE:0:19}" "+%s" 2>/dev/null)

if [ -z "$LAST_EPOCH" ]; then
    echo "UNHEALTHY: Failed to parse timestamp: $LAST_WRITE"
    exit 2
fi

# Calculate age
NOW_EPOCH=$(date "+%s")
AGE=$((NOW_EPOCH - LAST_EPOCH))

# Check if stale
if [ "$AGE" -gt "$MAX_AGE_SECS" ]; then
    echo "UNHEALTHY: Heartbeat is ${AGE}s old (max: ${MAX_AGE_SECS}s)"
    exit 1
fi

# Get additional stats
BARS_TOTAL=$(jq -r '.bars_written_total // 0' "$HEARTBEAT_FILE")
TRADES_TOTAL=$(jq -r '.trades_processed_total // 0' "$HEARTBEAT_FILE")
UPTIME=$(jq -r '.uptime_secs // 0' "$HEARTBEAT_FILE")
SYMBOL_COUNT=$(jq -r '.symbols | length' "$HEARTBEAT_FILE")

echo "HEALTHY: age=${AGE}s, bars=${BARS_TOTAL}, trades=${TRADES_TOTAL}, symbols=${SYMBOL_COUNT}, uptime=${UPTIME}s"
exit 0
