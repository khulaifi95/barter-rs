# Barter-Nautilus Rust Integration Plan

## Executive Summary

**Goal:** Replace pure-Python `barter-nautilus-data` package with a high-performance Rust core + thin Python wrapper (PyO3) for low-latency backtesting and large dataset processing.

**Decision (2026-02-05):** **Deferred** until resource limits are proven in practice.  
We will **keep the Python integration as the default** and only activate this plan if real backtests hit
RAM or latency ceilings. This keeps correctness and compatibility the priority while we finish
Parquet coverage and end-to-end Nautilus validation.

**Trigger conditions to activate this plan:**
- OOM or >60–70% RAM pressure when backtesting 3–6 months of data on target hardware.
- Parquet load time dominates total backtest runtime (e.g., >30–40% wall-clock).
- Repeated research runs on large datasets where streaming readers would materially reduce time/Cost.

**Tradeoff summary (why defer now):**
- ✅ Faster delivery of *correctness* (schemas + feature coverage + Nautilus validation).
- ✅ Lower integration risk (keep Python entrypoints stable).
- ❌ Potentially slower backtests on very large datasets (acceptable short-term).

**Current State:**
- `barter-nautilus-data`: 180 LOC Python (schema + CustomData classes)
- `barter-features`: 3,500+ LOC Rust (TPO, large trades, events)
- `barter-data-server/parquet`: 1,666 LOC Rust (encoding, schemas, writer)
- Validation scripts: 2,316 LOC Python

