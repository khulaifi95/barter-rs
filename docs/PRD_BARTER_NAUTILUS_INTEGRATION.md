# PRD: Barter-Nautilus Integration Architecture

**Version:** 1.1
**Last Updated:** 2026-01-29
**Author:** Architecture Team
**Status:** Draft - Updated after Opus review

---

## Executive Summary

This document defines the architecture and phased plan for integrating **Barter-RS** (real-time execution brain) with **Nautilus Trader** (backtesting engine) while keeping live execution inside Barter. The pipeline is **Rust-only** for lowest latency and operational simplicity. The initial rollout is **Binance-only** with **1m bars + raw trades**, then scales to **Bybit + Hyperliquid** using **lean multi-exchange spread snapshots** to avoid data explosion.

---

## 1. Objectives

### 1.1 Primary Goals

| ID | Objective | Success Criteria |
|----|-----------|------------------|
| O1 | **Backtest Signal Parity** | Same feature definitions used in Barter and Nautilus backtests |
| O2 | **Real-time Execution in Barter** | Live arb/strategy decisions stay in Barter, no Nautilus round-trip |
| O3 | **Binance Data Capture** | 1m bars + raw trades stored in Nautilus-compatible Parquet |
| O4 | **Cost-Optimized Storage** | Lean snapshots for multi-exchange to avoid data explosion |
| O5 | **Fault Tolerance** | Auto-recovery for missed data, backfill verification |
| O6 | **Scalability** | Scale from Binance → Bybit → Hyperliquid |

### 1.2 Tech Stack Summary

| Layer | Technology | Notes |
|-------|------------|-------|
| **Data ingestion** | Rust + tokio + barter-data | WebSocket/REST feeds |
| **Serialization** | MessagePack (rmp-serde) | UDS/TCP IPC |
| **Storage** | Apache Parquet (arrow/parquet) | 1m bars + trades + spread snapshots |
| **Cloud** | AWS S3 (aws-sdk-rust) | Optional rolling storage |
| **Clock sync** | NTP/chrony | Required for cross-venue arb |
| **Backtesting** | Nautilus Trader (Python + Rust) | External system |

### 1.3 Non-Goals

- Replacing Barter's existing WebSocket JSON feed (remains for TUIs)
- Running Nautilus for live execution (Barter handles execution)
- Routing every tick through Nautilus for live decisions
- Multi-exchange raw tick storage at launch (Binance-only Phase 1)
- Python tooling in the Barter data pipeline (Rust-only for collection/backfill)
- Multi-language schema (Rust-only for now)
- L2 orderbook historical backfill (too expensive, collect going forward only)

---

## 2. Architecture Overview

### 2.1 System Components

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         PRODUCTION ARCHITECTURE                              │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│     BINANCE (PHASE 1)  ───────────►  BARTER-RS (REAL-TIME BRAIN)            │
│     BYBIT + HYPERLIQUID (PHASE 2)   • Ingest WS/REST                        │
│                                     • Arb detection                         │
│                                     • Order execution                       │
│                                     • Risk gates                            │
│                                     • Aggregated snapshots                  │
│                                                                              │
│                      ┌───────────────────┬───────────────────┐               │
│                      │                   │                   │               │
│                      ▼                   ▼                   ▼               │
│         ┌─────────────────────┐  ┌────────────────────┐  ┌────────────────┐ │
│         │  EXECUTE (LIVE)     │  │  STORE (PARQUET)   │  │  STREAM (UDS)   │ │
│         │  Direct to venues   │  │  1m bars + trades  │  │  Signals only   │ │
│         └─────────────────────┘  │  + spread snaps    │  │  (optional)     │ │
│                                   └─────────┬─────────┘  └──────┬─────────┘ │
│                                             │                   │           │
│                                             ▼                   ▼           │
│                                ┌─────────────────────┐  ┌────────────────┐ │
│                                │  Local/S3 Storage   │  │  Nautilus       │ │
│                                │  (Backtest data)    │  │  Backtesting    │ │
│                                └─────────────────────┘  └────────────────┘ │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 2.2 Data Flow Modes

| Mode | Source | Transport | Consumer | Use Case |
|------|--------|-----------|----------|----------|
| **Live Trading** | Exchanges → Barter | Direct | Barter executor | Real-time arb/strategy execution |
| **Sandbox (Optional)** | Barter snapshots/signals | UDS/TCP | Nautilus | Validation, dry-run |
| **Backtesting** | Parquet (local/S3) | File read | Nautilus | Strategy development |
| **Data Collection** | Exchanges → Barter | Parquet write | Local/S3 | Historical storage |

---

## 2.3 Arbitrage Strategy Requirements

| Strategy Type | Minimum Exchanges | Notes |
|---------------|-------------------|-------|
| **2-Exchange Price/Funding Arb** | 2 | Buy cheap on A, sell expensive on B (or funding carry) |
| **Triangular Arb** | 1 exchange with 3+ pairs **or** 3 exchanges | Cycle A→B→C→A |
| **Best-Execution Routing** | 2+ | Not pure arb; chooses best venue |

**Decision:** Barter executes arb **immediately** in real time. Nautilus is used for backtesting and validation on **aggregated snapshots**, not for live routing or tick-level decisions.

## 2.4 Multi-Exchange Data Strategy (Pros/Cons)

| Option | What is Stored | Pros | Cons | When to Use |
|--------|----------------|------|------|-------------|
| **A. Store Everything** | Full tick + L2 for all venues | Maximum fidelity | Expensive, heavy | Research-only |
| **B. Synced Spread Snapshots (Recommended)** | Best bid/ask + funding + spread at fixed interval | 10–30× smaller, sufficient for arb backtests | Loses microstructure | Multi-venue arb |
| **C. Signals-Only** | Signals + executions | Cheapest | Can't re-test thresholds | Live-only audit |

**Cadence decision:** **1s snapshots by default.**  
Pros: low cost, enough for medium‑frequency arb backtests.  
Cons: may miss very short‑lived spreads.  
Optional: **100ms snapshots** for targeted symbols if needed (≈10× data).

---

## 3. Data Specifications

### 3.1 Data Types & Sources

**Phase 1 (Binance-only):** store 1m bars + raw trades.  
**Phase 2 (Multi-exchange):** add Bybit + Hyperliquid; store synced spread snapshots at 1s (or 100ms if needed).

| Data Type | Source | Frequency | Backfill Available | Storage |
|-----------|--------|-----------|-------------------|---------|
| 1m OHLCV Bars | Binance Klines | 1/minute | ✅ 2017+ (FREE) | Parquet |
| Raw Trades | Binance Trades | Per trade | ✅ 2017+ (FREE) | Parquet |
| Delta (per bar) | Calculated | 1/minute | ✅ From trades | In 1m bar |
| CVD | Calculated | 1/minute | ✅ From trades | In 1m bar |
| Open Interest | Binance API | 30 seconds | ⚠️ Limited | In 1m bar |
| Funding Rate | Binance API | 8 hours | ✅ 2019+ (FREE) | Parquet |
| Liquidations (raw) | Binance WS | Per event | ✅ 2020+ (FREE) | Optional (future) |
| Liquidation aggregates | Calculated | 1/minute | ✅ From WS | In extended bar |
| L1 Book (BBO) | Binance WS | Per tick | ❌ Collect forward | In 1m bar |
| L2 depth bands (10/50/100 bps) | Calculated from L2 | 1/minute | ❌ Collect forward | In extended bar |
| L2 Orderbook (raw) | Binance WS | 100ms | ❌ Collect forward | Optional (future) |
| Spread Snapshots (multi-exchange) | Best bid/ask + funding | 1s (default) | ❌ Collect forward | Parquet |

