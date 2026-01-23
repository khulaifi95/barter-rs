# Barter <-> Nautilus UDS Binary Integration Spec

Last updated: 2026-01-23
Branches:
- barter-rs: integration/nautilus-uds-binary
- nautilus_trader: integration/barter-uds-binary

## Goal
Provide a low-latency, local (same-host) binary feed from barter-data-server to NautilusTrader via Unix domain sockets (UDS) on Unix platforms, with a TCP fallback for Windows or multi-host setups. The feed should carry full-fidelity market events plus enriched analytics (orchestrator_result, CVD, OI, vol regime, etc.).

## Why this is needed
NautilusTrader does not natively provide the derivatives flow data that barter-data-server already computes (CVD, liquidation rate/state, OI delta/trend, funding velocity, vol regime, flow consensus, gamma context, orchestrator state). Integrating barter as an upstream data plant provides institutional-grade derivatives analytics in Nautilus.

## Evidence of gaps in Nautilus
1) No Unix domain socket support in network layer:
- nautilus_trader/crates/network/src/net.rs only re-exports TcpStream/TcpListener.
- No UnixStream/UnixListener usage in the repo (search for UnixStream or unix socket returns empty).

2) Custom data exists but no derivatives-flow feed:
- nautilus_trader/crates/common/src/custom.rs defines CustomData but no built-in flow analytics.
- DataEvent supports standard market data (Trade/Quote/Book/Bar) but not derivatives flow metrics.

## Scope
Phase 1 (MVP):
- UDS binary server in barter-data-server (Unix platforms).
- TCP binary server in barter-data-server (cross-platform fallback).
- Nautilus adapter that connects to UDS (Unix) or TCP (Windows/remote) and decodes messages.
- Emit CustomDataResponse (or DataEvent::Data where appropriate) into Nautilus DataEngine.

Phase 2:
- Optional: map specific barter events into native Nautilus types (TradeTick, QuoteTick, etc.) for tighter integration.
- Optional: multi-host auth/ACL or TLS for TCP deployments.

## Non-goals
- Replacing existing WS JSON feed (it remains for TUIs).
- Designing a multi-language schema (Rust-only for now).

## Transport
- Unix domain socket (UDS) for same-host low latency on Linux/macOS.
- TCP for Windows or multi-host deployments.
- UDS path configurable via env var, default: /tmp/barter-data.sock.
- TCP address configurable via env var, default: 127.0.0.1:9102.

## Binary framing
- Length-prefixed frames:
  - 4-byte big-endian length (u32)
  - payload bytes (MessagePack via rmp-serde)

## Message schema (initial)
Option A (simplest):
- Use serde + binary encoder (MessagePack via rmp-serde) on a shared enum:
  enum BarterMessage {
    Event(MarketEventMessage),
  }

Option B (more optimized later):
- Split into small fixed structs for trades/L1/L2/analytics.

## Where code changes go
barter-rs:
- barter-data-server/src/main.rs
  - Start UDS listener task
  - Broadcast BarterMessage frames to connected UDS clients
  - Keep WS path intact
- Add shared protocol module (either new crate or module in barter-data-server)

nautilus_trader:
- New adapter under crates/adapters/barter
  - Connect to UDS using tokio::net::UnixStream
  - Decode length-prefixed frames
  - Publish decoded MarketEventMessage via msgbus::publish_any on custom data topic
  - See: nautilus_trader/crates/adapters/barter/src

## Tests
- barter-data-server: unit test for framing encode/decode and UDS send loop
- nautilus_trader: unit test for frame decode and mapping to CustomDataResponse
- Integration smoke test (manual):
  - Start barter-data-server with UDS enabled
  - Run Nautilus UDS smoke test (counts decoded messages)

### Smoke test usage (manual)
1) Start server (Unix/macOS UDS):
   - `UDS_ENABLED=1 UDS_PATH=/tmp/barter-data.sock ./target/release/barter-data-server`
2) Start server (Windows or TCP fallback):
   - `TCP_ENABLED=1 TCP_ADDR=127.0.0.1:9102 ./target/release/barter-data-server`
