# Orchestrator Handoff: Market State Engine Implementation

> **Purpose:** This document enables a new orchestrator to coordinate parallel agents for implementing the Market State Engine. All context, boundaries, and coordination rules are captured here.

---

## 1. Project Context

### What We're Building
A **Hierarchical State Machine (HSM)** that synthesizes market signals into actionable WAIT/READY/CAUTION states. This replaces the current TUIs which show raw data without synthesis.

### Core Spec
**Location:** `barter-trading-tuis/docs/market_state.md`

### Key Architecture Decision
**Single binary** with multiple views, **not** microservices. All communication via lock-free primitives (`ArcSwap`, `tokio::sync::watch`).

---

## 2. Agent Coordination Rules

### Rule 1: TYPES FREEZE Gate

**Before parallel work begins:**
1. Agent A (Types Owner) defines ALL structs/enums in `market_state.rs`
2. Agent A defines config schema in `config/thresholds.toml`
3. All other agents review and approve
4. **FREEZE:** No changes to types after parallel phase starts
5. If type changes needed → stop parallel work, update types, re-freeze

### Rule 2: Single Owner for Integration

`calculate_state()` function is **owned by Agent A only**. No other agent touches this function. All other agents build modules that `calculate_state()` will call.

### Rule 3: Module Isolation

Each agent owns their module completely. No cross-module edits without explicit handoff.

---

## 3. Agent Assignments

### PHASE 0: Types Freeze (Sequential, 1 day)

| Owner | Deliverable | Files |
|-------|-------------|-------|
| **Agent A** | All type definitions | `src/shared/market_state.rs` (types only) |
| **Agent A** | Config schema | `config/thresholds.toml` |
| **Agent A** | Trait definitions | Any shared traits |

**Gate:** All agents review types. Once approved → FREEZE.

---

### PHASE 1: Core Modules (Parallel, 2-3 days)

| Agent | Module | Files | Dependencies |
|-------|--------|-------|--------------|
| **Agent A** | State Engine Logic | `src/shared/market_state.rs` | Types (frozen) |
| **Agent B** | Volatility Engine | `src/shared/vol_regime.rs` | Types (frozen) |
| **Agent C** | Audit Logger | `src/shared/audit.rs` | Types (frozen) |
| **Agent D** | Config Loader | `src/shared/config.rs` | Schema (frozen) |

**Interfaces (Frozen):**

```rust
// Agent B must implement:
pub trait VolatilityEngine {
    fn push_rv(&mut self, rv: f64);
    fn percentile(&self) -> f64;
    fn regime(&self) -> VolRegime;
    fn push_return_1m(&mut self, ret: f64);
    fn zscore_1m(&self) -> f64;
    fn is_shock(&self) -> bool;
}

// Agent C must implement:
pub trait AuditLog {
    fn log(&self, entry: AuditEntry);  // Non-blocking
}

// Agent D must implement:
pub trait ConfigProvider {
    fn thresholds(&self) -> &Thresholds;
    fn freshness(&self, signal: Signal) -> Duration;
}
```

---

### PHASE 2: External Integrations (Parallel, 2-3 days)

| Agent | Module | Files | Dependencies |
|-------|--------|-------|--------------|
| **Agent E** | Deribit Client | `src/shared/deribit.rs` | None |
| **Agent F** | Gamma Calculator | `src/shared/gamma.rs` | Deribit types |
| **Agent B** | Funding Tracker | `src/shared/funding.rs` | Types (frozen) |

**Interfaces:**

```rust
// Agent E must implement:
pub trait DeribitClient {
    async fn fetch_options_chain(&self, ticker: &str) -> Result<OptionsChain>;
}

// Agent F must implement:
pub trait GammaEngine {
    fn calculate_flip(&self, chain: &OptionsChain, spot: f64) -> f64;
    fn nearest_walls(&self, chain: &OptionsChain, spot: f64) -> (Option<Wall>, Option<Wall>);
}

// Agent B (Funding) must implement:
pub trait FundingTracker {
    fn push(&mut self, ts: i64, rate: f64);
    fn velocity(&self) -> f64;
    fn is_spiking(&self) -> bool;
    fn is_extreme(&self, rate: f64) -> bool;
}
```

---

### PHASE 3: Integration (Sequential, 2 days)

| Owner | Task |
|-------|------|
| **Agent A** | Wire all modules into `calculate_state()` |
| **Agent A** | Implement freshness gate checks |
| **Agent A** | Implement NO-TRAD / NO-GAMMA fallbacks |
| **Agent A** | Integration tests |

---

### PHASE 4: TUI Views (Parallel after Phase 3, 2-3 days)

| Agent | View | File |
|-------|------|------|
| **Agent G** | Global Radar | `src/views/global_radar.rs` |
| **Agent H** | Execution Cockpit | `src/views/execution.rs` |
| **Agent I** | Debug View | `src/views/debug.rs` |

**Shared contract:** All views receive `Arc<ArcSwap<MarketState>>` and render it. No business logic in views.

---

## 4. File Ownership Matrix

| File | Owner | Can Edit |
|------|-------|----------|
| `src/shared/market_state.rs` | Agent A | Agent A only |
| `src/shared/vol_regime.rs` | Agent B | Agent B only |
| `src/shared/audit.rs` | Agent C | Agent C only |
| `src/shared/config.rs` | Agent D | Agent D only |
| `src/shared/deribit.rs` | Agent E | Agent E only |
| `src/shared/gamma.rs` | Agent F | Agent F only |
| `src/shared/funding.rs` | Agent B | Agent B only |
| `src/views/*.rs` | Agent G/H/I | Respective owner |
| `src/bin/trading_terminal.rs` | Agent A | Agent A only |
| `config/thresholds.toml` | Agent D | After freeze: Agent D only |
| `Cargo.toml` | Orchestrator | Orchestrator only |