**Target State:**
- Single `barter-nautilus` Rust crate with PyO3 bindings
- Streaming parquet reader (no `.collect()` into RAM)
- Batch iterator with prefetch support
- Full Nautilus wrangler compatibility

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         barter-nautilus (Rust + PyO3)                   │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐      │
│  │  Parquet Reader  │  │  Data Wranglers  │  │  Feature Compute │      │
│  │  (streaming)     │  │  (Arrow IPC)     │  │  (TPO, Events)   │      │
│  └────────┬─────────┘  └────────┬─────────┘  └────────┬─────────┘      │
│           │                     │                     │                 │
│           └─────────────────────┼─────────────────────┘                 │
│                                 │                                        │
│                    ┌────────────▼────────────┐                          │
│                    │   PyO3 Bindings Layer   │                          │
│                    │   (#[pyclass], etc.)    │                          │
│                    └────────────┬────────────┘                          │
│                                 │                                        │
└─────────────────────────────────┼────────────────────────────────────────┘
                                  │
                    ┌─────────────▼─────────────┐
                    │   Python Thin Wrapper     │
                    │   (barter_nautilus.py)    │
                    └─────────────┬─────────────┘
                                  │
                    ┌─────────────▼─────────────┐
                    │   Nautilus Trader         │
                    │   (BacktestEngine, etc.)  │
                    └───────────────────────────┘
```

---

## Parallel Workstream Breakdown

### Agent 1: Core Rust Crate Setup & Parquet Streaming Reader
**Owner:** Rust specialist
**Duration:** 2-3 days (prototype) → 1 week (production)

**Tasks:**
1. Create `barter-nautilus/` crate structure
   - `Cargo.toml` with pyo3, arrow, parquet dependencies
   - Feature flags: `python` (default), `simd` (optional)

2. Implement streaming parquet reader
   ```rust
   pub struct ParquetStreamReader {
       reader: SerializedFileReader<File>,
       batch_size: usize,
       current_batch: Option<RecordBatch>,
   }

   impl Iterator for ParquetStreamReader {
       type Item = Result<RecordBatch>;
       fn next(&mut self) -> Option<Self::Item>;
   }
   ```

3. Add prefetch support (background thread)
   ```rust
   pub struct PrefetchReader {
       receiver: Receiver<RecordBatch>,
       prefetch_thread: JoinHandle<()>,
   }
   ```

4. Port precision encoding from `barter-data-server/parquet/encoder.rs`

**Deliverables:**
- [ ] `barter-nautilus/Cargo.toml`
- [ ] `barter-nautilus/src/lib.rs`
- [ ] `barter-nautilus/src/reader/mod.rs`
- [ ] `barter-nautilus/src/reader/streaming.rs`
- [ ] `barter-nautilus/src/reader/prefetch.rs`
- [ ] `barter-nautilus/src/precision.rs`
- [ ] Unit tests for streaming reader

**Dependencies:** None (can start immediately)

---

### Agent 2: Data Wranglers (Nautilus-Compatible)
**Owner:** Integration specialist
**Duration:** 2-3 days (prototype) → 1 week (production)

**Tasks:**
1. Implement `TradeTickWrangler` (matches Nautilus pattern)
   ```rust
   #[pyclass]
   pub struct TradeTickWrangler {
       instrument_id: String,
       price_precision: u8,
       size_precision: u8,
       metadata: HashMap<String, String>,
   }

   #[pymethods]
   impl TradeTickWrangler {
       fn process_record_batch_bytes(&self, data: &[u8]) -> PyResult<Vec<PyObject>>;
       fn from_parquet_file(&self, path: &str) -> PyResult<Vec<PyObject>>;
   }
   ```

2. Implement `BarWrangler` (1-minute bars)

3. Implement `ExtendedBarWrangler` (43-field schema)

4. Implement `TpoBracketWrangler` (feature output)

5. Implement `LargeTradeWrangler` (feature output)

6. Arrow IPC serialization for all types

**Deliverables:**
- [ ] `barter-nautilus/src/wranglers/mod.rs`
- [ ] `barter-nautilus/src/wranglers/trade.rs`
- [ ] `barter-nautilus/src/wranglers/bar.rs`
- [ ] `barter-nautilus/src/wranglers/extended_bar.rs`
- [ ] `barter-nautilus/src/wranglers/tpo_bracket.rs`
- [ ] `barter-nautilus/src/wranglers/large_trade.rs`
- [ ] Integration tests with real parquet files

**Dependencies:** Agent 1 (precision module)

---

### Agent 3: PyO3 Bindings & Python Package
**Owner:** Python/FFI specialist
**Duration:** 1-2 days (prototype) → 3-4 days (production)

**Tasks:**
1. Define PyO3 module structure
   ```rust
   #[pymodule]
   fn barter_nautilus(_py: Python, m: &PyModule) -> PyResult<()> {
       m.add_class::<TradeTickWrangler>()?;
       m.add_class::<BarWrangler>()?;
       m.add_class::<ExtendedBarWrangler>()?;
       m.add_class::<TpoBracketWrangler>()?;
       m.add_class::<LargeTradeWrangler>()?;
       m.add_class::<ParquetStreamReader>()?;
       m.add_function(wrap_pyfunction!(read_trades, m)?)?;
       m.add_function(wrap_pyfunction!(read_bars, m)?)?;
       Ok(())
   }
   ```

2. Create maturin build configuration
   ```toml
   # pyproject.toml
   [build-system]
   requires = ["maturin>=1.0,<2.0"]
   build-backend = "maturin"

   [project]
   name = "barter-nautilus"
   requires-python = ">=3.10"
   dependencies = ["nautilus_trader>=1.222.0"]
   ```

3. Implement thin Python wrapper
   ```python
   # barter_nautilus/__init__.py
   from .barter_nautilus import (
       TradeTickWrangler,
       BarWrangler,
       ExtendedBarWrangler,
       read_trades,
       read_bars,
   )

   class TradeTickWranglerV2:
       """Nautilus-compatible wrapper."""
       def __init__(self, instrument_id: str, price_precision: int, size_precision: int):
           self._inner = TradeTickWrangler(instrument_id, price_precision, size_precision)

       def from_parquet(self, path: str) -> list:
           return self._inner.from_parquet_file(path)

       def from_arrow(self, table) -> list:
           # Arrow IPC conversion
           ...
   ```

4. Add type stubs (`.pyi` files) for IDE support

**Deliverables:**
- [ ] `barter-nautilus/src/python/mod.rs`
- [ ] `barter-nautilus/pyproject.toml`
- [ ] `barter-nautilus/python/barter_nautilus/__init__.py`
- [ ] `barter-nautilus/python/barter_nautilus/wranglers.py`
- [ ] `barter-nautilus/python/barter_nautilus/__init__.pyi`
- [ ] CI/CD with maturin (GitHub Actions)

**Dependencies:** Agent 1 + Agent 2

---

### Agent 4: Feature Layer Integration
**Owner:** Feature computation specialist
**Duration:** 2-3 days (prototype) → 1 week (production)

**Tasks:**
1. Expose `barter-features` processors via PyO3
   ```rust
   #[pyclass]
   pub struct TpoProcessor {
       inner: barter_features::TpoProcessor,
   }

   #[pymethods]
   impl TpoProcessor {
       #[new]
       fn new(config: &PyDict) -> PyResult<Self>;
       fn process_bars(&mut self, bars: Vec<ExtendedBar>) -> PyResult<Vec<TpoBracket>>;
       fn get_current_bracket(&self) -> PyResult<Option<TpoBracket>>;
   }
   ```

2. Expose `LargeTradeDetector`

3. Expose `EventDetector`

4. Add streaming feature computation
   ```rust
   impl TpoProcessor {
       fn process_bar_stream(&mut self, reader: &mut ParquetStreamReader) -> PyResult<Vec<TpoBracket>>;
   }
   ```

5. Integrate with Nautilus CustomData registration

**Deliverables:**
- [ ] `barter-nautilus/src/features/mod.rs`
- [ ] `barter-nautilus/src/features/tpo.rs`
- [ ] `barter-nautilus/src/features/large_trades.rs`
- [ ] `barter-nautilus/src/features/events.rs`
- [ ] Python integration tests with Nautilus backtest

**Dependencies:** Agent 1 + Agent 2 + `barter-features` crate

---

## Timeline & Milestones

### Phase 1: MVP (Prototype)
**Duration:** 3-4 days with 4 parallel agents

| Day | Agent 1 | Agent 2 | Agent 3 | Agent 4 |
|-----|---------|---------|---------|---------|
| 1 | Crate setup, streaming reader | Trade wrangler | - | - |
| 2 | Prefetch, precision | Bar + ExtendedBar wranglers | PyO3 module setup | - |
| 3 | Integration tests | TpoBracket wrangler | Python wrapper | TpoProcessor binding |
| 4 | - | Large trade wrangler | maturin build | LargeTradeDetector |

**MVP Deliverables:**
- Streaming parquet reader (no RAM collect)
- Trade/Bar/ExtendedBar wranglers
- Basic Python package with maturin
- TpoProcessor exposed to Python

### Phase 2: Production-Ready
**Duration:** 1 week additional

| Task | Duration | Owner |
|------|----------|-------|
| Comprehensive test suite | 2 days | All agents |
| Error handling & edge cases | 1 day | Agent 1 + 2 |
| Documentation & examples | 1 day | Agent 3 |
| Performance benchmarks | 1 day | Agent 1 |
| CI/CD pipeline (wheels) | 1 day | Agent 3 |
| Nautilus backtest integration test | 1 day | Agent 4 |

### Phase 3: Enhancements (Optional)
**Duration:** As needed

- SIMD-accelerated encoding/decoding
- Multi-threaded prefetch with configurable buffer
- S3/GCS streaming support
- Real-time feature computation integration

---

## Risk Assessment & Mitigation

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| PyO3 version incompatibility | Medium | High | Pin pyo3 version, test with Nautilus deps |
| Arrow schema mismatch | Low | High | Use Nautilus test files as reference |
| Performance regression | Low | Medium | Benchmark against pure Python baseline |
| Async runtime conflict | Medium | Medium | Expose sync wrappers, avoid tokio in bindings |
| Memory leaks in FFI | Low | High | Use `#[pyclass]` properly, add drop tests |

---

## Effort Estimation Summary

| Component | Prototype | Production | Total |
|-----------|-----------|------------|-------|
| Streaming Reader | 2 days | 3 days | 5 days |
| Data Wranglers | 3 days | 4 days | 7 days |
| PyO3/Python Package | 2 days | 3 days | 5 days |
| Feature Integration | 2 days | 4 days | 6 days |
| Testing & CI | 1 day | 3 days | 4 days |
| **Total (Sequential)** | **10 days** | **17 days** | **27 days** |
| **Total (4 Parallel Agents)** | **3-4 days** | **5-6 days** | **8-10 days** |

---

## Quick Start Commands

```bash
# Clone and setup
cd /Users/screener-m3/projects/barter-rs
mkdir -p barter-nautilus/src/{reader,wranglers,features,python}

# Initialize Cargo.toml
cat > barter-nautilus/Cargo.toml << 'EOF'
[package]
name = "barter-nautilus"
version = "0.1.0"
edition = "2021"

[lib]
name = "barter_nautilus"
crate-type = ["cdylib", "rlib"]

[dependencies]
pyo3 = { version = "0.21", features = ["extension-module"] }
arrow = "54"
parquet = "54"
thiserror = "1.0"
serde = { version = "1.0", features = ["derive"] }

[dependencies.barter-features]
path = "../barter-features"

[features]
default = ["python"]
python = []
EOF

# Build with maturin
uv pip install maturin
cd barter-nautilus
maturin develop --release
```

---

## Success Criteria

1. **Performance:** 10x faster parquet loading than pure Python
2. **Memory:** Streaming reader uses <100MB for any file size
3. **Compatibility:** All existing Nautilus backtests pass
4. **Latency:** <1ms per batch decode (10,000 rows)
5. **Coverage:** >80% test coverage on Rust code

---

## Appendix: File Structure

```
barter-nautilus/
├── Cargo.toml
├── pyproject.toml
├── src/
│   ├── lib.rs                    # Crate root
│   ├── error.rs                  # Error types
│   ├── precision.rs              # Fixed-point encoding
│   ├── reader/
│   │   ├── mod.rs
│   │   ├── streaming.rs          # Iterator-based reader
│   │   └── prefetch.rs           # Background prefetch
│   ├── wranglers/
│   │   ├── mod.rs
│   │   ├── trade.rs              # TradeTick wrangler
│   │   ├── bar.rs                # Bar wrangler
│   │   ├── extended_bar.rs       # ExtendedBar wrangler
│   │   ├── tpo_bracket.rs        # TPO output wrangler
│   │   └── large_trade.rs        # Large trade wrangler
│   ├── features/
│   │   ├── mod.rs
│   │   ├── tpo.rs                # TpoProcessor binding
│   │   ├── large_trades.rs       # LargeTradeDetector binding
│   │   └── events.rs             # EventDetector binding
│   └── python/
│       └── mod.rs                # PyO3 module definition
├── python/
│   └── barter_nautilus/
│       ├── __init__.py           # Package exports
│       ├── wranglers.py          # Thin wrappers
│       └── __init__.pyi          # Type stubs
├── tests/
│   ├── test_streaming.rs
│   ├── test_wranglers.rs
│   └── test_features.rs
└── examples/
    ├── backtest_example.py
    └── streaming_example.py
```
