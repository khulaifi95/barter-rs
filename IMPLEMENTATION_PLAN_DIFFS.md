## Plan Diffs – Original vs Enhanced

This note highlights the key shifts from `IMPLEMENTATION_PLAN.md` to `IMPLEMENTATION_PLAN_ENHANCED.md` so we have a paper trail before further refactors.

### Architecture & Scope
- Original: 3 separate TUIs (microstructure, institutional, risk).
- Enhanced: Move toward a unified binary with view modes; add scalper mode (5s/15s/30s).
- Windows expanded: beyond 1m/5m to include 30s and shorter for velocity signals.

### Metrics & Panels
- CVD: Tighten neutral band (45–55 → 48–52), add 30s window, add flow signals (accumulation/distribution/exhaustion).
- Market Stats: Compact, one-line per asset; include top-3 dominance tags (EXCH-KIND), OI Δ5m + direction, speed tag; UX cap 1–2 lines.
- Basis: Add 1m/5m deltas with widening/narrowing tags; keep concise (1–2 lines).
- OI: Add Δ5m and velocity as a high-value addition.

### UX/Display Priorities (New)
- Readability/actionability first: abbreviated labels, at most 2–3 lines per ticker, minimal color (only for state/direction), avoid wall-of-text.
- Dominance limited to top-3 entries; numbers abbreviated.

### Quick Wins (P0)
- CVD band tightening, 30s window, compact Market Stats, add flow signals.

### High-Value Additions (P1)
- Basis momentum; OI velocity.

### What’s Deferred (P2)
- Unified binary with hotkeys; scalper mode with sub-30s deltas.

### Notes
- This file is documentation only; no code changes here. It captures the divergence from the original plan before we proceed with any further refactor or commits.