---

## 5. Coordination Checklist

### Before Starting Any Phase

- [ ] All agents have read `docs/market_state.md`
- [ ] Types freeze is complete and acknowledged
- [ ] Each agent knows their file ownership
- [ ] Each agent knows their interface contract

### During Parallel Work

- [ ] No agent edits files outside their ownership
- [ ] If type change needed → STOP → notify orchestrator
- [ ] Each agent writes unit tests for their module
- [ ] Each agent documents public functions

### Before Integration Phase

- [ ] All parallel modules pass their unit tests
- [ ] All modules implement their interface contracts
- [ ] No compilation errors when modules combined
- [ ] Agent A has reviewed all module APIs

### Before TUI Phase

- [ ] `calculate_state()` returns correct states (integration tests pass)
- [ ] Audit logging works (test file created)
- [ ] Config loading works (thresholds applied)

---

## 6. Critical Thresholds (From Spec)

| Threshold | Value | Purpose |
|-----------|-------|---------|
| Vol percentile extreme | ≥95th | L1 WAIT |
| Vol percentile high | 80-95th | CAUTION |
| Z-Score shock | ≥3.5σ | L1 WAIT |
| CVD consensus | ≥66% (2/3) | L3 gate |
| Funding spike | >0.02%/15m | CAUTION |
| Funding extreme long | >0.05% | Warning |
| Funding extreme short | <-0.02% | Warning |

---

## 7. Freshness Gates (From Spec)

| Signal | Max Staleness | Fallback |
|--------|---------------|----------|
| Price | 1 second | WAIT |
| CVD | 2 seconds | WAIT |
| L2 Book | 2 seconds | Use last known |
| Whale | 10 seconds | Ignore filter |
| Funding | 5 minutes | Use last known |
| Gamma | 10 minutes | NO-GAMMA mode |
| Trad Markets | 2 minutes | NO-TRAD mode |

---

## 8. Quality Gates

### Per-Module (Parallel Phase)

Each agent must deliver:
1. **Code:** Module compiles with no warnings
2. **Tests:** ≥80% coverage on public functions
3. **Docs:** All public functions documented
4. **Interface:** Implements assigned trait

### Integration (Sequential Phase)

Agent A must deliver:
1. **Integration tests** for state transitions
2. **Freshness gate** tests (stale data handling)
3. **Fallback mode** tests (NO-TRAD, NO-GAMMA)
4. **Audit log** verification (entries written correctly)

### Final Acceptance

- [ ] `cargo test` passes all tests
- [ ] `cargo clippy` has no warnings
- [ ] TUI renders correctly with live data
- [ ] State flips are logged to audit file
- [ ] Manual test: verify WAIT state during high vol

---

## 9. Communication Protocol

### Sync Points

1. **After Types Freeze:** All agents confirm they have the types
2. **Daily during parallel:** Quick status (blocked/on-track)
3. **Before integration:** All modules ready handoff
4. **Before TUI:** Integration complete handoff

### Escalation

If an agent is blocked:
1. Post blocker to orchestrator immediately
2. If blocker requires type change → orchestrator decides
3. If blocker is cross-module → orchestrator mediates

---

## 10. Rollback Plan

If integration fails:

1. **Identify** which module is causing failure
2. **Isolate** that module with mock
3. **Fix** module in isolation
4. **Re-integrate** with tests

If everything fails:

1. Revert to existing `scalper-v2` (still works)
2. Debug `trading-terminal` separately
3. No production impact (new code is additive)

---

## 11. Success Criteria

### Phase 1 Complete When:
- All 4 modules compile together
- Unit tests pass
- `MarketState::calculate()` returns valid states with mock data

### Phase 2 Complete When:
- Deribit client fetches real data
- Gamma flip calculated correctly
- Funding velocity tracked

### Phase 3 Complete When:
- Live data produces correct states
- Audit log captures state flips
- Freshness gates working

### Phase 4 Complete When:
- TUI displays state correctly
- View switching works
- Manual testing passes

### Project Complete When:
- Running in tmux alongside existing TUIs
- Audit log accumulating entries
- No crashes for 24 hours

---

## 12. Reference Links

| Document | Location |
|----------|----------|
| Full Spec | `docs/market_state.md` |
| Config Schema | `config/thresholds.toml` (to be created) |
| Existing State Code | `src/shared/state.rs` |
| Existing TUI | `src/bin/scalper_v2.rs` |

---

## 13. Orchestrator Checklist

### Day 1
- [ ] Review this handoff document
- [ ] Review `docs/market_state.md` spec
- [ ] Assign agents to roles
- [ ] Initiate Types Freeze (Agent A)

### Day 2
- [ ] Confirm Types Freeze complete
- [ ] Start parallel Phase 1
- [ ] Daily check-in with agents

### Day 3-4
- [ ] Monitor parallel progress
- [ ] Resolve any blockers
- [ ] Prepare for integration

### Day 5-6
- [ ] Start Phase 2 (can overlap with Phase 1 completion)
- [ ] Agent A begins integration prep

### Day 7-8
- [ ] Integration phase (Agent A)
- [ ] Other agents on standby for fixes

### Day 9-10
- [ ] TUI views (parallel)
- [ ] Final testing

### Day 11-12
- [ ] Manual testing
- [ ] Bug fixes
- [ ] Sign-off

---

**Handoff Complete.** New orchestrator has all context needed to coordinate implementation.
