# Latency Baseline (Capture Template)

Purpose: capture a repeatable 10-minute snapshot of server health and latency metrics
so we can compare refactors over time.

## Prereqs
- Run the server with consistent env vars (especially WS_BINARY_FRAMES, WS_ENVELOPE, AGG_EVENT_BUFFER).
- Keep ticker set and connected TUIs consistent across runs.

## Capture Steps (10 minutes)
1) Start server:
   RUST_LOG=info ./target/release/barter-data-server > /tmp/data-server.log 2>&1 &

2) Start TUIs (same set each run).

3) Wait 10 minutes.

4) Save metrics snapshot:
   ./scripts/capture_metrics.sh

5) Record system snapshot:
   - CPU/RSS for barter-data-server (ps/top)
   - Number of connected TUIs
   - WS_BINARY_FRAMES and WS_ENVELOPE values

## What to compare between runs
- METRICS: trades/min, skew_avg/min/max
- FEEDS: per-exchange rates, agg_dropped
- CPU and RSS
- TUI lag and reconnect frequency

## Notes
- skew_avg is positive when server is behind exchange time.
- skew_min can be negative (exchange clock ahead).
- agg_dropped > 0 indicates backpressure in aggregator channel.
