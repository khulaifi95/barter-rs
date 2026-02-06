"""Custom data class for Barter TPO bracket output."""

from dataclasses import dataclass

from nautilus_trader.core.data import Data
from nautilus_trader.model.custom import customdataclass
from nautilus_trader.model.identifiers import InstrumentId

from .schemas import PRICE_SCALE, SIZE_SCALE, decode_fixed_point


@customdataclass
class TpoBracket(Data):
    """
    Barter TPO bracket output (from features/tpo_brackets parquet).

    Fixed-point fields (int) use 1e9 scale.
    """

    # Metadata
    schema_version: str
    config_hash: str
    source_precision: str
    output_precision: int

    # Identifiers
    instrument_id: InstrumentId
    session_id: str
    session_date: str

    # Bracket info
    label: str
    bracket_index: int
    bracket_start_ts: int
    bracket_end_ts: int

    # OHLCV
    bracket_open: int
    bracket_high: int
    bracket_low: int
    bracket_close: int
    bracket_volume: int
    bracket_buy_volume: int
    bracket_sell_volume: int
    bracket_delta: int
    bracket_trade_count: int
    bracket_bar_count: int
    bracket_has_gap: bool

    # Volume Profile (prices + volume)
    vol_poc: int
    vol_vah: int
    vol_val: int
    vol_total: int

    # TPO Profile (prices)
    tpo_poc: int
    tpo_vah: int
    tpo_val: int
    tpo_total_periods: int

    # Initial Balance (prices)
    ib_high: int
    ib_low: int
    ib_complete: bool

    # Analysis
    poc_divergence: int
    session_type: str

    def bracket_open_f(self) -> float:
        return decode_fixed_point(self.bracket_open, PRICE_SCALE)

    def bracket_high_f(self) -> float:
        return decode_fixed_point(self.bracket_high, PRICE_SCALE)

    def bracket_low_f(self) -> float:
        return decode_fixed_point(self.bracket_low, PRICE_SCALE)

    def bracket_close_f(self) -> float:
        return decode_fixed_point(self.bracket_close, PRICE_SCALE)

    def bracket_volume_f(self) -> float:
        return decode_fixed_point(self.bracket_volume, SIZE_SCALE)

    def bracket_buy_volume_f(self) -> float:
        return decode_fixed_point(self.bracket_buy_volume, SIZE_SCALE)

    def bracket_sell_volume_f(self) -> float:
        return decode_fixed_point(self.bracket_sell_volume, SIZE_SCALE)

    def bracket_delta_f(self) -> float:
        return decode_fixed_point(self.bracket_delta, SIZE_SCALE)

    def vol_poc_f(self) -> float:
        return decode_fixed_point(self.vol_poc, PRICE_SCALE)

    def vol_vah_f(self) -> float:
        return decode_fixed_point(self.vol_vah, PRICE_SCALE)

    def vol_val_f(self) -> float:
        return decode_fixed_point(self.vol_val, PRICE_SCALE)

    def vol_total_f(self) -> float:
        return decode_fixed_point(self.vol_total, SIZE_SCALE)

    def tpo_poc_f(self) -> float:
        return decode_fixed_point(self.tpo_poc, PRICE_SCALE)

    def tpo_vah_f(self) -> float:
        return decode_fixed_point(self.tpo_vah, PRICE_SCALE)

    def tpo_val_f(self) -> float:
        return decode_fixed_point(self.tpo_val, PRICE_SCALE)

    def ib_high_f(self) -> float:
        return decode_fixed_point(self.ib_high, PRICE_SCALE)

    def ib_low_f(self) -> float:
        return decode_fixed_point(self.ib_low, PRICE_SCALE)

    def poc_divergence_f(self) -> float:
        return decode_fixed_point(self.poc_divergence, PRICE_SCALE)
