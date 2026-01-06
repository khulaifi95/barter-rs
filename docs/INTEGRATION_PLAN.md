# Centralized Data Integration Plan

This checklist tracks progress for the centralized data migration and
prevents missed commits or context loss. Each sub-phase must end with a
clean git tree and a commit recorded here.

## Status Legend

- [ ] Not started
- [x] Done
- (commit: <hash>) Commit recorded

## Phase A — Baseline & Safety Rails (no behavior change)

- [x] A1: Add versioned message envelope in barter-data (schema version, source, timestamp). (commit: 25ecee2)
- [x] A2: Feature flags in TUIs to prefer server data but allow fallback to direct fetch. (commit: 41282e9)
- [x] A3: Shadow-mode logging for new event types without UI changes. (commit: b1689d8)

## Phase B — Centralize External Sources (raw feeds)

- [x] B1: IBKR feed ingestion in barter-data-server + new event type. (commit: 168c6b0)
- [ ] B2: Deribit options poller in barter-data-server + options snapshot event. (commit: )
- [ ] B3: TUIs parse new event types with fallback paths. (commit: )

## Phase C — Centralize Calculations (derived metrics)

- [ ] C1: Define MarketSnapshot schema for derived metrics. (commit: )
- [ ] C2: Server-side snapshot builder (RVOL/CVD/vol regime/fuel). (commit: )
- [ ] C3: TUIs consume snapshot + drift debug panel. (commit: )

## Phase D — Cutover & Cleanup

- [ ] D1: Remove direct Deribit/IBKR connections from TUIs. (commit: )
- [ ] D2: Remove per-TUI rolling windows or guard behind debug flags. (commit: )
- [ ] D3: Lock thresholds and finalize docs. (commit: )

## Phase E — TUI Harmonization

- [ ] E1: Trading-terminal as macro cockpit. (commit: )
- [ ] E2: scalper_v2 as execution cockpit. (commit: )
- [ ] E3: micro-structure as deep-dive/debug. (commit: )

## Build/Test Gates

- [x] Build: `cargo build -p barter-data-server` (commit: 25ecee2)
- [x] Build: `cargo build -p barter-trading-tuis --bins` (commit: 41282e9)
- [ ] Tests: `cargo test -p barter-trading-tuis --lib` (commit: )
