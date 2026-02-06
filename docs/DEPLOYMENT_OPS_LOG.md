# Deployment Operations Log

## 2026-02-07: L2 Compute Mode + Healthcheck Fix

**VPS:** Hetzner CX32, `deployer@46.62.142.15`, Ubuntu 24.04

### Changes Deployed

1. **Enable L2 streaming for depth-band computation** (commit 13f61a2)
   - `STREAM_L2=1` — subscribe to Binance L2 order book
   - `PARQUET_WRITE_L2=0` — do NOT write L2 deltas to parquet (saves ~15GB/day)
   - L2 data used only for computing depth bands in `extended_bars_1m`

2. **Fix compose healthcheck quoting** (this session)
   - podman-compose 1.0.6 mangles list-style healthcheck `["CMD", "sh", "-c", "..."]`
   - Changed to string form: `test: "test -f /data/ipc/collector-heartbeat.json"`
   - Without fix, healthcheck runs garbled command and always returns ExitCode:1

3. **Fix deploy scripts for `deployer` username** (this session)
   - `deploy.sh`: `deploy@vps` → `deployer@vps`, `/home/deploy/` → `/home/deployer/`
   - `setup-vps.sh`: all `deploy` user refs → `deployer`, `systemctl restart sshd` → `ssh` (Ubuntu 24.04)
   - `backup.sh`: cron path `/home/deploy/` → `/home/deployer/`

### Verification (raw outputs)

**Container env:**
```
STREAM_L2=1 PARQUET_WRITE_L2=0
```

**Depth bands non-zero** (file: `extended_bars_1m/.../18_0.parquet`):
```
                                  1
book_imbalance         3.383876e-01
bid_depth_10bps_base   1.241640e+11
ask_depth_10bps_base   7.292200e+10
depth_imb_10bps        2.594671e-01
bid_depth_50bps_base   9.771130e+11
ask_depth_50bps_base   8.710870e+11
depth_imb_50bps        5.470878e-02
bid_depth_100bps_base  1.416800e+12
ask_depth_100bps_base  1.141262e+12
depth_imb_100bps       1.037163e-01
```

**Healthcheck (after fix):**
```json
{"Status":"healthy","FailingStreak":0,"Log":[{"ExitCode":0},{"ExitCode":0},{"ExitCode":0}]}
```

### Extended bars schema (43 columns)

| Category | Columns |
|----------|---------|
| Time | `ts_event`, `ts_init`, `ts_open` |
| Identity | `instrument_id` |
| OHLCV | `open`, `high`, `low`, `close`, `volume`, `quote_volume`, `trade_count` |
| Delta | `buy_volume`, `sell_volume`, `delta`, `cvd` |
| Derivatives | `open_interest`, `oi_change`, `funding_rate` |
| L1 | `bid_price`, `bid_size`, `ask_price`, `ask_size`, `spread_bps`, `book_imbalance` |
| Liquidations | `liq_buy_usd`, `liq_sell_usd`, `liq_total_usd`, `liq_count` |
| Depth 10bps | `bid_depth_10bps_base`, `ask_depth_10bps_base`, `bid_depth_10bps_usd`, `ask_depth_10bps_usd`, `depth_imb_10bps` |
| Depth 50bps | `bid_depth_50bps_base`, `ask_depth_50bps_base`, `bid_depth_50bps_usd`, `ask_depth_50bps_usd`, `depth_imb_50bps` |
| Depth 100bps | `bid_depth_100bps_base`, `ask_depth_100bps_base`, `bid_depth_100bps_usd`, `ask_depth_100bps_usd`, `depth_imb_100bps` |

### Parquet output status

| Type | Files | Note |
|------|-------|------|
| `trades/` | Writing | ~31K rows per flush |
| `extended_bars_1m/` | Writing | 43 columns, depth bands populated |
| `bars_1m/` | Writing | Nautilus binary-encoded OHLCV |
| `order_book_deltas/` | **None** | Correct — `PARQUET_WRITE_L2=0` |

### Known issue: podman-compose quoting

podman-compose 1.0.6 has a known bug with list-style healthcheck commands. Always use string form:

```yaml
# BAD — podman-compose mangles this
healthcheck:
  test: ["CMD", "sh", "-c", "test -f /path/to/file"]

# GOOD — works correctly
healthcheck:
  test: "test -f /path/to/file"
```
