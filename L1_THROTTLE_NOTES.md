# L1 Throttling Notes

## Background
- After L2 throttling and async mutex fixes, the next latency culprit was Binance L1 (best bid/ask) volume:
  - ~1,089 L1 msgs/sec total; ~93% from Binance (futures + spot).
  - Per-instrument rates: e.g., ETH perp ~473/sec, BTC ~156/sec, SOL ~109/sec.
- L1 feeds price_history, spread_pct, best_bid/ask, basis_history (perp vs spot). L2 imbalance is unaffected.

## Decision
- Apply a light per-instrument throttle to Binance L1 (100ms ~= 10 msgs/sec). Leave other venues as-is for now.
- Goal: reduce p99 tail latency without noticeable impact on UI (spreads/basis/mid fallback may be ~100ms behind the freshest BBO; primary price anchor remains trades).

## Trade-offs
- Pros: Cuts ~90% of Binance L1 flood; minimal code change; no TUI changes.
- Cons: L1 mid/spread can be up to 100ms stale vs the absolute latest BBO. Basis/spread displays might lag slightly in very fast markets. L2 imbalance and trade price remain real-time.

## If targets aren’t met after throttling
- Move L1 (and/or L2) to a secondary channel so the trade path is trades/liqs/OI only.
- Increase OKX-specific L2 throttle (e.g., 150–200ms) if OKX bursts remain.
- If more isolation needed: separate channels for trades, L1, L2 (more plumbing).

## Data users (for awareness)
- scalper/scalper_v2: spread and mid fallback come from L1; price anchor is trades.
- market_microstructure: basis and price trends use perp_mid/spot_mid and price_history.
- risk_scanner/institutional_flow: not L1-dependent for UI.

## Validation plan
- After throttling, re-run latency telemetry (single client, RUST_LOG=warn, 30–60s) and check p50/p95/p99.
- Targets: keep p50 ~15–20ms, p99 < 20–30ms. If tails stay high, consider moving L1 to secondary channel.
