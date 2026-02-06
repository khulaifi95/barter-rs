"""Shared schema helpers for Barter custom data classes."""

PRICE_SCALE = 1_000_000_000  # 1e9 fixed-point (matches extended_bars_1m encoding)
SIZE_SCALE = 1_000_000_000
NOTIONAL_SCALE = 1_000_000_000


def decode_fixed_point(value: int, scale: int = PRICE_SCALE) -> float:
    """Decode fixed-point integer to float."""
    return value / float(scale)
