# Kline Sync Changes (Nov 29, 2025)

## What Changed
- tvVWAP/ATR/RV now come from **authoritative Binance 1m klines** (perps only), not from trade-reconstructed candles.
- Added Binance 1m kline **WebSocket** per ticker (BTC/ETH/SOL); REST backfill on startup and on reconnect only.
- Disabled trade-based candle updates when kline metrics are active; trades are still used for CVD/flow/whales.
- Kept fallback 5m trade-based path only if kline data is unavailable.
- Added rustls (ring) provider install to fix TLS “CryptoProvider” runtime warnings.

## How It Works
1) On startup: backfill 1m klines via Binance REST (fapi.binance.com) for BTC/ETH/SOL.
2) Start WS streams: `wss://fstream.binance.com/ws/{symbol}@kline_1m` (perps).
3) On reconnect: re-run REST backfill, then resume WS stream.
4) tvVWAP/ATR/RV are gated off the 1m kline buffer; “warming” clears once enough 1m klines are loaded.

## Acceptance (for verification)
- tvVWAP matches Binance futures API within ≤0.05% and stays aligned over 30–60 min.
- ATR/RV use 1m kline buffer (14-sample ATR; RV 30/60 from 1m closes).
- No “CryptoProvider” warnings at runtime.

## Branch Status
- Local branch: `feature/tui-enhancements-and-fixes` (contains the kline WS changes and rustls fix).
- Not yet merged to dev/main; coordinate merge before new OKX L2 work.

## Next Work (planned)
- Add OKX L2 support (server + TUI) on a new branch off the shared tip (dev).
- Layout revamps can sit on top once kline changes are merged.
