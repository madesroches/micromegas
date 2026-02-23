# Micromegas DataFusion Extensions Crate

WASM-compatible DataFusion UDF extensions (JSONB, histogram) for the [Micromegas](https://github.com/madesroches/micromegas/) observability platform.

This crate provides shared user-defined functions that work in both native and `wasm32-unknown-unknown` targets, used by `micromegas-analytics` (server-side) and `micromegas-datafusion-wasm` (browser-side).

## Functions

### JSONB
- `jsonb_parse` - JSON string to JSONB binary
- `jsonb_format_json` - JSONB to JSON string
- `jsonb_get` - extract nested value by key
- `jsonb_as_string`, `jsonb_as_i64`, `jsonb_as_f64` - type casts
- `jsonb_object_keys` - extract object keys

### Histogram
- `make_histogram` (UDAF) - create histogram from values
- `sum_histograms` (UDAF) - merge histograms
- `expand_histogram` (UDTF) - histogram to rows of (bin_center, count)
- `quantile_from_histogram`, `variance_from_histogram`, `count_from_histogram`, `sum_from_histogram` - scalar accessors

## Documentation

- [Home Page](https://madesroches.github.io/micromegas/)
- [GitHub Repository](https://github.com/madesroches/micromegas)
