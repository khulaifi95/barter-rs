"""Custom data class for Barter profile events output."""

from dataclasses import dataclass

from nautilus_trader.core.data import Data
from nautilus_trader.model.custom import customdataclass
from nautilus_trader.model.identifiers import InstrumentId

from .schemas import PRICE_SCALE, decode_fixed_point


@customdataclass
class ProfileEvent(Data):
    """
    Barter profile event output (from features/profile_events parquet).

    Fixed-point fields (int) use 1e9 scale.
    Nullable fields may be None or sentinel values (-1, empty string).
    """

    # Metadata
    schema_version: str
    config_hash: str
    source_precision: str
    output_precision: int

    # Identifiers
    instrument_id: InstrumentId
    session_id: str
    event_type: str

    # Profile state (prices)
    vol_poc: int
    vol_vah: int
    vol_val: int
    tpo_poc: int
    tpo_vah: int
    tpo_val: int
    ib_high: int
    ib_low: int

    # Event-specific (use "" or -1 as sentinel for null)
    break_direction: str  # "" if not applicable
    break_price: int      # -1 if not applicable
    shift_amount: int     # -1 if not applicable
    previous_value: int   # -1 if not applicable

    # Gap-specific (use -1 as sentinel for null)
    gap_start_ts: int     # -1 if not applicable
    gap_end_ts: int       # -1 if not applicable
    missing_bars: int     # -1 if not applicable

    def vol_poc_f(self) -> float:
        return decode_fixed_point(self.vol_poc, PRICE_SCALE)

    def vol_vah_f(self) -> float:
        return decode_fixed_point(self.vol_vah, PRICE_SCALE)

    def vol_val_f(self) -> float:
        return decode_fixed_point(self.vol_val, PRICE_SCALE)

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

    def break_price_f(self) -> float:
        if self.break_price is None or self.break_price == -1:
            return float('nan')
        return decode_fixed_point(self.break_price, PRICE_SCALE)

    def shift_amount_f(self) -> float:
        if self.shift_amount is None or self.shift_amount == -1:
            return float('nan')
        return decode_fixed_point(self.shift_amount, PRICE_SCALE)

    def previous_value_f(self) -> float:
        if self.previous_value is None or self.previous_value == -1:
            return float('nan')
        return decode_fixed_point(self.previous_value, PRICE_SCALE)

    def is_gap_event(self) -> bool:
        return self.gap_start_ts is not None and self.gap_start_ts != -1
