from barter_nautilus_data import ExtendedBar1m


def test_extended_bars_schema_has_core_fields():
    schema = ExtendedBar1m._schema  # noqa: SLF001 (runtime schema)
    names = [f.name for f in schema]
    assert "instrument_id" in names
    assert "ts_event" in names
    assert "ts_init" in names
    assert "open" in names
    assert "close" in names
