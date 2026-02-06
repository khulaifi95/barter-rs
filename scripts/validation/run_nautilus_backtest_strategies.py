#!/usr/bin/env python3
"""
Run Nautilus backtests against a Barter catalog using basic strategies.

Modes:
  - ema: candle-based EMA cross strategy
  - custom: subscribes to Barter custom data (ExtendedBar1m, TPO, LargeTrade, ProfileEvent)
  - orderbook: subscribes to OrderBookDeltas (L2) and counts batches

Usage:
  python scripts/validation/run_nautilus_backtest_strategies.py --catalog-dir /tmp/nautilus_catalog_e2e --strategy ema
  python scripts/validation/run_nautilus_backtest_strategies.py --catalog-dir /tmp/nautilus_catalog_e2e --strategy custom
  python scripts/validation/run_nautilus_backtest_strategies.py --catalog-dir /tmp/nautilus_catalog_e2e --strategy orderbook
"""

import argparse
import sys
from decimal import Decimal
from pathlib import Path

try:
    from nautilus_trader.backtest.engine import BacktestEngine, BacktestEngineConfig
    from nautilus_trader.config import LoggingConfig
    from nautilus_trader.model.currencies import USD
    from nautilus_trader.model.data import BarType, DataType, OrderBookDeltas
    from nautilus_trader.model.enums import AccountType, OmsType
    from nautilus_trader.model.identifiers import ClientId, TraderId, Venue, InstrumentId, Symbol
    from nautilus_trader.model.instruments import CryptoPerpetual
    from nautilus_trader.model.objects import Money, Price, Quantity, Currency
    from nautilus_trader.persistence.catalog import ParquetDataCatalog
    from nautilus_trader.trading.strategy import Strategy, StrategyConfig
    from nautilus_trader.examples.strategies.ema_cross import EMACross, EMACrossConfig
except ImportError as e:
    print(f"ERROR: missing dependency: {e}")
    print("Run from the nautilus_trader repo using: uv run python <script>")
    sys.exit(1)


class CustomDataCounterConfig(StrategyConfig, frozen=True):
    instrument_id: InstrumentId
    bar_type: str = "BTCUSDT-PERP.BINANCE-1-MINUTE-LAST-EXTERNAL"
    log_every: int = 250


class CustomDataCounter(Strategy):
    def __init__(self, config: CustomDataCounterConfig):
        super().__init__(config)
        self.bar_type = BarType.from_str(config.bar_type)
        self.bars = 0
        self.ext = 0
        self.tpo = 0
        self.large = 0
        self.profile = 0

    def on_start(self):
        self.subscribe_bars(self.bar_type)
        # Subscribe to custom data types using instrument_id-based topics
        self.subscribe_data(DataType(ExtendedBar1m), instrument_id=self.config.instrument_id)
        self.subscribe_data(DataType(TpoBracket), instrument_id=self.config.instrument_id)
        self.subscribe_data(DataType(LargeTrade), instrument_id=self.config.instrument_id)
        self.subscribe_data(DataType(ProfileEvent), instrument_id=self.config.instrument_id)
        self.log.info("Custom data subscriptions active")

    def on_bar(self, bar):
        self.bars += 1
        if self.bars <= 3 or self.bars % self.config.log_every == 0:
            self.log.info(f"Bar {self.bars}: {bar}")

    def on_data(self, data):
        if isinstance(data, ExtendedBar1m):
            self.ext += 1
        elif isinstance(data, TpoBracket):
            self.tpo += 1
        elif isinstance(data, LargeTrade):
            self.large += 1
        elif isinstance(data, ProfileEvent):
            self.profile += 1

    def on_stop(self):
        self.log.info(
            f"Processed bars={self.bars}, extended={self.ext}, tpo={self.tpo}, "
            f"large={self.large}, profile={self.profile}"
        )


class OrderBookDeltaCounterConfig(StrategyConfig, frozen=True):
    instrument_id: InstrumentId
    log_every: int = 250


class OrderBookDeltaCounter(Strategy):
    def __init__(self, config: OrderBookDeltaCounterConfig):
        super().__init__(config)
        self.batches = 0
        self.deltas = 0

    def on_start(self):
        self.subscribe_order_book_deltas(self.config.instrument_id)
        self.log.info("Order book delta subscriptions active")

    def on_order_book_deltas(self, deltas: OrderBookDeltas):
        self.batches += 1
        self.deltas += len(deltas.deltas)
        if self.batches <= 3 or self.batches % self.config.log_every == 0:
            self.log.info(f"OrderBookDeltas batch={self.batches} deltas={len(deltas.deltas)}")

    def on_stop(self):
        self.log.info(f"OrderBookDeltas batches={self.batches} total_deltas={self.deltas}")

def build_engine(verbose: bool) -> BacktestEngine:
    engine_config = BacktestEngineConfig(
        trader_id=TraderId("TESTER-001"),
        logging=LoggingConfig(log_level="INFO" if verbose else "WARNING"),
    )
    return BacktestEngine(config=engine_config)


