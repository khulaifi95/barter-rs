"""Registration helpers for custom Barter Nautilus data classes."""

from . import extended_bars
from . import large_trades
from . import profile_events
from . import tpo_brackets


def register_all_custom_data() -> None:
    """
    Import all custom data modules to ensure Arrow schemas are registered.

    Call this once on startup before loading Parquet catalogs.
    """
    _ = extended_bars  # noqa: F841 (import for side effects)
    _ = large_trades  # noqa: F841 (import for side effects)
    _ = profile_events  # noqa: F841 (import for side effects)
    _ = tpo_brackets  # noqa: F841 (import for side effects)
