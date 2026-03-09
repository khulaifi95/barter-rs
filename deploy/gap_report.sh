#!/usr/bin/env bash
# deploy/gap_report.sh — Row-level daily gap report
#
# Audits bars_1m, extended_bars_1m, trades, and features via pyarrow
# (inside jupyter container). Classifies gaps as:
#   - WS disconnect (trades + bars both missing) → informational
#   - Pipeline bug  (trades exist, bar missing)  → critical alert
#   - Extended bars mismatch (bars != ext rows)  → alert
#   - Feature issues (missing/empty files)       → alert
#
# Usage:
#   ./gap_report.sh                # audit yesterday (default)
#   ./gap_report.sh --max-days 7   # audit last 7 days
#   ./gap_report.sh --threshold 20 # alert if WS gaps > 20 minutes
#
# Alerts: Discord webhook only when issues detected.
# Config: reads DISCORD_WEBHOOK_URL from ~/barter/.env.monitoring

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# ── Defaults ──────────────────────────────────────────────────
MAX_DAYS=1
GAP_ALERT_MINUTES=10   # alert if WS gaps > N minutes

# ── Parse args ────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case "$1" in
        --max-days) MAX_DAYS="$2"; shift 2 ;;
        --threshold) GAP_ALERT_MINUTES="$2"; shift 2 ;;
        *) shift ;;
    esac
done

# ── Load config ───────────────────────────────────────────────
DISCORD_WEBHOOK_URL=""
HC_GAP_URL=""
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

# ── Ensure jupyter container is running ───────────────────────
if ! podman ps --format "{{.Names}}" 2>/dev/null | grep -q "^jupyter$"; then
    echo "Starting jupyter container for pyarrow access..."
    podman-compose -f "$HOME/barter/compose.yaml" -f "$HOME/barter/compose.prod.yaml" up -d jupyter 2>/dev/null
    sleep 5
    if ! podman ps --format "{{.Names}}" 2>/dev/null | grep -q "^jupyter$"; then
        echo "ERROR: jupyter container failed to start — cannot run row-level audit"
        exit 1
    fi
fi

# ── Run row-level audit inside jupyter container ──────────────
audit_output=$(podman exec -i jupyter python3 -u - "$MAX_DAYS" "$GAP_ALERT_MINUTES" < "$SCRIPT_DIR/gap_audit.py" 2>&1)
audit_rc=$?

echo "=== Gap Report $(date -u '+%Y-%m-%d %H:%M UTC') ==="

if [ "$audit_rc" -ne 0 ]; then
    echo "ERROR: audit script failed:"
    echo "$audit_output"
    exit 1
fi

status=$(echo "$audit_output" | grep "^STATUS:" | head -1 | cut -d: -f2)
if [ "$status" = "ERROR" ]; then
    msg=$(echo "$audit_output" | grep "^MSG:" | cut -d: -f2-)
    echo "ERROR: $msg"
    exit 1
fi

# Extract metrics
headline=$(echo "$audit_output" | grep "^HEADLINE:" | cut -d: -f2-)
ws_gaps=$(echo "$audit_output" | grep "^WS_GAPS:" | cut -d: -f2)
pipeline_gaps=$(echo "$audit_output" | grep "^PIPELINE_GAPS:" | cut -d: -f2)
ext_mismatches=$(echo "$audit_output" | grep "^EXT_MISMATCHES:" | cut -d: -f2)
feature_issues=$(echo "$audit_output" | grep "^FEATURE_ISSUES:" | cut -d: -f2)

# Print headline + summary
echo "$headline"
echo ""
echo "$audit_output" | grep "^SUMMARY:" | sed 's/^SUMMARY:/  /'
echo ""

alerts=$(echo "$audit_output" | grep "^ALERT:" | sed 's/^ALERT://')
if [ -n "$alerts" ]; then
    echo "Details:"
    echo "$alerts"
    echo ""
fi

# ── Alerting logic (priority order) ──────────────────────────

has_alert=false

# Pipeline bugs are always critical
if [ "$pipeline_gaps" -gt 0 ]; then
    has_alert=true
    pipeline_detail=$(echo "$alerts" | grep "PIPELINE" || true)
    discord_msg="🚨 **Barter Gap Report** — Pipeline issue
${headline}
\`\`\`
${pipeline_detail}
\`\`\`
Trades exist but bars missing — investigate bar aggregator."
    echo "CRITICAL: $pipeline_gaps minute(s) with trades but no bar"
    send_discord "$discord_msg"
fi

# Extended bars mismatch
if [ "$ext_mismatches" -gt 0 ]; then
    has_alert=true
    ext_detail=$(echo "$alerts" | grep "EXT" || true)
    discord_msg="⚠️ **Barter Gap Report** — Extended bars mismatch
${headline}
\`\`\`
${ext_detail}
\`\`\`"
    echo "ALERT: $ext_mismatches extended bars mismatch(es)"
    send_discord "$discord_msg"
fi

# Feature issues
if [ "$feature_issues" -gt 0 ]; then
    has_alert=true
    feat_detail=$(echo "$alerts" | grep "FEAT" || true)
    discord_msg="⚠️ **Barter Gap Report** — Feature output issues
${headline}
\`\`\`
${feat_detail}
\`\`\`"
    echo "ALERT: $feature_issues feature issue(s)"
    send_discord "$discord_msg"
fi

# WS gaps above threshold
if [ "$ws_gaps" -gt "$GAP_ALERT_MINUTES" ]; then
    has_alert=true
    summary=$(echo "$audit_output" | grep "^SUMMARY:" | sed 's/^SUMMARY://')
    ws_detail=$(echo "$alerts" | grep "WS drop" || true)
    discord_msg="⚠️ **Barter Gap Report** — ${ws_gaps} min WS disconnects
${headline}
\`\`\`
${summary}

${ws_detail}
\`\`\`
Trades + bars both missing — upstream exchange drops."
    echo "ALERT: $ws_gaps minutes of WS disconnects (threshold: $GAP_ALERT_MINUTES)"
    send_discord "$discord_msg"
fi

# All clear
if [ "$has_alert" = false ]; then
    echo "OK: ${ws_gaps} min WS gaps, ${pipeline_gaps} pipeline, ${ext_mismatches} ext mismatch, ${feature_issues} feat issues"
    if [ -n "$HC_GAP_URL" ]; then
        curl -fsS -m 10 --retry 3 "$HC_GAP_URL/0" >/dev/null 2>&1 || true
    fi
fi
