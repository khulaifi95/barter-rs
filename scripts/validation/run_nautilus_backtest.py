#!/usr/bin/env python3
"""
End-to-end Nautilus Backtest Test

This script runs a minimal backtest using Barter-generated Parquet data
to verify the full integration pipeline works.

Usage:
    python scripts/validation/run_nautilus_backtest.py [--catalog-dir /path/to/catalog]

Prerequisites:
    uv pip install nautilus_trader pyarrow

    # Generate test data first:
    cargo run -p barter-data-server --bin generate-sample-parquet
    python scripts/validation/setup_nautilus_catalog.py
"""

import argparse
import sys
from decimal import Decimal
from pathlib import Path

try:
    from nautilus_trader.backtest.engine import BacktestEngine, BacktestEngineConfig
    from nautilus_trader.config import LoggingConfig
    from nautilus_trader.model.currencies import USD, USDT
    from nautilus_trader.model.data import BarType
    from nautilus_trader.model.enums import AccountType, OmsType, AssetClass
    from nautilus_trader.model.identifiers import TraderId, Venue, InstrumentId, Symbol
    from nautilus_trader.model.instruments import CryptoPerpetual
    from nautilus_trader.model.objects import Money, Price, Quantity, Currency
    from nautilus_trader.persistence.catalog import ParquetDataCatalog
    from nautilus_trader.trading.strategy import Strategy, StrategyConfig
except ImportError as e:
    print(f"ERROR: nautilus_trader not installed or missing dependency: {e}")
    print("Run: uv pip install nautilus_trader")
    sys.exit(1)


class SimpleTestStrategyConfig(StrategyConfig, frozen=True):
    """Configuration for test strategy."""
    bar_type: str = "BTCUSDT-PERP.BINANCE-1-MINUTE-LAST-EXTERNAL"


class SimpleTestStrategy(Strategy):
    """
    A minimal test strategy that just logs bar events.
    Used to verify data flows through the backtest engine.
    """

    def __init__(self, config: SimpleTestStrategyConfig):
        super().__init__(config)
        self.bar_type = BarType.from_str(config.bar_type)
        self.bar_count = 0

    def on_start(self):
        """Called when strategy starts."""
        self.subscribe_bars(self.bar_type)
        self.log.info(f"Strategy started, subscribed to {self.bar_type}")

    def on_bar(self, bar):
        """Called on each bar."""
        self.bar_count += 1
        if self.bar_count <= 3 or self.bar_count % 5 == 0:
            self.log.info(f"Bar {self.bar_count}: {bar}")

    def on_stop(self):
        """Called when strategy stops."""
        self.log.info(f"Strategy stopped. Processed {self.bar_count} bars")


def run_backtest(catalog_dir: Path, verbose: bool = True):
    """Run a minimal backtest with the test data."""

    if verbose:
        print("=" * 60)
        print("Nautilus End-to-End Backtest Test")
        print("=" * 60)

    # Load catalog
    if verbose:
        print(f"\nLoading catalog from: {catalog_dir}")

    catalog = ParquetDataCatalog(str(catalog_dir))

    # Check available data
    data_types = catalog.list_data_types()
    if verbose:
        print(f"Available data types: {data_types}")

    if "bar" not in data_types:
        print("ERROR: No bar data in catalog")
        return False

    # Load bars to get time range
    bars = catalog.bars()
    if not bars:
        print("ERROR: No bars loaded from catalog")
        return False

    if verbose:
        print(f"Loaded {len(bars)} bars")
        print(f"  First: {bars[0]}")
        print(f"  Last: {bars[-1]}")

    # Get time range from bars
    start = bars[0].ts_event
    end = bars[-1].ts_event

    # Configure backtest engine
    engine_config = BacktestEngineConfig(
        trader_id=TraderId("TESTER-001"),
        logging=LoggingConfig(log_level="INFO" if verbose else "WARNING"),
    )

    # Create engine
    engine = BacktestEngine(config=engine_config)

    # Add venue
    BINANCE = Venue("BINANCE")
    engine.add_venue(
        venue=BINANCE,
        oms_type=OmsType.NETTING,
        account_type=AccountType.MARGIN,
        base_currency=USD,
        starting_balances=[Money(100_000, USD)],
    )

    # Create instrument definition for BTCUSDT perpetual
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

    # Add instrument
    engine.add_instrument(btcusdt_perp)
    if verbose:
        print(f"Added instrument: {instrument_id}")

    # Add data - use the bars from catalog
    engine.add_data(bars)

    # Add strategy
    strategy_config = SimpleTestStrategyConfig(
        bar_type="BTCUSDT-PERP.BINANCE-1-MINUTE-LAST-EXTERNAL"
    )
    strategy = SimpleTestStrategy(config=strategy_config)
    engine.add_strategy(strategy)

    # Run backtest
    if verbose:
        print(f"\nRunning backtest...")
        print(f"  Time range: {start} to {end}")

    engine.run()

    # Get results
    if verbose:
        print(f"\n=== Backtest Results ===")
        print(f"  Bars processed: {strategy.bar_count}")

    # Cleanup
    engine.dispose()

    # Verify
    success = strategy.bar_count > 0
    if verbose:
        if success:
            print(f"\n✓ Backtest PASSED - processed {strategy.bar_count} bars")
        else:
            print(f"\n✗ Backtest FAILED - no bars processed")

    return success


def main():
    parser = argparse.ArgumentParser(description="Run Nautilus backtest with Barter data")
    parser.add_argument("--catalog-dir", type=Path, default=Path("/tmp/nautilus_catalog"),
                       help="Path to Nautilus catalog directory")
    parser.add_argument("--quiet", action="store_true",
                       help="Suppress verbose output")

    args = parser.parse_args()
    verbose = not args.quiet

    if not args.catalog_dir.exists():
        print(f"ERROR: Catalog directory not found: {args.catalog_dir}")
        print("Run setup_nautilus_catalog.py first")
        sys.exit(1)

    success = run_backtest(args.catalog_dir, verbose)
    sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()
