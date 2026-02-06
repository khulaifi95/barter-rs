"""Custom data class for Barter extended 1m bars."""

from dataclasses import dataclass
from typing import Any, Dict

from nautilus_trader.model.custom import customdataclass
from nautilus_trader.core.data import Data
from nautilus_trader.model.identifiers import InstrumentId

from .schemas import PRICE_SCALE, SIZE_SCALE, decode_fixed_point


@customdataclass
class ExtendedBar1m(Data):
    """
    Barter extended 1m bar (from extended_bars_1m parquet).

    Fixed-point fields (int) use 1e9 scale.
    Floating fields are stored as f64 directly.
    """

    instrument_id: InstrumentId
    ts_open: int

    open: int
    high: int
    low: int
    close: int

    volume: int
    quote_volume: int
    trade_count: int
    buy_volume: int
    sell_volume: int
    delta: int
    cvd: int
    open_interest: int
    oi_change: int

    funding_rate: float

    bid_price: int
    bid_size: int
    ask_price: int
    ask_size: int
    spread_bps: float
    book_imbalance: float

    liq_buy_usd: int
    liq_sell_usd: int
    liq_total_usd: int
    liq_count: int

    bid_depth_10bps_base: int
    ask_depth_10bps_base: int
    bid_depth_10bps_usd: int
    ask_depth_10bps_usd: int
    depth_imb_10bps: float

    bid_depth_50bps_base: int
    ask_depth_50bps_base: int
    bid_depth_50bps_usd: int
    ask_depth_50bps_usd: int
    depth_imb_50bps: float

    bid_depth_100bps_base: int
    ask_depth_100bps_base: int
    bid_depth_100bps_usd: int
    ask_depth_100bps_usd: int
    depth_imb_100bps: float

    def open_f(self) -> float:
        return decode_fixed_point(self.open, PRICE_SCALE)

    def high_f(self) -> float:
        return decode_fixed_point(self.high, PRICE_SCALE)

    def low_f(self) -> float:
        return decode_fixed_point(self.low, PRICE_SCALE)

    def close_f(self) -> float:
        return decode_fixed_point(self.close, PRICE_SCALE)

    def volume_f(self) -> float:
        return decode_fixed_point(self.volume, SIZE_SCALE)

    def quote_volume_f(self) -> float:
        return decode_fixed_point(self.quote_volume, SIZE_SCALE)

    def buy_volume_f(self) -> float:
        return decode_fixed_point(self.buy_volume, SIZE_SCALE)

    def sell_volume_f(self) -> float:
        return decode_fixed_point(self.sell_volume, SIZE_SCALE)

    def delta_f(self) -> float:
        return decode_fixed_point(self.delta, SIZE_SCALE)

    def cvd_f(self) -> float:
        return decode_fixed_point(self.cvd, SIZE_SCALE)

    def open_interest_f(self) -> float:
        return decode_fixed_point(self.open_interest, SIZE_SCALE)

    def oi_change_f(self) -> float:
        return decode_fixed_point(self.oi_change, SIZE_SCALE)

    def bid_price_f(self) -> float:
        return decode_fixed_point(self.bid_price, PRICE_SCALE)

    def ask_price_f(self) -> float:
        return decode_fixed_point(self.ask_price, PRICE_SCALE)

    def bid_size_f(self) -> float:
        return decode_fixed_point(self.bid_size, SIZE_SCALE)

    def ask_size_f(self) -> float:
        return decode_fixed_point(self.ask_size, SIZE_SCALE)

    def liq_buy_usd_f(self) -> float:
        return decode_fixed_point(self.liq_buy_usd, SIZE_SCALE)

    def liq_sell_usd_f(self) -> float:
        return decode_fixed_point(self.liq_sell_usd, SIZE_SCALE)

    def liq_total_usd_f(self) -> float:
        return decode_fixed_point(self.liq_total_usd, SIZE_SCALE)

    def depth_snapshot(self) -> dict[str, Any]:
        """Return a compact dict of depth bands in float form."""
        return {
            "bid_10bps_base": decode_fixed_point(self.bid_depth_10bps_base, SIZE_SCALE),
            "ask_10bps_base": decode_fixed_point(self.ask_depth_10bps_base, SIZE_SCALE),
            "bid_10bps_usd": decode_fixed_point(self.bid_depth_10bps_usd, SIZE_SCALE),
            "ask_10bps_usd": decode_fixed_point(self.ask_depth_10bps_usd, SIZE_SCALE),
            "imb_10bps": self.depth_imb_10bps,
            "bid_50bps_base": decode_fixed_point(self.bid_depth_50bps_base, SIZE_SCALE),
            "ask_50bps_base": decode_fixed_point(self.ask_depth_50bps_base, SIZE_SCALE),
            "bid_50bps_usd": decode_fixed_point(self.bid_depth_50bps_usd, SIZE_SCALE),
            "ask_50bps_usd": decode_fixed_point(self.ask_depth_50bps_usd, SIZE_SCALE),
            "imb_50bps": self.depth_imb_50bps,
            "bid_100bps_base": decode_fixed_point(self.bid_depth_100bps_base, SIZE_SCALE),
            "ask_100bps_base": decode_fixed_point(self.ask_depth_100bps_base, SIZE_SCALE),
            "bid_100bps_usd": decode_fixed_point(self.bid_depth_100bps_usd, SIZE_SCALE),
            "ask_100bps_usd": decode_fixed_point(self.ask_depth_100bps_usd, SIZE_SCALE),
            "imb_100bps": self.depth_imb_100bps,
        }