**Extension path (future):**
- If we need lower‑timeframe signals later, we add *new datasets* rather than changing existing schemas:
  - `extended_bars_10s/` or `extended_bars_5s/`
  - `l2_snapshots_1m/` or `l2_snapshots_10s/`
  - `liquidations_raw/` (tick)
- Core Nautilus files stay stable; extended datasets can version with `schema_version` without breaking anything.

### 3.2 Nautilus Compatibility Tradeoff (Decision)

**Constraint:** Nautilus built-in Parquet loaders require exact schemas (types, order, metadata).  
**Decision:** Use **Option 1** (two files). Keep core Nautilus schemas 100% compatible, store extra metrics separately.

| Option | Description | Pros | Cons | Decision |
|--------|-------------|------|------|----------|
| **1. Two files (Recommended)** | Write Nautilus-compatible Trades/Bars + separate extended bars | Zero ingestion risk, fast ship | Two files to manage | ✅ Chosen |
| 2. CustomData pipeline | Keep core bars, load extras as CustomData | Clean in Nautilus | Extra loader/integration work | Later |
| 3. Extend core schema | Add extra columns to bars/trades | Single file | Breaks Nautilus loaders | ❌ No |

**Timestamp policy (sync requirement):**
- Every dataset includes `ts_event` (event time, nanos UTC) and `ts_init` (ingest time).
- For **1m bars and derived metrics**, `ts_event` equals the **bar close time** (minute boundary).
- **Derived open time:** `ts_open = ts_event - bar_interval` for cross-tool alignment (e.g., TradingView uses bar open timestamps). Store `ts_open` in extended bars only.
- Extended metrics use the same `ts_event` so joins are exact.
**Compatibility note:** TradingView bar timestamps use **open time** (`time`), while Nautilus defaults to **close time**. We store `ts_open` in extended bars to map 1:1 when needed.

### 3.3 Core 1m Bar Schema (Nautilus-Compatible)

**Precision mode:**  
- **Standard** = FixedSizeBinary(8), i64 × 1e9  
- **High** = FixedSizeBinary(16), i128 × 1e16  
Controlled by env var `NAUTILUS_PRECISION` (`standard` or `high`, default `high`).
Python wheels on macOS/Linux are typically **high-precision** by default; Windows wheels are **standard**.

```
┌─────────────────────────────────────────────────────────────────┐
│ CORE 1m BAR SCHEMA (Nautilus Compatible)                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│ FIELD       │ TYPE                    │ DESCRIPTION            │
│ ════════════╪═════════════════════════╪════════════════════════│
│ open        │ FixedSizeBinary(PRECISION_BYTES) │ fixed-point    │
│ high        │ FixedSizeBinary(PRECISION_BYTES) │ fixed-point    │
│ low         │ FixedSizeBinary(PRECISION_BYTES) │ fixed-point    │
│ close       │ FixedSizeBinary(PRECISION_BYTES) │ fixed-point    │
│ volume      │ FixedSizeBinary(PRECISION_BYTES) │ fixed-point    │
│ ts_event    │ UInt64                  │ Bar close time (nanos)│
│ ts_init     │ UInt64                  │ Ingest time (nanos)   │
│                                                                 │
│ Required metadata: bar_type, instrument_id, price_precision,    │
│                    size_precision                               │
└─────────────────────────────────────────────────────────────────┘

Notes:
- Depth bands are computed from the **latest L2 snapshot at bar close** using mid price:  
  `mid = (best_bid + best_ask) / 2`  
  `bid_depth_Xbps_base = Σ size for bids with price ≥ mid*(1 - X/10000)`  
  `ask_depth_Xbps_base = Σ size for asks with price ≤ mid*(1 + X/10000)`  
  `bid_depth_Xbps_usd  = Σ size*price for bids within band`  
  `ask_depth_Xbps_usd  = Σ size*price for asks within band`  
  `depth_imb_Xbps = (bid_depth_usd - ask_depth_usd) / (bid_depth_usd + ask_depth_usd)`
- If no L2 snapshot is available for a bar, depth fields are zero.
- Storing both **base** and **USD notional** adds negligible storage (<1MB/day for 3 symbols) and preserves interpretability.
```

### 3.4 Extended 1m Bar Schema (Barter-only)

```
┌─────────────────────────────────────────────────────────────────┐
│ EXTENDED 1m BAR SCHEMA (Barter-only, join on ts_event)          │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│ ts_event            │ i64 (nanos)    │ Bar close time           │
│ ts_init             │ i64 (nanos)    │ Ingest time              │
│ ts_open             │ i64 (nanos)    │ Derived bar open time    │
│ instrument_id       │ string         │ "BTCUSDT.BINANCE"        │
│                                                                 │
│ # OHLCV (optional mirror of core)                               │
│ open/high/low/close │ i64 (fixed-pt) │ × 1e9                    │
│ volume              │ i64 (fixed-pt) │ × 1e9                    │
│ quote_volume        │ i64 (fixed-pt) │ × 1e9                    │
│ trade_count         │ i64            │ Trades count             │
│                                                                 │
│ # Volume Delta                                           │
│ buy_volume          │ i64 (fixed-pt) │ × 1e9                    │
│ sell_volume         │ i64 (fixed-pt) │ × 1e9                    │
│ delta               │ i64 (fixed-pt) │ buy - sell               │
│ cvd                 │ i64 (fixed-pt) │ Cumulative delta         │
│                                                                 │
│ # Derivatives / L1                                           │
│ open_interest       │ i64 (fixed-pt) │ OI at bar close          │
│ oi_change           │ i64 (fixed-pt) │ OI change                │
│ funding_rate        │ f64            │ Current funding          │
│ bid_price/bid_size  │ i64 (fixed-pt) │ L1 bid                   │
│ ask_price/ask_size  │ i64 (fixed-pt) │ L1 ask                   │
│ spread_bps          │ f64            │ Spread in bps            │
│ book_imbalance      │ f64            │ (bid-ask)/(bid+ask)      │
│                                                                 │
│ # Liquidations (1m aggregates, quote notional)                  │
│ liq_buy_usd        │ i64 (fixed-pt) │ Buy-side liqs in USD     │
│ liq_sell_usd       │ i64 (fixed-pt) │ Sell-side liqs in USD    │
│ liq_total_usd      │ i64 (fixed-pt) │ Total liqs in USD        │
│ liq_count          │ u64            │ Liquidation count        │
│                                                                 │
│ # L2 depth bands (1m snapshot, base + notional within bps of mid)│
│ bid_depth_10bps_base │ i64 (fixed-pt) │ Bid depth (base)       │
│ ask_depth_10bps_base │ i64 (fixed-pt) │ Ask depth (base)       │
│ bid_depth_10bps_usd  │ i64 (fixed-pt) │ Bid depth (USD)        │
│ ask_depth_10bps_usd  │ i64 (fixed-pt) │ Ask depth (USD)        │
│ depth_imb_10bps      │ f64            │ USD imbalance          │
│ bid_depth_50bps_base │ i64 (fixed-pt) │ Bid depth (base)       │
│ ask_depth_50bps_base │ i64 (fixed-pt) │ Ask depth (base)       │
│ bid_depth_50bps_usd  │ i64 (fixed-pt) │ Bid depth (USD)        │
│ ask_depth_50bps_usd  │ i64 (fixed-pt) │ Ask depth (USD)        │
│ depth_imb_50bps      │ f64            │ USD imbalance          │
│ bid_depth_100bps_base│ i64 (fixed-pt) │ Bid depth (base)       │
│ ask_depth_100bps_base│ i64 (fixed-pt) │ Ask depth (base)       │
│ bid_depth_100bps_usd │ i64 (fixed-pt) │ Bid depth (USD)        │
│ ask_depth_100bps_usd │ i64 (fixed-pt) │ Ask depth (USD)        │
│ depth_imb_100bps     │ f64            │ USD imbalance          │
└─────────────────────────────────────────────────────────────────┘
```

