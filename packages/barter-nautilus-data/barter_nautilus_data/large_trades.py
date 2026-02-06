"""Custom data class for Barter large trade output."""

from dataclasses import dataclass

from nautilus_trader.core.data import Data
from nautilus_trader.model.custom import customdataclass
from nautilus_trader.model.identifiers import InstrumentId

from .schemas import PRICE_SCALE, SIZE_SCALE, NOTIONAL_SCALE, decode_fixed_point


@customdataclass
class LargeTrade(Data):
    """
    Barter large trade output (from features/large_trades parquet).

    All fixed-point fields use 1e9 scale.
    """

    # Metadata
    schema_version: str
    config_hash: str
    source_precision: str
    output_precision: int

    # Identifiers
    instrument_id: InstrumentId
    trade_id: str

    # Trade data
    price: int
    size: int
    side: str
    notional_usd: int
    category: str
    threshold_used: int
    threshold_mode: str

    def price_f(self) -> float:
        return decode_fixed_point(self.price, PRICE_SCALE)

    def size_f(self) -> float:
        return decode_fixed_point(self.size, SIZE_SCALE)

    def notional_usd_f(self) -> float:
        return decode_fixed_point(self.notional_usd, NOTIONAL_SCALE)

    def threshold_used_f(self) -> float:
        return decode_fixed_point(self.threshold_used, NOTIONAL_SCALE)
