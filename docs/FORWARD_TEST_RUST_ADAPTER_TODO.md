# Forward Testing (Rust-First Adapter) — TODO & Status
**Date:** 2026-02-06
**Scope:** Barter UDS → Nautilus Rust adapter → Strategies (real-time)

## Goal
Enable real-time forward testing with minimum latency using the Rust adapter.

## Verified ✅
- UDS stream from barter-data-server starts and accepts connections.
- candle_1m is already streamed.
- extended_bar_1m is now streamed (added in barter-data-server).
- L2 order_book_deltas can be captured to parquet and loaded into Nautilus catalog.
- Nautilus orderbook imbalance backtest runs on L2 data (iterations match data range).

## TODO (Short-Term)
1. **UDS smoke validation with required kinds** ✅
   - Coverage mode passed with:
     `trade,order_book_l1,order_book_l2,candle_1m,extended_bar_1m`
   - Note: `PARQUET_WRITE_EXTENDED=true` must be set to emit `extended_bar_1m`
     even when `PARQUET_ENABLED=0`.
   - Latest coverage log:
     `/tmp/barter_uds_smoke_coverage_20260206_072443/uds_smoke.log`

2. **Forward-test runbook** ✅
   - Document minimal steps to run barter-data-server (UDS enabled) and Nautilus adapter.
   - Include env vars and expected output.
   - See: `docs/FORWARD_TEST_RUNBOOK.md`

3. **Strategy sanity check**
   - Run orderbook imbalance strategy with a longer capture window (30–60 minutes) OR
     lower thresholds for trade activity.
   - Capture output log and store in `logs/`.
4. **Python live example (thin wrapper)**
   - Add `nautilus_trader/adapters/barter` package and a minimal live script that
     wires `BarterDataClientFactory` into `TradingNode`.

## TODO (Medium-Term)
4. **Adapter robustness**
   - Add integration test that decodes each UDS kind and publishes to Nautilus bus.
   - Ensure extended_bar_1m maps to CustomData and can be subscribed in strategy.

5. **Performance review**
   - Evaluate string interning or Arc<str> for instrument IDs in hot paths.
   - Benchmark UDS throughput with `uds_bench`.

## Out of Scope (For Now)
- Feature layer (TPO/LargeTrades/ProfileEvents) streaming over UDS.
- Distributed / remote live adapter setup.

## Handoff to Opus
- Once items 1–3 pass, request Opus validation for:
  - UDS smoke results
  - Strategy run log
  - Forward-test runbook