### 3.5 Raw Trades Schema (Parquet)

**Precision mode:**  
- **Standard** = FixedSizeBinary(8), i64 × 1e9  
- **High** = FixedSizeBinary(16), i128 × 1e16  
Controlled by env var `NAUTILUS_PRECISION` (`standard` or `high`, default `high`).

```
┌─────────────────────────────────────────────────────────────────┐
│ RAW TRADES SCHEMA (Nautilus TradeTick Compatible)               │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│ FIELD          │ TYPE                │ DESCRIPTION             │
│ ═══════════════╪═════════════════════╪═════════════════════════│
│ price          │ FixedSizeBinary(PRECISION_BYTES) │ fixed-point │
│ size           │ FixedSizeBinary(PRECISION_BYTES) │ fixed-point │
│ aggressor_side │ UInt8               │ 0=No,1=Buy,2=Sell       │
│ trade_id       │ Utf8                │ Exchange trade ID       │
│ ts_event       │ UInt64              │ Exchange timestamp      │
│ ts_init        │ UInt64              │ Ingest timestamp        │
│                                                                 │
│ Required metadata: instrument_id, price_precision, size_precision│
└─────────────────────────────────────────────────────────────────┘
```

### 3.6 Spread Snapshot Schema (Multi-Exchange Arb)

```
┌─────────────────────────────────────────────────────────────────┐
│ SPREAD SNAPSHOT SCHEMA (ARB BACKTEST)                           │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│ ts_event               │ i64 (nanos)    │ Snapshot time         │
│ ts_init                │ i64 (nanos)    │ Ingest time           │
│ symbol                 │ string         │ "BTCUSDT"             │
│                                                                 │
│ # Exchange BBOs        │                │                      │
│ binance_bid            │ i64 (fixed-pt) │ bid × 1e9             │
│ binance_ask            │ i64 (fixed-pt) │ ask × 1e9             │
│ bybit_bid              │ i64 (fixed-pt) │ bid × 1e9             │
│ bybit_ask              │ i64 (fixed-pt) │ ask × 1e9             │
│ hyper_bid              │ i64 (fixed-pt) │ bid × 1e9             │
│ hyper_ask              │ i64 (fixed-pt) │ ask × 1e9             │
│                                                                 │
│ # Spreads (precalc)    │                │                      │
│ spread_binance_bybit   │ i64 (fixed-pt) │ bidA - askB           │
│ spread_binance_hyper   │ i64 (fixed-pt) │ bidA - askC           │
│ spread_bybit_hyper     │ i64 (fixed-pt) │ bidB - askC           │
│                                                                 │
│ # Funding (latest)     │                │                      │
│ funding_binance        │ f64            │ current funding       │
│ funding_bybit          │ f64            │ current funding       │
│ funding_hyper          │ f64            │ current funding       │
└─────────────────────────────────────────────────────────────────┘
```

---

## 4. Storage Architecture

### 4.1 Hybrid Storage Strategy

**Clarification (as of 2026-01-29):**
- **Historical backfill (years ≤2025)** is generated **locally** on macOS/Windows.
- **Live data from 2026-01-01 onward** is written **on AWS (collector)** to Parquet, then synced to local storage.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         STORAGE TOPOLOGY                                     │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│   AWS S3 (Hot Storage - Rolling 30 Days)                                    │
│   ══════════════════════════════════════                                    │
│   s3://trading-data/                                                        │
│   ├── bars_1m/symbol=BTCUSDT/date=2026-01-29.parquet                       │
│   ├── trades/symbol=BTCUSDT/date=2026-01-29/hour=14.parquet                │
│   └── spread_snapshots/symbol=BTCUSDT/date=2026-01-29.parquet (Phase 2)    │
│   └── ... (auto-deleted after 30 days via lifecycle rule)                  │
│                                                                              │
│   Cost: ~$0.02/month (30 days × 3 assets)                                  │
│                                                                              │
│   ─────────────────────────────────────────────────────────────────────────│
│                                                                              │
│   LOCAL STORAGE (Cold Storage - Permanent)                                  │
│   ════════════════════════════════════════                                  │
│   C:\trading-data\          (Windows - Nautilus machine)                   │
│   ├── historical\           (Backfill 2024-2025, static)                   │
│   │   ├── bars_1m\                                                          │
│   │   │   ├── BTCUSDT-2024.parquet                                         │
│   │   │   ├── BTCUSDT-2025.parquet                                         │
│   │   │   └── ...                                                           │
│   │   └── trades\                                                           │
│   │       └── ...                                                           │
│   │   └── spread_snapshots\  (Phase 2)                                      │
│   │       └── ...                                                           │
│   │                                                                          │
│   └── recent\               (Synced from S3, grows)                        │
│       ├── bars_1m\                                                          │
│       ├── trades\                                                           │
│       └── spread_snapshots\  (Phase 2)                                      │
│                                                                              │
│   Cost: $0 (uses existing disk)                                            │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 4.2 Storage Estimates (3 Assets, 3 Years)

| Data Type | Per Year | 3 Years | Location |
|-----------|----------|---------|----------|
| 1m Unified Bars | ~200 MB | ~600 MB | Local |
| Raw Trades | ~12 GB | ~36 GB | Local |
| Spread Snapshots (1s, 3 venues) | ~18 GB | ~54 GB | Local/S3 (Phase 2) |
| Spread Snapshots (100ms, 3 venues) | ~180 GB | ~540 GB | Local/S3 (Phase 2, optional) |
| **TOTAL (Phase 1)** | **~12 GB** | **~37 GB** | Local |
| **TOTAL (Phase 2)** | **~30 GB** | **~91 GB** | Local |
| S3 (rolling 30 days) | ~1–3 GB | N/A | S3 |

**Total Cost (Phase 1):** ~$0.25/year (S3) + $0 (local) = **~$0.25/year**  
**Total Cost (Phase 2):** depends on snapshot rate; 1s is low, 100ms is ~10×.

**Operational Split:**
- **Backfill (≤2025):** run locally, write to `historical/` then optional upload.
- **Live (≥2026-01-01):** written on AWS collector, synced to local `recent/`.

---

## 5. Fault Tolerance & Data Integrity