3) Run smoke test (nautilus_trader):
   - UDS: `BARTER_UDS_PATH=/tmp/barter-data.sock BARTER_SMOKE_TARGET=200 cargo run -p nautilus-barter --bin uds_smoke`
   - TCP: `BARTER_TCP_ADDR=127.0.0.1:9102 BARTER_SMOKE_TARGET=200 cargo run -p nautilus-barter --bin uds_smoke`

### Coverage mode (modular, configurable)
Run for a fixed duration and report counts per `kind`. Optional required kinds can be enforced.

Example (report only):
- `BARTER_SMOKE_MODE=coverage BARTER_SMOKE_DURATION_SECS=20 cargo run -p nautilus-barter --bin uds_smoke`

Example (require a few kinds):
- `BARTER_SMOKE_MODE=coverage BARTER_SMOKE_DURATION_SECS=20 BARTER_SMOKE_REQUIRED_KINDS=trade,order_book_l1,cumulative_volume_delta cargo run -p nautilus-barter --bin uds_smoke`

Example (per-kind minimums):
- `BARTER_SMOKE_MODE=coverage BARTER_SMOKE_DURATION_SECS=30 BARTER_SMOKE_MIN_COUNTS=trade=100,order_book_l1=50 cargo run -p nautilus-barter --bin uds_smoke`

## Configuration (MVP)
barter-data-server:
- UDS_ENABLED=1 (default true on Unix)
- UDS_PATH=/tmp/barter-data.sock
- UDS_BUFFER=100000 (clamped 1k-500k)
- TCP_ENABLED=1 (default true on Windows)
- TCP_ADDR=127.0.0.1:9102
- TCP_BUFFER=100000 (clamped 1k-500k)

nautilus_trader (BarterDataClientConfig):
- uds_path: Option<String> (defaults to /tmp/barter-data.sock)
- tcp_addr: Option<String> (defaults to 127.0.0.1:9102 on Windows or when set)
- data_type: \"barter.market_event\"
- reconnect_delay_ms: 1000
- max_frame_bytes: 8MB

Notes:
- Setting `BARTER_TCP_ADDR` forces TCP even on Unix platforms.

## Tradeoffs (current approach)
Current: UDS + MessagePack + MarketEventMessage (serde_json::Value payload)

Pros:
- Low latency (local UDS) without changing existing data model.
- No impact to WS/TUIs or core aggregation pipeline.
- Works for all current event types without schema refactor.

Cons:
- MessagePack still allocates and traverses serde_json::Value (not zero-copy).
- Slightly higher CPU than a fully typed binary schema.
- Payload size larger than a tightly packed schema.

## Lowest-latency upgrade paths (if needed later)
1) Fully typed binary schema (no serde_json::Value)
   - Define enum/structs per event type.
   - Encode with rkyv/flatbuffers/capnp or custom packed structs.
   - Pros: lowest CPU/latency, smallest payloads.
   - Cons: largest refactor, strict versioning, dual maintenance.

2) Shared memory ring buffer (same host)
   - Single-writer / multi-reader, zero-copy read path.
   - Pros: microsecond-scale latency, minimal serialization.
   - Cons: more complex lifecycle, requires careful backpressure handling.

3) Port analytics into Nautilus (shared Rust crate)
   - Extract barter analytics into a library and embed in Nautilus.
   - Pros: zero serialization, single codebase for calcs.
   - Cons: higher integration cost, tighter coupling.

4) Keep MessagePack + UDS (current)
   - Best short-term stability with minimal risk.

## Open questions
1) Binary encoder choice locked to MessagePack (rmp-serde). OK?
2) Which subset for MVP? (All MarketEventMessage + orchestrator_result, or trades-only first?)
3) UDS path and permissions (default /tmp/barter-data.sock OK?)
4) Should Nautilus treat payloads as CustomData (raw) or map to native Data types?
5) Do we need explicit schema versioning in the header?

## Agent split (if parallel work needed)
- Agent A (barter-rs): UDS server + encoder + tests
- Agent B (nautilus_trader): adapter + decoder + DataEvent emission
