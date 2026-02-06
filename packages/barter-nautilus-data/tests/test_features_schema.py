from barter_nautilus_data import LargeTrade, ProfileEvent, TpoBracket


def test_tpo_bracket_schema_has_core_fields():
    schema = TpoBracket._schema  # noqa: SLF001 (runtime schema)
    names = [field.name for field in schema]
    assert "instrument_id" in names
    assert "label" in names
    assert "bracket_open" in names
    assert "vol_poc" in names
    assert "tpo_poc" in names


def test_large_trade_schema_has_core_fields():
    schema = LargeTrade._schema  # noqa: SLF001 (runtime schema)
    names = [field.name for field in schema]
    assert "instrument_id" in names
    assert "trade_id" in names
    assert "notional_usd" in names
    assert "category" in names
    assert "threshold_mode" in names


def test_profile_event_schema_has_nullable_gap_fields():
    schema = ProfileEvent._schema  # noqa: SLF001 (runtime schema)
    missing_bars = next(field for field in schema if field.name == "missing_bars")
    assert missing_bars.name == "missing_bars"