### 5.1 Backfill & Recovery Strategy

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         FAULT TOLERANCE SYSTEM                               │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│   5.1.1 COLLECTOR HEALTH MONITORING                                         │
│   ═══════════════════════════════════                                       │
│                                                                              │
│   • Heartbeat file: /tmp/collector-heartbeat.json                          │
│     {                                                                        │
│       "last_write": "2026-01-29T14:30:00Z",                                │
│       "symbols": ["BTCUSDT", "ETHUSDT", "SOLUSDT"],                        │
│       "last_bars": { "BTCUSDT": "2026-01-29T14:29:00Z", ... }              │
│     }                                                                        │
│                                                                              │
│   • Health check script (cron every 5 min):                                │
│     - If last_write > 5 min ago → alert + restart collector                │
│     - If gap detected → trigger backfill                                    │
│                                                                              │
│   ─────────────────────────────────────────────────────────────────────────│
│                                                                              │
│   5.1.2 GAP DETECTION & BACKFILL                                           │
│   ══════════════════════════════                                           │
│                                                                              │
│   On startup and hourly:                                                    │
│   1. Scan Parquet files for each symbol                                    │
│   2. Build timeline of existing bars                                        │
│   3. Identify gaps > 1 minute                                               │
│   4. For each gap:                                                          │
│      a. If < 24 hours: Fetch from Binance REST API                         │
│      b. If > 24 hours: Fetch from Binance Public Data (bulk)               │
│   5. Write backfilled data to Parquet                                       │
│   6. Log backfill report                                                    │
│                                                                              │
│   ─────────────────────────────────────────────────────────────────────────│
│                                                                              │
│   5.1.3 DATA VERIFICATION                                                   │
│   ═══════════════════════════                                               │
│                                                                              │
│   Daily verification job:                                                   │
│   • Count bars per day (expect 1440 for 1m)                                │
│   • Verify no duplicate timestamps                                          │
│   • Check for NULL/invalid values                                           │
│   • Compare with Binance candle count via API                              │
│   • Report discrepancies                                                    │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 5.2 Sync Verification (S3 → Local)

```bash
# sync-and-verify.sh (run weekly on Windows machine)

#!/bin/bash
set -e

# 1. Sync from S3
aws s3 sync s3://trading-data/bars_1m/ C:/trading-data/recent/bars_1m/
aws s3 sync s3://trading-data/trades/ C:/trading-data/recent/trades/

# 2. Verify file integrity
./target/release/verify_parquet C:/trading-data/recent/

# 3. Check for gaps
./target/release/check_gaps C:/trading-data/ --start 2024-01-01 --end today

# 4. Report
echo "Sync complete. $(find C:/trading-data/recent -name '*.parquet' | wc -l) files."
```

---

## 6. Dependencies

### 6.1 Software Dependencies

| Component | Dependency | Version | Purpose |
|-----------|------------|---------|---------|
| **Barter-RS** | Rust | 1.75+ | Core language |
| | tokio | 1.42+ | Async runtime |
| | rmp-serde | latest | MessagePack encoding |
| | arrow/parquet | 53+ | Parquet writing |
| | chrono | 0.4+ | Time handling |
| **Nautilus** | Python | 3.12+ | Runtime |
| | Rust | 1.92+ | Core crates |
| | nautilus-barter adapter | latest | Barter integration |
| | pyarrow | 14+ | Parquet reading |
| **Infrastructure** | AWS CLI | latest | S3 operations |
| | rclone (optional) | latest | Cross-platform sync |
| | NTP/chrony | latest | Clock sync for cross-venue arb |

**Note:** Barter data pipeline and backfill tooling are **Rust-only**; no Python runtime required on the collector.

### 6.2 Repository Dependencies

```
barter-rs (this repo)
├── barter-data           # WebSocket streaming
├── barter-data-server    # UDS/TCP server + Parquet writer (NEW/MODIFY)
├── barter-execution      # Order execution + risk gates (arb)
├── barter-integration    # Protocol utilities
└── barter-instrument     # Exchange definitions
├── barter-tools          # Rust CLI tools (backfill/verify)

nautilus_trader (external)
├── crates/adapters/barter/   # Barter adapter (EXISTS)
├── crates/persistence/       # Parquet catalog
└── crates/data/              # Data engine
```

---

## 7. Assumptions

| ID | Assumption | Risk if Invalid |
|----|------------|-----------------|
| A1 | Binance WebSocket remains stable | Need fallback exchange |
| A2 | Binance Public Data continues free access | Need paid provider |
| A3 | 3 assets sufficient for initial testing | Scale collector |
| A4 | Windows machine has 50GB+ free disk | Reduce history |
| A5 | AWS Free Tier available for 12 months | Switch to R2/local |
| A6 | UDS latency <1ms is acceptable | Use shared memory |
| A7 | Nautilus Barter adapter is stable | Fix/contribute upstream |
| A8 | Bybit + Hyperliquid public data available in Phase 2 | Adjust venue list |
| A9 | Exchange execution keys available for Barter | Run paper/sandbox first |

---

## 8. Pre-requisites

### 8.1 Before Phase 1

- [ ] AWS account with S3 access configured
- [ ] AWS CLI installed and configured on collector VPS
- [ ] Rust toolchain installed (1.75+) on development machine
- [ ] Nautilus Trader installed on Windows backtesting machine
- [ ] Network connectivity: VPS ↔ S3, Windows ↔ S3
- [ ] Binance API key (optional, higher rate limits)

### 8.2 Before Phase 2

- [ ] Historical backfill downloaded (2024-2025)
- [ ] Parquet schema validated with Nautilus
- [ ] UDS smoke test passing
- [ ] Local backfill hardware (macOS/Windows) has sufficient disk

### 8.3 Before Phase 3

- [ ] At least 1 week of live data collected
- [ ] Backfill verification passing
- [ ] Strategy code ready for testing

### 8.4 Before Phase 4

- [ ] Bybit + Hyperliquid API access approved

---

## 9. Implementation Phases

