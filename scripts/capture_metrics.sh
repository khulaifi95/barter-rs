#!/usr/bin/env bash
set -euo pipefail

LOG_FILE="${1:-/tmp/data-server.log}"
OUT_FILE="${2:-/tmp/metrics_capture_$(date +%Y%m%d_%H%M%S).txt}"

if [[ ! -f "$LOG_FILE" ]]; then
  echo "Log file not found: $LOG_FILE" >&2
  exit 1
fi

{
  echo "# Metrics capture"
  echo "# Log: $LOG_FILE"
  echo "# Time: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo
  echo "## METRICS (last 10)"
  rg -n "METRICS:" "$LOG_FILE" | tail -n 10 || true
  echo
  echo "## FEEDS (last 10)"
  rg -n "FEEDS:" "$LOG_FILE" | tail -n 10 || true
  echo
  echo "## STALE warnings (last 10)"
  rg -n "STALE:" "$LOG_FILE" | tail -n 10 || true
  echo
  echo "## BACKPRESSURE warnings (last 10)"
  rg -n "BACKPRESSURE:" "$LOG_FILE" | tail -n 10 || true
} | tee "$OUT_FILE"

echo "Saved: $OUT_FILE"
