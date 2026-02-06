# barter-nautilus-data

Custom NautilusTrader data classes for Barter Parquet datasets.

## Purpose

This package lives inside the `barter-rs` repo so the Parquet schemas stay in
lock-step with collector changes. It is a standalone Python package you install
into your Nautilus environment.

## Install (editable)

```bash
uv pip install -e /Users/screener-m3/projects/barter-rs/packages/barter-nautilus-data
```

## Usage

```python
from barter_nautilus_data import register_all_custom_data

register_all_custom_data()
```

This registers the custom data classes with Nautilus' Arrow serializer so they can
be loaded from Parquet via `ParquetDataCatalog`.

## Code Layout

- `barter_nautilus_data/extended_bars.py`
  - CustomData class for 1m extended bars written by `barter-data-server`.
- `barter_nautilus_data/schemas.py`
  - Fixed-point decoding helpers (1e9) and schema constants.
- `barter_nautilus_data/registry.py`
  - `register_all_custom_data()` for Nautilus integration.
- `barter_nautilus_data/__init__.py`
  - Package exports.

## Extending

To add new feature-layer Parquet types later:
1. Create a new `CustomData` class matching the Parquet schema.
2. Add it to `register_all_custom_data()` in `registry.py`.
3. Keep fixed-point precision aligned with the collector output.