### Phase 1: Binance-Only Data Pipeline (Week 1-2)

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ PHASE 1: BINANCE-ONLY DATA PIPELINE                                          │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│ TASK 1.1: Parquet Writer Module                                             │
│ ══════════════════════════════                                              │
│ Files:                                                                       │
│   • barter-data-server/src/parquet/mod.rs (NEW)                            │
│   • barter-data-server/src/parquet/schema.rs (NEW)                         │
│   • barter-data-server/src/parquet/writer.rs (NEW)                         │
│   • barter-data-server/Cargo.toml (ADD arrow, parquet deps)                │
│                                                                              │
│ Deliverables:                                                               │
│   • ParquetBarWriter - writes unified 1m bars (Binance)                     │
│   • ParquetTradeWriter - writes raw trades (Binance)                        │
│   • Schema definitions matching Nautilus format                            │
│   • Buffered writing with configurable flush interval                      │
│                                                                              │
│ Dependencies: None                                                          │
│ Estimate: 3-4 days                                                          │
│                                                                              │
│ ───────────────────────────────────────────────────────────────────────────│
│                                                                              │
│ TASK 1.2: 1-Minute Aggregator                                               │
│ ═════════════════════════════                                               │
│ Files:                                                                       │
│   • barter-data-server/src/aggregator/mod.rs (NEW)                         │
│   • barter-data-server/src/aggregator/minute_bar.rs (NEW)                  │
│   • barter-data-server/src/aggregator/delta.rs (NEW)                       │
│                                                                              │
│ Deliverables:                                                               │
│   • MinuteBarAggregator - builds 1m bars from trades (Binance)              │
│   • Delta/CVD calculator - tracks buy/sell volume                          │
│   • L1 snapshot capture at bar close                                        │
│   • OI/Funding rate integration                                             │
│                                                                              │
│ Dependencies: Task 1.1                                                      │
│ Estimate: 2-3 days                                                          │
│                                                                              │
│ ───────────────────────────────────────────────────────────────────────────│
│                                                                              │
│ TASK 1.3: S3 Upload Integration                                             │
│ ═══════════════════════════════                                             │
│ Files:                                                                       │
│   • barter-data-server/src/storage/mod.rs (NEW)                            │
│   • barter-data-server/src/storage/s3.rs (NEW)                             │
│   • barter-data-server/src/storage/local.rs (NEW)                          │
│                                                                              │
│ Deliverables:                                                               │
│   • S3 upload with retry logic                                              │
│   • Local fallback if S3 unavailable                                        │
│   • Configurable via env vars                                               │
│                                                                              │
│ Dependencies: Task 1.1                                                      │
│ Estimate: 2 days                                                            │
│                                                                              │
│ ───────────────────────────────────────────────────────────────────────────│
│                                                                              │
│ TASK 1.4: Health Monitoring                                                 │
│ ═══════════════════════════                                                 │
│ Files:                                                                       │
│   • barter-data-server/src/health/mod.rs (NEW)                             │
│   • barter-data-server/src/health/heartbeat.rs (NEW)                       │
│   • scripts/health-check.sh (NEW)                                           │
│                                                                              │
│ Deliverables:                                                               │
│   • Heartbeat file writer                                                   │
│   • Health check script for cron                                            │
│   • Alert integration (optional: webhook)                                   │
│                                                                              │
│ Dependencies: None                                                          │
│ Estimate: 1 day                                                             │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Phase 2: Historical Backfill (Optional, Week 2-3)

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ PHASE 2: HISTORICAL BACKFILL                                                 │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│ TASK 2.1: Binance Public Data Downloader                                    │
│ ════════════════════════════════════════                                    │
│ Files:                                                                       │
│   • barter-tools/src/bin/binance_downloader.rs (NEW)                       │
│   • config/backfill.yaml (NEW)                                              │
│                                                                              │
│ Deliverables:                                                               │
│   • Download klines, trades, funding from data.binance.vision              │
│   • Resume capability (skip existing files)                                 │
│   • Parallel downloads for speed                                            │
│   • Runs locally (macOS/Windows) for years ≤2025                            │
│                                                                              │
│ Dependencies: None                                                          │
│ Estimate: 2 days                                                            │
│                                                                              │
│ ───────────────────────────────────────────────────────────────────────────│
│                                                                              │
│ TASK 2.2: CSV to Parquet Converter                                          │
│ ══════════════════════════════════                                          │
│ Files:                                                                       │
│   • barter-tools/src/bin/csv_to_parquet.rs (NEW)                           │
│   • barter-tools/src/bin/calculate_delta.rs (NEW)                          │
│                                                                              │
│ Deliverables:                                                               │
│   • Convert Binance CSV to Nautilus Parquet schema                         │
│   • Calculate Delta/CVD from trade data                                     │
│   • Merge into unified 1m bars                                              │
│                                                                              │
│ Dependencies: Task 2.1                                                      │
│ Estimate: 2-3 days                                                          │
│                                                                              │
│ ───────────────────────────────────────────────────────────────────────────│
│                                                                              │
│ TASK 2.3: Gap Detection & Auto-Backfill                                     │
│ ═══════════════════════════════════════                                     │
│ Files:                                                                       │
│   • barter-data-server/src/backfill/mod.rs (NEW)                           │
│   • barter-data-server/src/backfill/gap_detector.rs (NEW)                  │
│   • barter-data-server/src/backfill/fetcher.rs (NEW)                       │
│                                                                              │
│ Deliverables:                                                               │
│   • Scan Parquet files for gaps                                             │
│   • Fetch missing data from Binance API                                     │
│   • Auto-run on startup and hourly                                          │
│                                                                              │
│ Dependencies: Task 1.1, 2.2                                                 │
│ Estimate: 3 days                                                            │
│                                                                              │
│ ───────────────────────────────────────────────────────────────────────────│
│                                                                              │
│ TASK 2.4: Data Verification Suite                                           │
│ ═════════════════════════════════                                           │
│ Files:                                                                       │
│   • barter-tools/src/bin/verify_parquet.rs (NEW)                           │
│   • barter-tools/src/bin/check_gaps.rs (NEW)                                │
│   • barter-tools/src/bin/compare_binance.rs (NEW)                          │
│                                                                              │
│ Deliverables:                                                               │
│   • Verify bar counts (1440/day)                                            │
│   • Check for duplicates/nulls                                              │
│   • Compare with Binance API counts                                         │
│                                                                              │
│ Dependencies: Task 2.2                                                      │
│ Estimate: 1-2 days                                                          │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Phase 3: Nautilus Backtesting Integration (Week 3-4)

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ PHASE 3: NAUTILUS BACKTESTING                                                │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│ TASK 3.1: Verify Barter Adapter                                             │
│ ═══════════════════════════════                                             │
│ Files (nautilus_trader repo):                                               │
│   • crates/adapters/barter/src/lib.rs (REVIEW)                             │
│   • crates/adapters/barter/src/decoder.rs (REVIEW)                         │
│                                                                              │
│ Deliverables:                                                               │
│   • Verify adapter decodes our MessagePack format                          │
│   • Run smoke test with snapshot/signal stream (optional)                  │
│   • Document any schema mismatches                                          │
│                                                                              │
│ Dependencies: Phase 1 complete                                              │
│ Estimate: 1-2 days                                                          │
│                                                                              │
│ ───────────────────────────────────────────────────────────────────────────│
│                                                                              │
│ TASK 3.2: Parquet Catalog Setup                                             │
│ ═══════════════════════════════                                             │
│ Files:                                                                       │
│   • config/nautilus_catalog.yaml (NEW)                                      │
│   • docs/NAUTILUS_SETUP.md (NEW)                                            │
│                                                                              │
│ Deliverables:                                                               │
│   • Configure ParquetDataCatalog for hybrid storage                        │
│   • Test loading historical + recent data                                   │
│   • Verify date range queries work                                          │
│                                                                              │
│ Dependencies: Phase 2 complete                                              │
│ Estimate: 1 day                                                             │
│                                                                              │
│ ───────────────────────────────────────────────────────────────────────────│
│                                                                              │
│ TASK 3.3: End-to-End Test                                                   │
│ ═════════════════════════                                                   │
│ Files:                                                                       │
│   • nautilus_trader/tests/integration/test_barter_adapter.py (REVIEW)      │
│   • nautilus_trader/tests/integration/test_backtest_parity.py (REVIEW)     │
│                                                                              │
│ Deliverables:                                                               │
│   • Aggregated snapshots/signals flow to Nautilus (optional)                │
│   • Backtest uses same feature schema as Barter                             │
│   • Performance metrics match between modes                                 │
│                                                                              │
│ Dependencies: Task 3.1, 3.2                                                 │
│ Estimate: 2 days                                                            │
│                                                                              │
│ ───────────────────────────────────────────────────────────────────────────│
│                                                                              │
│ TASK 3.4: Sync Automation                                                   │
│ ═════════════════════════                                                   │
│ Files:                                                                       │
│   • scripts/sync/sync_s3_to_local.sh (NEW)                                 │
│   • scripts/sync/sync_s3_to_local.ps1 (NEW, Windows)                       │
│   • config/sync_schedule.yaml (NEW)                                         │
│                                                                              │
│ Deliverables:                                                               │
│   • Automated weekly sync from S3 to local                                  │
│   • Verification after sync                                                 │
│   • Cleanup old S3 data after confirmed sync                                │
│                                                                              │
│ Dependencies: Task 3.2                                                      │
│ Estimate: 1 day                                                             │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Phase 4: Multi-Exchange Expansion + Production (Week 4-6)

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ PHASE 4: MULTI-EXCHANGE + PRODUCTION                                         │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│ TASK 4.1: Add Bybit + Hyperliquid Feeds                                      │
│ ══════════════════════════════════════                                      │
│ Deliverables:                                                               │
│   • Connectors for trades + BBO + funding + OI + liquidations               │
│   • Unified instrument IDs (BTCUSDT) across venues                          │
│                                                                              │
│ Dependencies: Phase 1 complete                                              │
│ Estimate: 3-4 days                                                          │
│                                                                              │
│ ───────────────────────────────────────────────────────────────────────────│
│                                                                              │
│ TASK 4.2: Spread Snapshot Writer (1s default)                                │
│ ════════════════════════════════                                             │
│ Deliverables:                                                               │
│   • 1s spread snapshots per symbol (default)                                │
│   • Optional 100ms for targeted symbols                                     │
│   • Funding + spread pre-calcs                                              │
│   • Parquet schema for arb backtests                                        │
│                                                                              │
│ Dependencies: Task 4.1                                                      │
│ Estimate: 2-3 days                                                          │
│                                                                              │
│ ───────────────────────────────────────────────────────────────────────────│
│                                                                              │
│ TASK 4.3: Barter Arb Detector + Execution                                    │
│ ════════════════════════════════                                             │
│ Deliverables:                                                               │
│   • Price/funding spread thresholds                                         │
│   • Risk gates + circuit breakers                                           │
│   • Execution routing to venue pairs                                        │
│                                                                              │
│ Dependencies: Task 4.1                                                      │
│ Estimate: 3-5 days                                                          │
│                                                                              │
│ ───────────────────────────────────────────────────────────────────────────│
│                                                                              │
│ TASK 4.4: AWS Infrastructure + Monitoring                                    │
│ ════════════════════════════════                                             │
│ Deliverables:                                                               │
│   • EC2 + systemd deployment                                                │
│   • S3 lifecycle rules                                                      │
│   • CloudWatch alarms + disk monitoring                                     │
│                                                                              │
│ Dependencies: Phase 1 complete                                              │
│ Estimate: 1-2 days                                                          │
│                                                                              │
│ ───────────────────────────────────────────────────────────────────────────│
│                                                                              │
│ TASK 4.5: Documentation                                                     │
│ ═══════════════════════                                                     │
│ Deliverables:                                                               │
│   • Deployment guide + troubleshooting                                      │
│   • Schema documentation                                                    │
│                                                                              │
│ Dependencies: All phases                                                    │
│ Estimate: 1-2 days                                                          │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 10. File Change Summary

