# Data Provenance & Tradeoffs

This system intentionally separates **system-of-record market data** from **UI-grade
visualizations**. The goal is consistent research/backtest/forward-test results while
still giving traders fast, readable real-time views.

## Canonical Data (System of Record)

These sources are the backbone for research, backtesting, and forward testing:

- **Trade stream** (normalized from exchange WSS)
- **Deterministic aggregation** (1m bars built from trades)
- **Extended bars** (CVD, OI snapshot + bar-to-bar delta, funding snapshot, L1/L2 snapshots)
- **Parquet storage** (authoritative persistence for research/backtest)

If a metric influences trading decisions, it must come from this canonical pipeline.

## UI/Visualization Data (Experimental & Advisory)

The TUIs are designed as **experimental visual cues** for traders. They can mix
sources for clarity and responsiveness:

- **Normalized WS feed** (from the server at `ws://127.0.0.1:9001`)
- **Exchange klines** (e.g., Binance 1m/5m for tvVWAP/ATR/RV)
- **REST-only sources** where WSS lacks fields (e.g., OI snapshots, options chain)
- **Rolling windows and derived signals** (divergence, whale detection, velocity)

These views are not guaranteed to match Parquet 1m bars or backtest inputs.

## Why This Separation Exists

- **Normalization** reduces noise and makes human decisions safer and faster.
- **Kline/REST data** fills gaps when WSS lacks a field or is noisy.
- **UI-grade metrics** optimize clarity and timing, not auditability.

## Tradeoffs

- **Trade-aggregated bars vs exchange klines**: klines can differ from trade-based bars
  (different definitions, timing, rounding, and volume). This is expected.
- **Low latency vs completeness**: WSS is fast but not always complete; REST backfills
  are slower but authoritative for certain fields.
- **UI flexibility vs research rigor**: UI metrics can be experimental; research should
  stay on canonical pipelines.

## Promotion Rule (When Signals Become "Real")

A signal starts in the TUI layer as an experiment. It becomes "real" only after:

1. It is verified against canonical trade-based data.
2. It is validated in research/backtest with documented assumptions.
3. It is productionized in the server/Parquet pipeline.

Until then, it is **visual-only** and must not drive automated trades.

## Summary

- **Parquet = system of record**
- **TUIs = experimental/advisory views**
- **Trading logic = canonical data only**

