# Handoff Context: ETH/SOL CVD missing & server config

## Current state
- Reworked TUIs to use shared aggregator and trade-derived CVD (primary path). Basis deadband added; funding still n/a.
- Added `barter-trading-tuis/src/shared/state.rs` aggregator; used by all three TUIs.
- The server (`barter-data-server`) is new in this branch (not in origin/main). It subscribes to BTC/ETH/SOL perps on Binance/Bybit/OKX and spot trades for basis; OKX spot L1 removed to avoid panic.
- Running server started with `RUST_LOG=info,barter_data=debug,barter_data_server=debug cargo run -p barter-data-server > /tmp/barter-server.log 2>&1` (PID may change). Port 9001.
- Live log shows zero ETH/SOL events; only BTC flows. Grepping `/tmp/barter-server.log` for `eth/usdt` or `sol/usdt` returns nothing.
- CVD divergence panel shows BETC data; ETH/SOL show ALIGNED when tiny deltas exist; CVD is derived from trades if exchange CVD is absent.

## Suspected issue
- Upstream ETH/SOL perp trade streams are not delivering; not a casing issue (connectors handle lower/upper/hyphen per exchange).
- The code additions (server, aggregator) are local; origin/main was BTC-only. OKX spot L1 panic was introduced locally and fixed by skipping that sub.

## What needs to be done
- Confirm ETH/SOL perp trades actually reach the server. After restart, `rg -i "eth/usdt" /tmp/barter-server.log | head` and same for SOL. If empty, investigate connection errors in logs for those streams.
- Ensure current build is running (port 9001). Older instance could be missing ETH/SOL subs.
- If exchanges are sending (they should), check network/region constraints or missed reconnections.
- Once trades flow, CVD/orderflow/liqs for ETH/SOL will populate automatically via trade-derived CVD.

## Files changed
- Added aggregator: `barter-trading-tuis/src/shared/state.rs`, re-exported in lib and used by TUIs.
- Server config: `barter-data-server/src/main.rs` (new in this branch) with ETH/SOL subs; OKX spot L1 removed.
- TUIs updated to use shared snapshot and show data-quality warnings.

## Commands used
- Start server: `pkill -f barter-data-server || true; RUST_LOG=info,barter_data=debug,barter_data_server=debug cargo run -p barter-data-server > /tmp/barter-server.log 2>&1`
- Check ETH/SOL flow: `rg -i "eth/usdt" /tmp/barter-server.log | head` and `rg -i "sol/usdt" /tmp/barter-server.log | head`