### 10.1 Barter-RS Repository

| File | Action | Phase | Description |
|------|--------|-------|-------------|
| `barter-data-server/Cargo.toml` | MODIFY | 1 | Add arrow, parquet, aws-sdk deps |
| `barter-data-server/src/main.rs` | MODIFY | 1 | Add Parquet writer initialization |
| `barter-data-server/src/parquet/mod.rs` | NEW | 1 | Parquet module entry |
| `barter-data-server/src/parquet/schema.rs` | NEW | 1 | Nautilus-compatible schemas |
| `barter-data-server/src/parquet/writer.rs` | NEW | 1 | Buffered Parquet writer |
| `barter-data-server/src/aggregator/mod.rs` | NEW | 1 | Aggregation module entry |
| `barter-data-server/src/aggregator/minute_bar.rs` | NEW | 1 | 1m bar aggregator |
| `barter-data-server/src/aggregator/delta.rs` | NEW | 1 | Delta/CVD calculator |
| `barter-data-server/src/storage/mod.rs` | NEW | 1 | Storage abstraction |
| `barter-data-server/src/storage/s3.rs` | NEW | 1 | S3 upload logic |
| `barter-data-server/src/storage/local.rs` | NEW | 1 | Local storage fallback |
| `barter-data-server/src/health/mod.rs` | NEW | 1 | Health monitoring |
| `barter-data-server/src/health/heartbeat.rs` | NEW | 1 | Heartbeat writer |
| `barter-data-server/src/backfill/mod.rs` | NEW | 2 | Backfill module |
| `barter-data-server/src/backfill/gap_detector.rs` | NEW | 2 | Gap detection |
| `barter-data-server/src/backfill/fetcher.rs` | NEW | 2 | Binance API fetcher |
| `barter-tools/src/bin/binance_downloader.rs` | NEW | 2 | Bulk downloader (Rust) |
| `config/backfill.yaml` | NEW | 2 | Backfill configuration |
| `barter-tools/src/bin/csv_to_parquet.rs` | NEW | 2 | CSV converter (Rust) |
| `barter-tools/src/bin/calculate_delta.rs` | NEW | 2 | Delta calculator (Rust) |
| `barter-tools/src/bin/verify_parquet.rs` | NEW | 2 | Verification suite (Rust) |
| `barter-tools/src/bin/check_gaps.rs` | NEW | 2 | Gap checker (Rust) |
| `barter-tools/src/bin/compare_binance.rs` | NEW | 2 | Binance parity check (Rust) |
| `scripts/health-check.sh` | NEW | 1 | Cron health check |
| `scripts/sync/sync_s3_to_local.sh` | NEW | 3 | S3 sync script |
| `deploy/aws/collector.service` | NEW | 4 | Systemd service |
| `deploy/aws/setup.sh` | NEW | 4 | AWS setup script |
| `deploy/aws/s3-lifecycle.json` | NEW | 4 | S3 lifecycle rules |
| `barter-data-server/src/spread_snapshots/` | NEW | 4 | Spread snapshot writer (arb) |
| `barter-data/src/exchange/hyperliquid/` | NEW | 4 | Hyperliquid connector |
| `docs/NAUTILUS_SETUP.md` | NEW | 3 | Nautilus setup guide |
| `docs/PRD_BARTER_NAUTILUS_INTEGRATION.md` | NEW | 0 | This document |
| `docs/DEPLOYMENT_GUIDE.md` | NEW | 4 | Deployment guide |
| `docs/DATA_SCHEMA.md` | NEW | 4 | Schema docs |

### 10.2 Nautilus Trader Repository (External)

| File | Action | Phase | Description |
|------|--------|-------|-------------|
| `crates/adapters/barter/` | REVIEW | 3 | Verify compatibility |
| N/A | N/A | N/A | Adapter already exists, minimal changes expected |

---

## 11. Risk Register

| ID | Risk | Probability | Impact | Mitigation |
|----|------|-------------|--------|------------|
| R1 | Binance API rate limiting | Medium | High | Use Public Data bulk download, implement backoff |
| R2 | S3 costs exceed budget | Low | Low | Use R2 or local-only mode |
| R3 | Schema mismatch with Nautilus | Medium | High | Test early, document schema clearly |
| R4 | Collector crashes lose data | Medium | Medium | Heartbeat + auto-restart + backfill |
| R5 | Network latency affects live trading | Low | High | UDS for same-host, TCP for remote |
| R6 | Disk space exhaustion | Low | Medium | Monitor + alerts + lifecycle rules |
| R7 | Multi-exchange data explosion | Medium | Medium | Use spread snapshots, not raw ticks |

---

## 12. Validation & Success Criteria

### 12.1 Critical Validation Points (Integration Tests)