def add_instrument(engine: BacktestEngine) -> InstrumentId:
    BINANCE = Venue("BINANCE")
    engine.add_venue(
        venue=BINANCE,
        oms_type=OmsType.NETTING,
        account_type=AccountType.MARGIN,
        base_currency=USD,
        starting_balances=[Money(100_000, USD)],
    )

    instrument_id = InstrumentId(Symbol("BTCUSDT-PERP"), BINANCE)
    btcusdt_perp = CryptoPerpetual(
        instrument_id=instrument_id,
        raw_symbol=Symbol("BTCUSDTPERP"),
        base_currency=Currency.from_str("BTC"),
        quote_currency=Currency.from_str("USDT"),
        settlement_currency=Currency.from_str("USDT"),
        is_inverse=False,
        price_precision=2,
        size_precision=3,
        price_increment=Price.from_str("0.01"),
        size_increment=Quantity.from_str("0.001"),
        lot_size=Quantity.from_str("1"),
        max_quantity=Quantity.from_str("10000"),
        min_quantity=Quantity.from_str("0.001"),
        max_notional=None,
        min_notional=Money.from_str("10 USDT"),
        max_price=Price.from_str("1000000.00"),
        min_price=Price.from_str("0.01"),
        margin_init=Decimal("0.05"),
        margin_maint=Decimal("0.025"),
        maker_fee=Decimal("0.0002"),
        taker_fee=Decimal("0.0004"),
        ts_event=0,
        ts_init=0,
    )
    engine.add_instrument(btcusdt_perp)
    return instrument_id


def run_ema(catalog: ParquetDataCatalog, verbose: bool) -> bool:
    bars = catalog.bars()
    if not bars:
        print("ERROR: No bars loaded")
        return False

    engine = build_engine(verbose)
    instrument_id = add_instrument(engine)
    engine.add_data(bars)

    bar_type = BarType.from_str("BTCUSDT-PERP.BINANCE-1-MINUTE-LAST-EXTERNAL")
    strat_config = EMACrossConfig(
        instrument_id=instrument_id,
        bar_type=bar_type,
        trade_size=Decimal("0.001"),
        fast_ema_period=10,
        slow_ema_period=20,
        subscribe_quote_ticks=False,
        subscribe_trade_ticks=False,
        request_bars=False,
        unsubscribe_data_on_stop=True,
        order_quantity_precision=3,
    )
    engine.add_strategy(EMACross(config=strat_config))

    if verbose:
        print("Running EMA cross backtest...")
    engine.run()
    engine.dispose()
    return True


def run_custom(catalog: ParquetDataCatalog, verbose: bool) -> bool:
    try:
        from barter_nautilus_data.registry import register_all_custom_data
        from barter_nautilus_data.extended_bars import ExtendedBar1m
        from barter_nautilus_data.tpo_brackets import TpoBracket
        from barter_nautilus_data.large_trades import LargeTrade
        from barter_nautilus_data.profile_events import ProfileEvent
    except ImportError as e:
        print(f"ERROR: missing barter_nautilus_data: {e}")
        print("Install from barter-rs: uv pip install -e /Users/screener-m3/projects/barter-rs/packages/barter-nautilus-data")
        return False

    # Make types available for CustomDataCounter runtime lookup.
    globals().update(
        {
            "ExtendedBar1m": ExtendedBar1m,
            "TpoBracket": TpoBracket,
            "LargeTrade": LargeTrade,
            "ProfileEvent": ProfileEvent,
        }
    )

    # Register custom data schemas before loading
    register_all_custom_data()

    bars = catalog.bars()
    if not bars:
        print("ERROR: No bars loaded")
        return False

    custom_ext = catalog.custom_data(cls=ExtendedBar1m)
    custom_tpo = catalog.custom_data(cls=TpoBracket)
    custom_large = catalog.custom_data(cls=LargeTrade)
    custom_profile = catalog.custom_data(cls=ProfileEvent)

    engine = build_engine(verbose)
    instrument_id = add_instrument(engine)

    engine.add_data(bars)
    engine.add_data(custom_ext, ClientId("CUSTOM"))
    engine.add_data(custom_tpo, ClientId("CUSTOM"))
    engine.add_data(custom_large, ClientId("CUSTOM"))
    engine.add_data(custom_profile, ClientId("CUSTOM"))

    strat = CustomDataCounter(CustomDataCounterConfig(instrument_id=instrument_id))
    engine.add_strategy(strat)

    if verbose:
        print("Running custom data backtest...")
    engine.run()
    engine.dispose()
    return True


def run_orderbook(catalog: ParquetDataCatalog, verbose: bool) -> bool:
    deltas = catalog.order_book_deltas(batched=True)
    if not deltas:
        print("ERROR: No order book deltas loaded")
        return False

    engine = build_engine(verbose)
    instrument_id = add_instrument(engine)
    engine.add_data(deltas)

    strat = OrderBookDeltaCounter(OrderBookDeltaCounterConfig(instrument_id=instrument_id))
    engine.add_strategy(strat)

    if verbose:
        print("Running order book deltas backtest...")
    engine.run()
    engine.dispose()
    return True


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--catalog-dir", default="/tmp/nautilus_catalog_e2e")
    parser.add_argument("--strategy", choices=["ema", "custom", "orderbook"], default="ema")
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args()

    catalog = ParquetDataCatalog(str(Path(args.catalog_dir)))
    if args.strategy == "ema":
        ok = run_ema(catalog, verbose=not args.quiet)
    elif args.strategy == "custom":
        ok = run_custom(catalog, verbose=not args.quiet)
    else:
        ok = run_orderbook(catalog, verbose=not args.quiet)

    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
