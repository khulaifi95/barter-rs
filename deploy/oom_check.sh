#!/usr/bin/env bash
# deploy/oom_check.sh — Daily OOM kill and container restart alerting
#
# Checks:
#   1. Kernel OOM kills in last 24h (journalctl -k)
#   2. Container restart counts for barter-server and barter-features
#   3. Container current state (running / exited / missing)
#
# Alerts: sends Discord webhook only when issues are detected.
# Config: reads DISCORD_WEBHOOK_URL from ~/barter/.env.monitoring
#
# NOTE: This script is read-only — it never stops or restarts containers.

set -euo pipefail

# ── Load config ───────────────────────────────────────────────
DISCORD_WEBHOOK_URL=""
if [ -f "$HOME/barter/.env.monitoring" ]; then
    # shellcheck source=/dev/null
    . "$HOME/barter/.env.monitoring"
fi

# ── Send Discord (python3 for JSON escaping, curl for HTTP) ──
send_discord() {
    local message="$1"
    if [ -z "$DISCORD_WEBHOOK_URL" ]; then
        return 0
    fi
    local payload
    payload=$(python3 -c "import json,sys; print(json.dumps({'content': sys.stdin.read()}))" <<< "$message")
    curl -s -m 10 -H "Content-Type: application/json" -d "$payload" "$DISCORD_WEBHOOK_URL" >/dev/null 2>&1 || true
}

# ── Check container health ────────────────────────────────────
check_container() {
    local name="$1"
    local alerts=""

    if ! podman inspect "$name" >/dev/null 2>&1; then
        echo "  $name: container not found"
        return 0
    fi

    local state restart_count started_at
    state=$(podman inspect "$name" --format '{{.State.Status}}' 2>/dev/null || echo "unknown")
    restart_count=$(podman inspect "$name" --format '{{.RestartCount}}' 2>/dev/null || echo "0")
    started_at=$(podman inspect "$name" --format '{{.State.StartedAt}}' 2>/dev/null || echo "unknown")

    # Truncate timestamp to readable format
    started_short=$(echo "$started_at" | cut -d. -f1 | sed 's/T/ /')

    if [ "$state" != "running" ]; then
        alerts+="$name: state=$state (expected running), started: $started_short"$'\n'
    fi

    if [ "$restart_count" -gt 0 ]; then
        alerts+="$name: $restart_count restart(s), last started: $started_short"$'\n'
    fi

    if [ -z "$alerts" ]; then
        echo "  $name: running, 0 restarts, since $started_short"
    else
        echo "$alerts"
    fi
}

# ── Main ──────────────────────────────────────────────────────
echo "=== OOM/Restart Check $(date -u '+%Y-%m-%d %H:%M UTC') ==="
alerts=""

# 1. Check kernel OOM kills in last 24h
oom_lines=$(journalctl -k --since "24 hours ago" --no-pager 2>/dev/null \
    | grep -i "Killed process" || true)
if [ -n "$oom_lines" ]; then
    oom_count=$(echo "$oom_lines" | wc -l | tr -d ' ')
    alerts+="OOM: ${oom_count} kill(s) in last 24h"$'\n'
    # Extract process names
    echo "$oom_lines" | while read -r line; do
        proc=$(echo "$line" | grep -oP "Killed process \d+ \(\K[^)]+")
        echo "  OOM killed: $proc"
    done
    alerts+="$(echo "$oom_lines" | head -3)"$'\n'
fi

# 2. Check containers
echo ""
echo "Containers:"
for svc in barter-server barter-features; do
    result=$(check_container "$svc")
    echo "$result"
    # Capture alerts (lines without leading spaces are problems)
    problems=$(echo "$result" | grep -v "^  " || true)
    if [ -n "$problems" ]; then
        alerts+="$problems"$'\n'
    fi
done

# 3. Report
echo ""
if [ -n "$alerts" ]; then
    echo "Issues found:"
    echo "$alerts"

    send_discord "🚨 **Barter OOM/Restart Alert**
\`\`\`
${alerts}\`\`\`"
else
    echo "All clear — no OOM events, no restarts, containers healthy"
fi