| Test | Phase | Procedure | Pass Criteria |
|------|-------|-----------|---------------|
| Parquet schema compatibility | 1–3 | Load bars/trades via Nautilus ParquetDataCatalog | No schema errors; data query succeeds |
| 1m bar parity vs Binance klines | 1 | Compare OHLCV vs Binance klines for sample window | OHLC within 1 tick; volume within 0.1% |
| Raw trade completeness | 1 | Compare trade count vs Binance API sample window | ≥99.5% match |
| Gap detection + dedupe | 1–2 | Run verify scripts on stored data | No missing 1m bars; duplicates <0.01% |
| OI/Funding alignment | 1 | Verify latest OI/funding carried into 1m bars | Freshness ≤ 2× poll interval |
| Liquidation aggregation | 1 | Sum liquidation events per minute vs extended bars | Values match within 0.1% |
| L2 depth bands | 1 | Compare bands vs in-memory L2 snapshot at bar close | Bands computed; non-zero when book present |
| Spread snapshot correctness | 4 | Validate BBO + spread calculations | Spread sign correct; BBO freshness ≤ 2s |
| Normalization sanity | 4 | Unit test contract-size conversions | OKX/Bybit USD notional within expected bounds |
| Reconnect resilience | 1–4 | Force disconnect, observe recovery | Streams resume <30s; no bar duplication |
| Sandbox decode (optional) | 3 | UDS snapshot/signal -> Nautilus adapter | Decode success; fields populated |

### 12.2 Success Metrics

| Metric | Target | Measurement |
|--------|--------|-------------|
| Data completeness | >99.9% | Bars per day vs expected 1440 |
| 1m bar parity | ≥99.9% | OHLCV matches Binance klines within tolerance |
| Raw trade completeness | ≥99.5% | Trade count vs Binance API |
| Spread snapshot completeness | ≥99% | Expected snapshots per day (Phase 4) |
| UDS latency (optional) | <1ms p99 | Measure in smoke test |
| Backfill recovery time | <1 hour for 24h gap | Time to detect + fetch + write |
| Storage cost | <$1/year (Phase 1) | AWS billing |
| Backtest-signal parity | 100% | Same feature outputs for same data |

### 12.3 Phase Exit Criteria

**Phase 1 (Binance-only):**
- 7 consecutive days of bars + trades with >99.9% completeness
- 1m parity test vs Binance klines passes
- Parquet schema loads in Nautilus

**Phase 2 (Backfill optional):**
- Backfill gap detection clean (no >1m gaps)
- Dedupe and verification scripts pass

**Phase 3 (Nautilus backtests):**
- Nautilus backtest runs with Parquet bars/trades
- Feature outputs match Barter snapshots for sample window

**Phase 4 (Multi-exchange):**
- Spread snapshots produced at 1s cadence
- Arb detector + risk gates in Barter validated in sandbox

---

## 13. Timeline Summary

| Phase | Duration | Start | End | Milestones |
|-------|----------|-------|-----|------------|
| Phase 1 | 2 weeks | Week 1 | Week 2 | Binance 1m bars + raw trades |
| Phase 2 | 1.5 weeks | Week 2 | Week 3 | Historical backfill (optional) |
| Phase 3 | 1 week | Week 3 | Week 4 | Nautilus backtests running |
| Phase 4 | 2 weeks | Week 4 | Week 6 | Bybit + Hyperliquid + spread snapshots |
| **TOTAL** | **6 weeks** | | | Multi-venue system operational |

---

## 14. Approval

### 14.1 Execution Plan

**Approved Plan:** Option B (Fast-track)
- **Phase 1:** Binance data pipeline (Parquet writer + aggregator + S3)
- **Phase 3:** Nautilus validation (starts after 7 days live data)
- **Phase 2:** Historical backfill (deferred, optional enhancement)

**Rationale:** Gets Phase 3 validation ASAP with real live data, without waiting for historical backfill.

### 14.2 Gating Evidence (Required Before Phase 1 Execution)

| Evidence | Description | Pass Criteria |
|----------|-------------|---------------|
| **Schema compatibility proof** | Produce sample Parquet file (bars + trades), load in Nautilus | Zero schema errors in `ParquetDataCatalog` |
| **Timestamp convention proof** | Demonstrate ts_event = bar close time, ts_open = ts_event − 60s in extended bars | Timestamps align correctly |

### 14.3 Corrections to Plan

| Item | Correction |
|------|------------|
| Task 3.3 test script | `scripts/validation/test_nautilus_load.py` lives in **barter-rs repo** (local validation script), not in Nautilus |

### 14.4 Sign-off

| Role | Name | Date | Status |
|------|------|------|--------|
| Architect | Codex | 2026-01-29 | ✅ Approved (Option B, with gating evidence) |
| Developer | Opus | 2026-01-29 | Pending gating evidence |
| Reviewer | | | |

---

## Appendix A: Environment Variables

```bash
# Collector Configuration
BINANCE_WS_URL=wss://fstream.binance.com/ws
SYMBOLS=BTCUSDT,ETHUSDT,SOLUSDT
EXCHANGES=binance              # Phase 1: binance only
PARQUET_OUTPUT_DIR=/data/parquet
PARQUET_FLUSH_INTERVAL_SECS=60
NAUTILUS_PRECISION=high         # high (16-byte) or standard (8-byte)
SPREAD_SNAPSHOT_INTERVAL_MS=1000  # Phase 2: multi-exchange snapshots (default)
LIVE_START_DATE=2026-01-01        # Live Parquet on AWS from this date

# S3 Configuration
S3_ENABLED=true
S3_BUCKET=trading-data
S3_REGION=us-east-1
AWS_ACCESS_KEY_ID=xxx
AWS_SECRET_ACCESS_KEY=xxx

# UDS Configuration (existing)
UDS_ENABLED=true
UDS_PATH=/tmp/barter-data.sock

# Health Monitoring
HEARTBEAT_FILE=/tmp/collector-heartbeat.json
HEARTBEAT_INTERVAL_SECS=30

# Backfill Configuration
BACKFILL_ON_STARTUP=true
BACKFILL_CHECK_INTERVAL_MINS=60
BINANCE_API_KEY=xxx (optional, for higher rate limits)
```

---

## Appendix B: Nautilus Catalog Configuration

```python
# nautilus_config.py

from nautilus_trader.persistence.catalog import ParquetDataCatalog

# Single catalog pointing to root directory
# Nautilus automatically discovers all Parquet files
catalog = ParquetDataCatalog("C:/trading-data/")

# Query example - software finds correct files
bars = catalog.bars(
    instrument_ids=["BTCUSDT.BINANCE", "ETHUSDT.BINANCE", "SOLUSDT.BINANCE"],
    start="2024-01-01",
    end="2026-12-31"
)

# Works seamlessly with:
# - C:/trading-data/historical/bars_1m/*.parquet (backfill)
# - C:/trading-data/recent/bars_1m/*.parquet (synced from S3)
```

---

## Appendix C: Quick Start Commands

```bash
# 1. Start collector (AWS VPS)
cd /opt/barter-data-server
./target/release/barter-data-server

# 2. Verify health
curl -s http://localhost:8080/health | jq

# 3. Check Parquet output
ls -la /data/parquet/bars_1m/

# 4. Sync to local (Windows)
aws s3 sync s3://trading-data/bars_1m/ C:\trading-data\recent\bars_1m\

# 5. Run Nautilus backtest
python run_backtest.py --start 2024-01-01 --end 2026-01-29

# 6. Run Nautilus sandbox (optional)
BARTER_ENABLED=1 python run_sandbox.py
```

---

## Appendix D: Implementation Specifications

### D.1 Nautilus Arrow Schema (Exact Specification)

**IMPORTANT: Precision Mode (2026-01-29)**

