# Future Enhancement: Native Rust IB Integration

> **Status**: NOT A PRIORITY - Documentation only
> **Created**: 2026-01-26
> **Priority**: Low
> **Effort**: Medium (1-2 months with testing)
> **Risk**: Medium

---

## Executive Summary

This document tracks a potential future enhancement to replace the current Python-based `ibkr-bridge` with a native Rust IB client using the `rust-ibapi` crate. This would eliminate one process and reduce latency by ~2-5ms.

**Current decision: Keep the Python bridge.** The system is stable, has battle-tested reconnection logic, and the 2-5ms overhead is negligible for 5-second bar aggregation.

---

## Table of Contents

1. [Current Architecture](#current-architecture)
2. [Proposed Architecture](#proposed-architecture)
3. [Latency Analysis](#latency-analysis)
4. [Library Comparison](#library-comparison)
5. [Benefits vs Risks Analysis](#benefits-vs-risks-analysis)
6. [Why We Are NOT Doing This Now](#why-we-are-not-doing-this-now)
7. [When to Reconsider](#when-to-reconsider)
8. [IB Limitations (Important Context)](#ib-limitations-important-context)
9. [Implementation Plan](#implementation-plan)
10. [Testing Strategy](#testing-strategy)
11. [References](#references)

---

## Current Architecture

### System Overview

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                         CURRENT DATA PATH                                     │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│   IB Gateway (Java)      ibkr-bridge (Python)       barter-rs (Rust)         │
│   ┌──────────────┐      ┌──────────────────┐       ┌──────────────────┐      │
│   │  IB's code   │ TCP  │  Python script   │  WS   │  feed.rs         │      │
│   │  (Podman)    │─────►│  using ibapi     │──────►│  WebSocket client│      │
│   │  Port 4001   │      │  Port 8765       │       │  JSON parsing    │      │
│   └──────────────┘      └──────────────────┘       └──────────────────┘      │
│                                                                               │
│   COMPONENT DETAILS:                                                          │
│   ══════════════════                                                          │
│   IB Gateway:     Java application by Interactive Brokers (proprietary)      │
│   ibkr-bridge:    Custom Python script using official ibapi library          │
│   feed.rs:        Rust WebSocket client consuming JSON tick messages         │
│                                                                               │
│   METRICS:                                                                    │
│   ═════════                                                                   │
│   Processes:      3 (IB Gateway + Python bridge + Rust TUI)                  │
│   Network hops:   2 (TCP socket + WebSocket)                                 │
│   Serialization:  JSON x2 (Python serialize → Rust deserialize)              │
│   Added latency:  ~2-5ms overhead                                            │
│                                                                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Detailed Data Flow

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                         DETAILED DATA FLOW                                    │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│   IB Exchange Servers (Chicago, etc.)                                        │
│           │                                                                   │
│           │ Internet (~10-50ms depending on location)                        │
│           ▼                                                                   │
│   ┌──────────────────────────────────────────────────────────────────────┐   │
│   │  IB Gateway (Java) - Running in Podman                               │   │
│   │  • Authenticates with IB servers                                     │   │
│   │  • Maintains persistent connection                                   │   │
│   │  • Exposes TWS API on TCP socket :4001                              │   │
│   │  • Aggregates ticks internally (~250ms snapshots)                   │   │
│   └────────────────────────────┬─────────────────────────────────────────┘   │
│                                │                                              │
│                                │ TWS Protocol (TCP :4001)                    │
│                                │ Latency: ~1-2ms                             │
│                                ▼                                              │
│   ┌──────────────────────────────────────────────────────────────────────┐   │
│   │  ibkr-bridge (Python)                                                │   │
│   │  • Uses official ibapi Python library                               │   │
│   │  • Receives: EClient callbacks (tickPrice, tickSize, etc.)          │   │
│   │  • Converts to JSON: {"type":"tick","symbol":"ES","px":5000.25,...} │   │
│   │  • Serves WebSocket on :8765                                        │   │
│   │  • Handles reconnection to IB Gateway                               │   │
│   │  • Overhead: ~0.5-2ms (GIL + JSON serialize)                        │   │
│   └────────────────────────────┬─────────────────────────────────────────┘   │
│                                │                                              │
│                                │ WebSocket JSON (:8765)                      │
│                                │ Latency: ~0.1-0.5ms                         │
│                                ▼                                              │
│   ┌──────────────────────────────────────────────────────────────────────┐   │
│   │  barter-rs TUI (Rust)                                                │   │
│   │  • feed.rs: tokio-tungstenite WebSocket client                      │   │
│   │  • Deserializes JSON with serde_json                                │   │
│   │  • Updates TradMarketState                                          │   │
│   │  • Aggregates into 5-second micro-bars                              │   │
│   │  • Computes ES/NQ/BTC correlation signals                           │   │
│   └──────────────────────────────────────────────────────────────────────┘   │
│                                                                               │
│   TOTAL ADDED LATENCY (above IB's inherent latency):                         │
│   ═══════════════════════════════════════════════════                         │
│   TWS Protocol parsing:  ~1-2ms                                              │
│   Python GIL + ibapi:    ~0.5-1ms                                            │
│   JSON serialization:    ~0.2-0.5ms                                          │
│   WebSocket transport:   ~0.1-0.3ms                                          │
│   JSON deserialization:  ~0.1-0.2ms                                          │
│   ─────────────────────────────────                                           │
│   TOTAL:                 ~2-5ms                                              │
│                                                                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Current Files

| File | Purpose |
|------|---------|
| `barter-trading-tuis/src/shared/trad_markets/feed.rs` | WebSocket client connecting to ibkr-bridge |
| `barter-trading-tuis/src/shared/trad_markets/state.rs` | TradMarketState consuming tick data |
| `barter-trading-tuis/src/shared/trad_markets/widget.rs` | TUI rendering for ES/NQ panel |
| `ibkr-bridge.py` (external) | Python script forwarding IB data via WebSocket |

### Current Strengths

- **Stable and battle-tested** - Running in production
- **Automated reconnection** - Handles IB Gateway restarts, network issues
- **Stale data detection** - Warns when feeds go silent
- **Decoupled architecture** - Bridge can be restarted independently
- **Well-understood failure modes** - Logging and status indicators
- **Proven ibapi library** - Official IB Python library with large user base

---

## Proposed Architecture

### System Overview

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                         PROPOSED DATA PATH                                    │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│   IB Gateway (Java)                         barter-rs (Rust)                 │
│   ┌──────────────┐                         ┌──────────────────────┐          │
│   │  IB's code   │  TCP                    │  feed.rs             │          │
│   │  (Podman)    │────────────────────────►│  + rust-ibapi crate  │          │
│   │  Port 4001   │  Direct socket          │  Direct connection   │          │
│   └──────────────┘                         └──────────────────────┘          │
│                                                                               │
│   WHAT CHANGES:                                                               │
│   ══════════════                                                              │
│   • Python bridge REMOVED                                                    │
│   • WebSocket hop ELIMINATED                                                 │
│   • JSON serialization ELIMINATED                                            │
│   • rust-ibapi crate ADDED to Cargo.toml                                    │
│   • feed.rs REWRITTEN to use rust-ibapi                                     │
│                                                                               │
│   WHAT STAYS THE SAME:                                                        │
│   ════════════════════                                                        │
│   • IB Gateway still required (Podman)                                       │
│   • Same IB rate limits and data quality                                     │
│   • Same TradMarketState interface                                           │
│   • Same 5-second bar aggregation logic                                      │
│                                                                               │
│   METRICS:                                                                    │
│   ═════════                                                                   │
│   Processes:      2 (IB Gateway + Rust TUI)                                  │
│   Network hops:   1 (TCP socket only)                                        │
│   Serialization:  None (native Rust structs from rust-ibapi)                │
│   Added latency:  ~0.05-0.1ms                                                │
│                                                                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Detailed Data Flow (Proposed)

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                         PROPOSED DATA FLOW                                    │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│   IB Exchange Servers                                                         │
│           │                                                                   │
│           │ Internet (~10-50ms) - UNCHANGED                                  │
│           ▼                                                                   │
│   ┌──────────────────────────────────────────────────────────────────────┐   │
│   │  IB Gateway (Java) - UNCHANGED                                       │   │
│   │  • Still required (IB's proprietary software)                       │   │
│   │  • Still runs in Podman                                             │   │
│   │  • Still exposes TWS API on :4001                                   │   │
│   └────────────────────────────┬─────────────────────────────────────────┘   │
│                                │                                              │
│                                │ TWS Protocol (TCP :4001)                    │
│                                │ Latency: ~0.05-0.1ms (native parsing)       │
│                                ▼                                              │
│   ┌──────────────────────────────────────────────────────────────────────┐   │
│   │  barter-rs TUI (Rust) - ENHANCED                                     │   │
│   │  • feed.rs: Uses rust-ibapi crate directly                          │   │
│   │  • Receives native Rust structs (no JSON)                           │   │
│   │  • tokio async streams for tick data                                │   │
│   │  • Updates TradMarketState (unchanged interface)                    │   │
│   │  • Same 5-second bar aggregation                                    │   │
│   └──────────────────────────────────────────────────────────────────────┘   │
│                                                                               │
│   LATENCY SAVINGS:                                                            │
│   ════════════════                                                            │
│   Python GIL + ibapi:    ELIMINATED (-0.5-1ms)                               │
│   JSON serialization:    ELIMINATED (-0.2-0.5ms)                             │
│   WebSocket transport:   ELIMINATED (-0.1-0.3ms)                             │
│   JSON deserialization:  ELIMINATED (-0.1-0.2ms)                             │
│   Process context switch: ELIMINATED (-0.5-1ms)                              │
│   ───────────────────────────────────────                                     │
│   TOTAL SAVINGS:         ~2-5ms                                              │
│                                                                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

---

## Latency Analysis

### Latency Breakdown Comparison

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                         LATENCY COMPARISON                                    │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│   Component                    Current (Python)    Proposed (Rust)           │
│   ─────────────────────────────────────────────────────────────────          │
│   IB servers → IB Gateway      10-50ms             10-50ms (same)            │
│   IB Gateway internal          ~1-2ms              ~1-2ms (same)             │
│   TWS protocol parsing         ~0.5ms (Python)     ~0.05ms (Rust)            │
│   GIL contention               ~0.5-1ms            N/A                       │
│   JSON serialize               ~0.2-0.5ms          N/A                       │
│   WebSocket transport          ~0.1-0.3ms          N/A                       │
│   JSON deserialize             ~0.1-0.2ms          N/A                       │
│   Process context switch       ~0.5-1ms            N/A                       │
│   ─────────────────────────────────────────────────────────────────          │
│   ADDED OVERHEAD               ~2-5ms              ~0.05-0.1ms               │
│   ─────────────────────────────────────────────────────────────────          │
│   TOTAL (IB + overhead)        ~12-57ms            ~10-52ms                  │
│                                                                               │
│   SAVINGS: ~2-5ms (4-10% improvement)                                        │
│                                                                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Latency in Context

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                         LATENCY IN CONTEXT                                    │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│   YOUR USE CASE: 5-second bar aggregation                                    │
│   ═══════════════════════════════════════                                     │
│                                                                               │
│   Bar duration:        5,000ms                                               │
│   Current overhead:    2-5ms                                                 │
│   Overhead as % of bar: 0.04-0.1%                                            │
│                                                                               │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │                                                                      │   │
│   │  5-second bar                                                        │   │
│   │  ════════════════════════════════════════════════════════════════   │   │
│   │  [████████████████████████████████████████████████████████████░]    │   │
│   │   ^                                                             ^    │   │
│   │   │                                                             │    │   │
│   │   99.9% of bar time                                    0.1% overhead │   │
│   │                                                                      │   │
│   │  The 2-5ms overhead is INVISIBLE in your 5-second bars              │   │
│   │                                                                      │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                                                               │
│   FOR TICK-BY-TICK HFT:                                                      │
│   ═════════════════════                                                       │
│   Target latency:      <1ms                                                  │
│   Current overhead:    2-5ms                                                 │
│   Overhead as % of target: 200-500%  ← SIGNIFICANT                          │
│                                                                               │
│   For HFT, the overhead matters. But IB itself adds 10-50ms,                │
│   so even with native Rust, IB is not suitable for true HFT.                │
│                                                                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Comparison with Nautilus Trader

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                    NAUTILUS TRADER COMPARISON                                 │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│   NAUTILUS DATA PATH:                                                        │
│   ═══════════════════                                                         │
│   IB Gateway → ibapi (Python/Cython) → Nautilus core (same process)         │
│                                                                               │
│   • No WebSocket hop                                                         │
│   • No JSON serialization                                                    │
│   • In-process function calls                                                │
│   • Cython optimized hot paths                                               │
│   • Added overhead: ~0.1ms                                                   │
│                                                                               │
│   COMPARISON:                                                                │
│   ───────────────────────────────────────────────────────────────            │
│   │ System              │ IB Overhead │ Language      │ Maturity │          │
│   ├─────────────────────┼─────────────┼───────────────┼──────────┤          │
│   │ Current (bridge)    │ ~2-5ms      │ Python + Rust │ High     │          │
│   │ Proposed (rust-ibapi)│ ~0.1ms      │ Pure Rust     │ Medium   │          │
│   │ Nautilus            │ ~0.1ms      │ Python/Cython │ High     │          │
│   ───────────────────────────────────────────────────────────────            │
│                                                                               │
│   Nautilus achieves similar latency to rust-ibapi because it also           │
│   eliminates the IPC/serialization overhead by keeping ibapi in-process.    │
│                                                                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

---

## Library Comparison

### Rust IB Libraries Evaluated

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                         RUST IB LIBRARIES                                     │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │  rust-ibapi (wboayue)  ★★★★☆  RECOMMENDED                           │   │
│   │  https://github.com/wboayue/rust-ibapi                              │   │
│   ├─────────────────────────────────────────────────────────────────────┤   │
│   │  Stars: 257 │ Commits: 609 │ License: MIT                          │   │
│   │                                                                      │   │
│   │  FEATURES:                                                          │   │
│   │  ✓ Tokio async/await native                                        │   │
│   │  ✓ Real-time market data streaming                                 │   │
│   │  ✓ Historical data requests                                        │   │
│   │  ✓ Order management                                                │   │
│   │  ✓ Account information                                             │   │
│   │  ✓ Contract builders (futures, options, forex)                     │   │
│   │  ✓ Actively maintained (609 commits)                               │   │
│   │                                                                      │   │
│   │  CONCERNS:                                                          │   │
│   │  • Community library (not official IB)                             │   │
│   │  • 257 stars vs Python ibapi's wider adoption                      │   │
│   │  • Less battle-tested in production                                │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                                                               │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │  IBKR-API-Rust (sparkstart)  ★★★☆☆  BACKUP OPTION                   │   │
│   │  https://github.com/sparkstartconsulting/IBKR-API-Rust              │   │
│   ├─────────────────────────────────────────────────────────────────────┤   │
│   │  Stars: 170 │ Commits: 153 │ License: MIT                          │   │
│   │                                                                      │   │
│   │  FEATURES:                                                          │   │
│   │  ✓ Port of official IB API (v9.76.01)                              │   │
│   │  ✓ EClient/EWrapper pattern (familiar to ibapi users)              │   │
│   │  ✗ Synchronous only (no async/await)                               │   │
│   │  ✗ Not on crates.io                                                │   │
│   │                                                                      │   │
│   │  CONCERNS:                                                          │   │
│   │  • No async support (thread-based)                                 │   │
│   │  • Less actively maintained                                        │   │
│   │  • Would require wrapping for tokio compatibility                  │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                                                               │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │  ib-rs (dylanmckay)  ★★☆☆☆  NOT RECOMMENDED                         │   │
│   │  https://github.com/dylanmckay/ib-rs                                │   │
│   ├─────────────────────────────────────────────────────────────────────┤   │
│   │  • Low activity                                                     │   │
│   │  • Limited features                                                 │   │
│   │  • Not production ready                                            │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                                                               │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │  ibkr_client_portal  ★★☆☆☆  DIFFERENT USE CASE                      │   │
│   │  https://lib.rs/crates/ibkr_client_portal                           │   │
│   ├─────────────────────────────────────────────────────────────────────┤   │
│   │  • REST API client (not TWS socket API)                            │   │
│   │  • Higher latency than socket API                                  │   │
│   │  • Different authentication model                                  │   │
│   │  • Not suitable for real-time tick data                            │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                                                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Official IB Support

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                    OFFICIAL IB API SUPPORT                                    │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│   Languages officially supported by Interactive Brokers:                     │
│                                                                               │
│   ✓ Java        - Primary, most complete                                    │
│   ✓ C++         - Full support                                              │
│   ✓ C#          - Full support                                              │
│   ✓ Python      - Full support (ibapi package)                              │
│   ✗ Rust        - NOT officially supported                                  │
│                                                                               │
│   All Rust libraries are community-created ports/implementations.            │
│   IB does not provide official Rust support or bindings.                    │
│                                                                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

---

## Benefits vs Risks Analysis

### Benefits

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                              BENEFITS                                         │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│   LATENCY IMPROVEMENT                                                        │
│   ═══════════════════                                                         │
│   • Save ~2-5ms per tick                                                     │
│   • Relevant for: tick-by-tick strategies, order execution                  │
│   • Irrelevant for: 5-second bar aggregation (current use case)             │
│                                                                               │
│   OPERATIONAL SIMPLICITY                                                     │
│   ══════════════════════                                                      │
│   • 2 processes instead of 3                                                 │
│   • No Python runtime dependency for IB                                      │
│   • One fewer WebSocket connection to monitor                                │
│   • Simpler deployment scripts                                               │
│                                                                               │
│   PURE RUST STACK                                                            │
│   ═══════════════                                                             │
│   • Single language for entire trading system                                │
│   • Unified error handling                                                   │
│   • No Python/Rust interop complexity                                        │
│   • Easier debugging (single process)                                        │
│                                                                               │
│   RESOURCE EFFICIENCY                                                        │
│   ═══════════════════                                                         │
│   • No Python interpreter memory overhead                                    │
│   • No GIL contention                                                        │
│   • Lower CPU usage (no JSON parsing)                                        │
│                                                                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Risks

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                                RISKS                                          │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│   LIBRARY MATURITY                                                           │
│   ════════════════                                                            │
│   Risk: rust-ibapi is community library with 257 stars                       │
│   Impact: May have undiscovered bugs, edge cases                             │
│   Mitigation: Extensive parallel testing before production                   │
│   Severity: MEDIUM                                                           │
│                                                                               │
│   API SURFACE DIFFERENCES                                                    │
│   ═══════════════════════                                                     │
│   Risk: Different from current WebSocket message format                      │
│   Impact: Requires rewriting feed.rs, potential bugs                         │
│   Mitigation: Careful implementation, thorough testing                       │
│   Severity: MEDIUM                                                           │
│                                                                               │
│   CONTRACT SYMBOLOGY                                                         │
│   ═══════════════════                                                         │
│   Risk: ES/NQ front-month symbols change quarterly                          │
│   Impact: May need different handling than Python bridge                     │
│   Mitigation: Test across contract rollovers                                │
│   Severity: LOW-MEDIUM                                                       │
│                                                                               │
│   RECONNECTION LOGIC                                                         │
│   ══════════════════                                                          │
│   Risk: Current Python bridge has battle-tested reconnection                │
│   Impact: Must reimplement in Rust, may miss edge cases                     │
│   Mitigation: Port existing logic carefully, extensive testing              │
│   Severity: MEDIUM                                                           │
│                                                                               │
│   BREAKING STABLE SYSTEM                                                     │
│   ══════════════════════                                                      │
│   Risk: Current system works well, change introduces risk                   │
│   Impact: Potential production issues, debugging time                        │
│   Mitigation: Feature flag, gradual rollout, easy rollback                  │
│   Severity: MEDIUM-HIGH                                                      │
│                                                                               │
│   MAINTENANCE BURDEN                                                         │
│   ══════════════════                                                          │
│   Risk: rust-ibapi may lag behind IB API updates                            │
│   Impact: May need to contribute fixes or wait for updates                  │
│   Mitigation: Monitor library activity, have fallback plan                  │
│   Severity: LOW                                                              │
│                                                                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Risk/Benefit Matrix

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                         RISK/BENEFIT MATRIX                                   │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│                           LOW RISK ◄────────────────► HIGH RISK              │
│                                                                               │
│   HIGH     │  ┌─────────────────┐                                           │
│   BENEFIT  │  │ IDEAL CHANGES   │                                           │
│            │  │ (none in this   │                                           │
│            │  │  case)          │                                           │
│            │  └─────────────────┘                                           │
│      ▲     │                                                                 │
│      │     │                                                                 │
│      │     │                     ┌─────────────────────────┐                │
│      │     │                     │ NATIVE RUST IB          │                │
│      │     │                     │ • Medium risk           │                │
│      │     │                     │ • Low-medium benefit    │                │
│      │     │                     │ • For 5s bars: marginal │                │
│      ▼     │                     └─────────────────────────┘                │
│            │                                                                 │
│   LOW      │  ┌─────────────────┐                                           │
│   BENEFIT  │  │ KEEP CURRENT    │                                           │
│            │  │ • Zero risk     │                                           │
│            │  │ • Works fine    │ ← WE ARE HERE                             │
│            │  └─────────────────┘                                           │
│                                                                               │
│   CONCLUSION: Migration is in "medium risk, low-medium benefit" quadrant.   │
│   Not compelling for current use case (5-second bars).                      │
│                                                                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

---

## Why We Are NOT Doing This Now

### Primary Reasons

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                    WHY NOT NOW - DETAILED REASONING                           │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│   1. CURRENT SYSTEM WORKS WELL                                               │
│   ════════════════════════════                                                │
│   • Stable in production                                                     │
│   • Handles reconnection gracefully                                          │
│   • Stale data detection works                                               │
│   • No reported issues with current latency                                  │
│                                                                               │
│   "If it ain't broke, don't fix it"                                         │
│                                                                               │
│   ───────────────────────────────────────────────────────────────────────    │
│                                                                               │
│   2. LATENCY SAVINGS ARE IRRELEVANT FOR USE CASE                            │
│   ══════════════════════════════════════════════                              │
│   • We aggregate into 5-second bars                                          │
│   • 2-5ms overhead = 0.04-0.1% of bar duration                              │
│   • This is statistical noise, not a real problem                           │
│   • Correlation signals don't need sub-millisecond precision                │
│                                                                               │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │  5000ms bar                                                          │   │
│   │  [████████████████████████████████████████████████████████████░]    │   │
│   │                                                           ^^^        │   │
│   │                                                           2-5ms      │   │
│   │                                                           (noise)    │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                                                               │
│   ───────────────────────────────────────────────────────────────────────    │
│                                                                               │
│   3. RISK VS REWARD DOESN'T JUSTIFY                                         │
│   ═════════════════════════════════                                           │
│   • Medium risk of breaking stable system                                    │
│   • Low-medium benefit (imperceptible latency improvement)                  │
│   • 1-2 months of testing effort                                            │
│   • Opportunity cost: could work on higher-value features                   │
│                                                                               │
│   ───────────────────────────────────────────────────────────────────────    │
│                                                                               │
│   4. IB IS THE BOTTLENECK, NOT THE BRIDGE                                   │
│   ═══════════════════════════════════════                                     │
│   • IB adds 10-50ms inherent latency                                        │
│   • IB aggregates ticks internally (~250ms snapshots)                       │
│   • Even with native Rust, IB is still retail infrastructure               │
│   • Optimizing 2-5ms when IB adds 10-50ms is premature                     │
│                                                                               │
│   ───────────────────────────────────────────────────────────────────────    │
│                                                                               │
│   5. LIBRARY MATURITY CONCERNS                                              │
│   ════════════════════════════                                                │
│   • rust-ibapi has 257 stars (vs ibapi's massive user base)                 │
│   • Community library, not official IB support                              │
│   • May have undiscovered edge cases                                        │
│   • Prefer to wait for more production users/feedback                       │
│                                                                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

### What We Would Lose

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                    WHAT WE WOULD LOSE BY MIGRATING                            │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│   • Battle-tested reconnection logic                                         │
│   • Known and understood failure modes                                       │
│   • Stability of current production system                                   │
│   • Time that could be spent on higher-value features                       │
│   • Decoupled architecture (bridge restarts don't affect TUI)               │
│   • Ability to debug bridge separately from TUI                             │
│                                                                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

---

## When to Reconsider

### Decision Criteria

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                    WHEN TO RECONSIDER THIS MIGRATION                          │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│   RECONSIDER IF ANY OF THESE BECOME TRUE:                                    │
│                                                                               │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │  1. MOVING TO TICK-BY-TICK STRATEGIES                               │   │
│   │     If we need individual tick processing (not 5s bars),            │   │
│   │     the 2-5ms overhead becomes significant (~50% of tick interval)  │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                                                               │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │  2. PYTHON BRIDGE BECOMES MAINTENANCE BURDEN                        │   │
│   │     If we're spending significant time debugging/maintaining        │   │
│   │     the Python bridge, native Rust may reduce total effort          │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                                                               │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │  3. OPERATIONAL COMPLEXITY CAUSING ISSUES                           │   │
│   │     If managing 3 processes is causing deployment/monitoring pain   │   │
│   │     and simplification to 2 processes would help significantly      │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                                                               │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │  4. rust-ibapi REACHES HIGHER MATURITY                              │   │
│   │     When library has 500+ stars, more production users,             │   │
│   │     and proven stability, the risk decreases                        │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                                                               │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │  5. IB ADDS EXECUTION FEATURES                                      │   │
│   │     If we start placing orders through IB (not just data),          │   │
│   │     native Rust for order execution latency may matter more         │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                                                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Maturity Indicators to Watch

| Indicator | Current | Target Before Migrating |
|-----------|---------|-------------------------|
| rust-ibapi GitHub stars | 257 | 500+ |
| Production user testimonials | Few | Multiple documented |
| Years since v1.0 release | ~1 | 2+ |
| Open issues/bugs | ? | Minimal critical issues |
| Commit activity | Active | Consistently active |

---

## IB Limitations (Important Context)

### Why IB Is Not Suitable for True HFT

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                    INTERACTIVE BROKERS LIMITATIONS                            │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│   Even with native Rust, these IB limitations remain:                        │
│                                                                               │
│   LATENCY LIMITS                                                             │
│   ══════════════                                                              │
│   • IB servers → you: 10-50ms (retail internet path)                        │
│   • IB internal tick aggregation: ~250ms snapshots                          │
│   • No sub-millisecond data available                                        │
│   • No colocation offered by IB                                              │
│                                                                               │
│   RATE LIMITS                                                                │
│   ═══════════                                                                 │
│   • 50 messages/second max to TWS                                           │
│   • 20 active orders per contract per side                                  │
│   • Historical data pacing (60 requests/10 min)                             │
│   • Order Efficiency Ratio (OER) monitoring                                 │
│                                                                               │
│   DATA QUALITY                                                               │
│   ════════════                                                                │
│   • IB states: "we are not a specialized market data provider"              │
│   • Ticks are aggregated, not true tick-by-tick                            │
│   • Some data delayed/throttled during high volume                          │
│                                                                               │
│   BOTTOM LINE                                                                │
│   ═══════════                                                                 │
│   Optimizing the bridge from 2-5ms to 0.1ms doesn't change the fact        │
│   that IB itself adds 10-50ms. For true HFT (<1ms), you need:              │
│   • Direct Market Access (DMA)                                              │
│   • Colocation at exchange                                                   │
│   • FIX protocol connections                                                │
│   • Specialized market data (Databento, etc.)                               │
│                                                                               │
│   IB is suitable for: swing trading, position trading, MFT (seconds+)       │
│   IB is NOT suitable for: HFT, market making, sub-second strategies         │
│                                                                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

---

## Implementation Plan

> **Note**: This section is for future reference. Do not implement until decision criteria are met.

### Phase 1: Create Test Binary (Zero Risk)

Create a separate binary that doesn't affect production:

```rust
// NEW FILE: barter-trading-tuis/src/bin/test_ib_native.rs

use ibapi::Client;
use ibapi::contracts::Contract;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    println!("Testing native IB connection...");
    let start = Instant::now();

    // Connect to IB Gateway
    let client = Client::connect("127.0.0.1:4001", 200).await?;
    println!("✓ Connected in {:?}", start.elapsed());

    // Subscribe to ES
    let es = Contract::future("ES").exchange("CME");
    let mut ticks = client.req_tick_by_tick_data(&es, "AllLast", 0, false)?;

    println!("✓ Subscribed to ES, receiving ticks...");

    let mut count = 0;
    while let Some(tick) = ticks.next().await {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        println!(
            "Tick #{}: price={:.2}, size={}, latency={}ms",
            count, tick.price, tick.size, now - tick.time.timestamp_millis() as u128
        );
        count += 1;
        if count > 100 {
            break;
        }
    }

    Ok(())
}
```

**Add to Cargo.toml:**

```toml
[dependencies]
ibapi = "1.0"  # Check crates.io for latest version

[[bin]]
name = "test_ib_native"
path = "src/bin/test_ib_native.rs"
```

### Phase 2: Parallel Logging (Zero Risk)

Run both systems simultaneously, compare latency.

### Phase 3: Feature Flag Implementation (Low Risk)

```rust
// feed.rs - Add feature flag, keep both implementations

pub fn spawn_ib_feed(...) -> JoinHandle<()> {
    let use_native = std::env::var("USE_NATIVE_IB")
        .map(|v| v == "1")
        .unwrap_or(false);  // Default: use Python bridge

    if use_native {
        spawn_native_ib_feed(state, status_tx)  // New code
    } else {
        spawn_ibkr_bridge_feed(state, status_tx)  // Current code (unchanged)
    }
}
```

### Phase 4: Full Native Implementation

Detailed implementation in `feed.rs` with reconnection logic.

### Phase 5: Production Switch

Gradual rollout with easy rollback.

### Phase 6: Cleanup

Remove Python bridge code after confidence.

---

## Testing Strategy

### Testing Checklist

- [ ] Test binary connects to IB Gateway successfully
- [ ] ES tick data received correctly
- [ ] NQ tick data received correctly
- [ ] Reconnection works after IB Gateway restart
- [ ] Reconnection works after network interruption
- [ ] Handles market close/open transitions
- [ ] Handles contract rollover (quarterly)
- [ ] Stale data detection works
- [ ] Latency improvement confirmed (compare logs)
- [ ] No memory leaks after 24h run
- [ ] No CPU spikes or degradation
- [ ] TradMarketState updates correctly
- [ ] Correlation signals compute correctly
- [ ] TUI displays data correctly
- [ ] Paper trading tested for 1 week
- [ ] Live trading tested for 1 week (monitored)

### Rollback Plan

```bash
# If issues occur, rollback is simple:
# 1. Set environment variable
export USE_NATIVE_IB=0

# 2. Restart TUI
cargo run --bin scalper-v2

# 3. Start Python bridge again
python ibkr-bridge.py

# System returns to current stable state
```

---

## Timeline Estimate

| Phase | Duration | Risk Level | Prerequisites |
|-------|----------|------------|---------------|
| 1. Test binary | 1 day | Zero | None |
| 2. Parallel logging | 1-2 weeks | Zero | Phase 1 |
| 3. Feature flag | 1 day | Low | Phase 2 |
| 4. Native implementation | 2-3 days | Low | Phase 3 |
| 5. Opt-in testing | 2-4 weeks | Low | Phase 4 |
| 6. Production switch | 1 day | Medium | Phase 5 success |
| 7. Cleanup | 1 day | Low | Phase 6 stable |

**Total: 6-8 weeks** (with proper testing)

---

## Environment Variables

### Current

| Variable | Default | Purpose |
|----------|---------|---------|
| `IBKR_BRIDGE_WS_URL` | `ws://127.0.0.1:8765/ws` | Python bridge WebSocket |

### After Migration

| Variable | Default | Purpose |
|----------|---------|---------|
| `IBG_HOST` | `127.0.0.1` | IB Gateway host |
| `IBG_PORT` | `4001` | IB Gateway port (4001=live, 4002=paper) |
| `USE_NATIVE_IB` | `0` | Feature flag (set to `1` to enable) |

---

## References

### Libraries

- [rust-ibapi GitHub](https://github.com/wboayue/rust-ibapi)
- [IBKR-API-Rust GitHub](https://github.com/sparkstartconsulting/IBKR-API-Rust)
- [IB TWS API Documentation](https://interactivebrokers.github.io/tws-api/)

### Current Implementation

- [feed.rs](../barter-trading-tuis/src/shared/trad_markets/feed.rs)
- [state.rs](../barter-trading-tuis/src/shared/trad_markets/state.rs)
- [ES/BTC Correlation Spec](./ES_BTC_CORRELATION_SPEC.md)

### Related Documentation

- [IB Order Limitations](https://interactivebrokers.github.io/tws-api/order_limitations.html)
- [IB Historical Data Limitations](https://interactivebrokers.github.io/tws-api/historical_limitations.html)

---

## Changelog

| Date | Change |
|------|--------|
| 2026-01-26 | Initial documentation created |

---

> **Final Note**: This document serves as a reference for potential future work. The decision to proceed should be based on the criteria outlined in [When to Reconsider](#when-to-reconsider). Do not prioritize this migration unless those criteria are clearly met.
