#!/usr/bin/env bash
# deploy/status.sh — Check barter deployment health
#
# Usage:
#   ./status.sh                  # full status report
#   ./status.sh --check-disk     # disk check only (for cron alerting)
#   ./status.sh --check-heartbeat # heartbeat watchdog (for cron alerting)

set -euo pipefail

HEARTBEAT_FILE="/data/ipc/collector-heartbeat.json"
PARQUET_DIR="/data/parquet"
FEATURES_DIR="/data/features"
CHECKPOINT_FILE="/data/_checkpoints/features_state.json"
DISK_ALERT_PERCENT=80
HEARTBEAT_STALE_SECS=300

CHECK_DISK_ONLY=false
CHECK_HEARTBEAT_ONLY=false

for arg in "$@"; do
    case "$arg" in
        --check-disk) CHECK_DISK_ONLY=true ;;
        --check-heartbeat) CHECK_HEARTBEAT_ONLY=true ;;
    esac
done

# ── Disk check (used by cron) ─────────────────────────────────
check_disk() {
    DISK_USED=$(df /data | tail -1 | awk '{gsub(/%/,""); print $5}')
    if [ "$DISK_USED" -ge "$DISK_ALERT_PERCENT" ]; then
        echo "ALERT: Disk usage at ${DISK_USED}% (threshold: ${DISK_ALERT_PERCENT}%)"
        echo "  Parquet: $(du -sh "$PARQUET_DIR" 2>/dev/null | cut -f1)"
        echo "  Consider running backup.sh or adding storage"
        return 1
    fi
    return 0
}

# ── Heartbeat watchdog (used by cron) ─────────────────────────
check_heartbeat() {
    local exit_code=0

    # Check barter-server heartbeat
    if [ -f "$HEARTBEAT_FILE" ]; then
        HEARTBEAT_AGE=$(( $(date +%s) - $(stat -c %Y "$HEARTBEAT_FILE" 2>/dev/null || stat -f %m "$HEARTBEAT_FILE" 2>/dev/null || echo 0) ))
        if [ "$HEARTBEAT_AGE" -ge "$HEARTBEAT_STALE_SECS" ]; then
            echo "ALERT: barter-server heartbeat stale (${HEARTBEAT_AGE}s ago, threshold: ${HEARTBEAT_STALE_SECS}s)"
            exit_code=1
        fi
    else
        echo "ALERT: barter-server heartbeat file missing: $HEARTBEAT_FILE"
        exit_code=1
    fi

    # Check barter-server container
    if ! podman ps --format "{{.Names}}" 2>/dev/null | grep -q "^barter-server$"; then
        echo "ALERT: barter-server container is not running"
        exit_code=1
    fi

    # Check barter-features container (if it exists)
    if podman ps -a --format "{{.Names}}" 2>/dev/null | grep -q "^barter-features$"; then
        if ! podman ps --format "{{.Names}}" 2>/dev/null | grep -q "^barter-features$"; then
            echo "ALERT: barter-features container exists but is not running"
            exit_code=1
        fi
    fi

    return $exit_code
}

if [ "$CHECK_DISK_ONLY" = true ]; then
    check_disk
    exit $?
fi

if [ "$CHECK_HEARTBEAT_ONLY" = true ]; then
    check_heartbeat
    exit $?
fi

# ── Full status report ────────────────────────────────────────
echo "=== Barter Deployment Status ==="
echo "Time: $(date -u '+%Y-%m-%d %H:%M:%S UTC')"
echo ""

# Container status
echo "── Containers ──"
podman ps -a --format "table {{.Names}}\t{{.Status}}\t{{.Created}}" 2>/dev/null || echo "  podman not available"
echo ""

# Heartbeat
echo "── Heartbeat ──"
if [ -f "$HEARTBEAT_FILE" ]; then
    HEARTBEAT_AGE=$(( $(date +%s) - $(stat -c %Y "$HEARTBEAT_FILE" 2>/dev/null || stat -f %m "$HEARTBEAT_FILE" 2>/dev/null || echo 0) ))
    if [ "$HEARTBEAT_AGE" -lt 120 ]; then
        echo "  Status: HEALTHY (last update ${HEARTBEAT_AGE}s ago)"
    else
        echo "  Status: STALE (last update ${HEARTBEAT_AGE}s ago)"
    fi
    # Show heartbeat content (compact)
    python3 -c "
import json, sys
with open('$HEARTBEAT_FILE') as f:
    hb = json.load(f)
print(f\"  Uptime:  {hb.get('uptime_secs', 0) // 3600}h {(hb.get('uptime_secs', 0) % 3600) // 60}m\")
print(f\"  Bars:    {hb.get('bars_written_total', 0)}\")
print(f\"  Trades:  {hb.get('trades_processed_total', 0)}\")
for sym, ts in hb.get('last_bars', {}).items():
    print(f\"  Last bar: {sym} → {ts}\")
" 2>/dev/null || cat "$HEARTBEAT_FILE"
else
    echo "  Status: NO HEARTBEAT FILE"
fi
echo ""

# Barter-features status
echo "── Features ──"
if podman ps --format "{{.Names}}" 2>/dev/null | grep -q "^barter-features$"; then
    echo "  Container: RUNNING"
else
    echo "  Container: NOT RUNNING"
fi
if [ -f "$CHECKPOINT_FILE" ]; then
    CKPT_AGE=$(( $(date +%s) - $(stat -c %Y "$CHECKPOINT_FILE" 2>/dev/null || stat -f %m "$CHECKPOINT_FILE" 2>/dev/null || echo 0) ))
    echo "  Checkpoint: exists (last update ${CKPT_AGE}s ago)"
    python3 -c "
import json
with open('$CHECKPOINT_FILE') as f:
    ck = json.load(f)
for k, v in ck.get('files_processed', {}).items():
    print(f'  Processed: {k}')
" 2>/dev/null || true
else
    echo "  Checkpoint: not yet created"
fi
if [ -d "$FEATURES_DIR" ]; then
    FEATURES_FILES=$(find "$FEATURES_DIR" -name "*.parquet" 2>/dev/null | wc -l)
    echo "  Feature files: $FEATURES_FILES"
else
    echo "  Feature dir: not found"
fi
echo ""

# Disk usage
echo "── Disk ──"
df -h /data 2>/dev/null | tail -1 | awk '{print "  Total: " $2 "  Used: " $3 " (" $5 ")  Free: " $4}'
echo "  Parquet:  $(du -sh "$PARQUET_DIR" 2>/dev/null | cut -f1 || echo 'N/A')"
echo "  Features: $(du -sh "$FEATURES_DIR" 2>/dev/null | cut -f1 || echo 'N/A')"
check_disk 2>/dev/null || true
echo ""

# Recent parquet files
echo "── Recent Parquet Files ──"
find "$PARQUET_DIR" -name "*.parquet" -mmin -60 2>/dev/null | head -10 | while read -r f; do
    echo "  $(basename "$f") ($(du -h "$f" | cut -f1))"
done
TOTAL_FILES=$(find "$PARQUET_DIR" -name "*.parquet" 2>/dev/null | wc -l)
echo "  Total parquet files: $TOTAL_FILES"
echo ""

# Recent logs
echo "── Recent Logs (barter-server, last 20 lines) ──"
podman logs --tail 20 barter-server 2>/dev/null || echo "  No logs available"
echo ""
echo "── Recent Logs (barter-features, last 10 lines) ──"
podman logs --tail 10 barter-features 2>/dev/null || echo "  No logs available"