Nautilus supports **two precision modes**:
- **Standard**: `FixedSizeBinary(8)` with `i64 × 1e9`
- **High**: `FixedSizeBinary(16)` with `i128 × 1e16`

We control this in Barter with `NAUTILUS_PRECISION` (`standard` or `high`, default `high`).
This must match the Nautilus build you will read with.

**Bar Schema (7 columns, strict order):**
```rust
// Column order is CRITICAL - Nautilus uses positional indexing
use arrow::datatypes::{DataType, Field, Schema};

pub fn nautilus_bar_schema(metadata: HashMap<String, String>, precision_bytes: i32) -> Schema {
    Schema::new_with_metadata(vec![
        Field::new("open", DataType::FixedSizeBinary(precision_bytes), false),
        Field::new("high", DataType::FixedSizeBinary(precision_bytes), false),
        Field::new("low", DataType::FixedSizeBinary(precision_bytes), false),
        Field::new("close", DataType::FixedSizeBinary(precision_bytes), false),
        Field::new("volume", DataType::FixedSizeBinary(precision_bytes), false),
        Field::new("ts_event", DataType::UInt64, false),
        Field::new("ts_init", DataType::UInt64, false),
    ], metadata)
}

// Required metadata keys:
// - "bar_type": "BTCUSDT-PERP.BINANCE-1-MINUTE-LAST-EXTERNAL"
// - "instrument_id": "BTCUSDT-PERP.BINANCE"
// - "price_precision": "2"
// - "size_precision": "3"
```

**TradeTick Schema (6 columns, strict order):**
```rust
pub fn nautilus_trade_schema(metadata: HashMap<String, String>, precision_bytes: i32) -> Schema {
    Schema::new_with_metadata(vec![
        Field::new("price", DataType::FixedSizeBinary(precision_bytes), false),
        Field::new("size", DataType::FixedSizeBinary(precision_bytes), false),
        Field::new("aggressor_side", DataType::UInt8, false),
        Field::new("trade_id", DataType::Utf8, false),
        Field::new("ts_event", DataType::UInt64, false),
        Field::new("ts_init", DataType::UInt64, false),
    ], metadata)
}

// Required metadata keys:
// - "instrument_id": "BTCUSDT-PERP.BINANCE"
// - "price_precision": "2"
// - "size_precision": "3"
```

### D.2 Fixed-Point Encoding

```rust
// Precision mode selects scalar + byte width.
// Standard: 1e9, 8 bytes (i64)
// High:     1e16, 16 bytes (i128)

pub fn encode_fixed(value: f64, multiplier: f64, precision_bytes: i32) -> Vec<u8> {
    if precision_bytes == 8 {
        let raw = (value * multiplier).round() as i64;
        raw.to_le_bytes().to_vec()
    } else {
        let raw = (value * multiplier).round() as i128;
        raw.to_le_bytes().to_vec()
    }
}

pub fn decode_fixed(bytes: &[u8], multiplier: f64) -> f64 {
    match bytes.len() {
        8 => i64::from_le_bytes(bytes.try_into().unwrap()) as f64 / multiplier,
        16 => i128::from_le_bytes(bytes.try_into().unwrap()) as f64 / multiplier,
        _ => 0.0,
    }
}

// AggressorSide enum values (UInt8):
// 0 = NO_AGGRESSOR
// 1 = BUYER
// 2 = SELLER

// Example: Price 100000.00 -> raw = 100000.00 * 1e16 = 1e21
// Verified against Nautilus Price.from_str("100000.00").raw
```

### D.3 File Structure to Create

```
barter-rs/
├── barter-data-server/
│   ├── Cargo.toml                    # ADD: arrow, parquet, aws-sdk-s3
│   └── src/
│       ├── lib.rs                    # NEW: module exports
│       ├── main.rs                   # MODIFY: init parquet pipeline
│       ├── parquet/
│       │   ├── mod.rs                # NEW
│       │   ├── schema.rs             # NEW: Nautilus-compatible schemas
│       │   ├── encoder.rs            # NEW: fixed-point encoding
│       │   └── writer.rs             # NEW: buffered ParquetWriter
│       ├── aggregator/
│       │   ├── mod.rs                # NEW
│       │   ├── minute_bar.rs         # NEW: MinuteBarBuilder
│       │   ├── delta.rs              # NEW: Delta/CVD tracker
│       │   └── extended_bar.rs       # NEW: Extended bar with OI/funding
│       ├── storage/
│       │   ├── mod.rs                # NEW: StorageBackend trait
│       │   ├── local.rs              # NEW: LocalStorage
│       │   └── s3.rs                 # NEW: S3Storage with retry
│       ├── health/
│       │   ├── mod.rs                # NEW
│       │   └── heartbeat.rs          # NEW: JSON heartbeat writer
│       └── backfill/                 # Phase 2
│           ├── mod.rs                # NEW
│           ├── gap_detector.rs       # NEW
│           └── fetcher.rs            # NEW: Binance API fetcher
├── barter-tools/                     # NEW CRATE
│   ├── Cargo.toml                    # NEW
│   └── src/
│       └── bin/
│           ├── binance_downloader.rs # Phase 2
│           ├── csv_to_parquet.rs     # Phase 2
│           ├── verify_parquet.rs     # Phase 2
│           └── check_gaps.rs         # Phase 2
├── scripts/
│   ├── health-check.sh               # NEW
│   └── validation/
│       └── test_nautilus_load.py     # NEW: local validation script
└── config/
    └── backfill.yaml                 # Phase 2
```

### D.4 Cargo.toml Dependencies

```toml
# Add to barter-data-server/Cargo.toml [dependencies]
arrow = { version = "53", default-features = false, features = ["ipc"] }
parquet = { version = "53", default-features = false, features = ["arrow", "snap"] }
aws-config = "1.5"
aws-sdk-s3 = "1.65"
```

### D.5 Integration Points in main.rs

```rust
// Current main.rs has trade broadcast channel at ~line 760
// Hook parquet writer here:

// 1. Initialize parquet writer
let parquet_config = ParquetConfig::from_env();
let parquet_writer = ParquetWriter::new(parquet_config)?;

// 2. Initialize aggregator with writer
let aggregator = MinuteBarAggregator::new(parquet_writer.clone());

// 3. Spawn aggregator task (similar to existing run_aggregator_task)
tokio::spawn(async move {
    run_parquet_aggregator(trade_rx, aggregator).await;
});

// 4. In trade processing, send to aggregator channel
// (parallel to existing TUI aggregator)
```

### D.6 Verification Commands

```bash
# 1. Build and test schema
cargo build -p barter-data-server
cargo test -p barter-data-server parquet::

# 2. Generate sample Parquet file
cargo run -p barter-data-server --bin generate_sample_parquet

# 3. Verify in Nautilus (Python)
python scripts/validation/test_nautilus_load.py

# 4. Check timestamp alignment
python -c "
import pyarrow.parquet as pq
t = pq.read_table('/tmp/test_bars.parquet')
print('ts_event (close):', t['ts_event'][0].as_py())
# Extended bar should have ts_open = ts_event - 60_000_000_000 (60s in nanos)
"
```

### D.7 Gating Evidence Checklist

Before full implementation proceeds, produce:

- [ ] `test_bars.parquet` - Sample 1m bars file
- [ ] `test_trades.parquet` - Sample trades file
- [ ] Screenshot/log of Nautilus `ParquetDataCatalog` loading both files
- [ ] Timestamp alignment proof: ts_event = close time, ts_open = ts_event - 60s

---

*Document End*
