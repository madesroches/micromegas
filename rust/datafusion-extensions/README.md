# micromegas-datafusion-extensions

Apache DataFusion UDFs that also compile to wasm32: JSONB, histograms, properties, colors, binning and math, for the [Micromegas](https://github.com/madesroches/micromegas/) observability platform.

This crate provides shared user-defined functions that work in both native and `wasm32-unknown-unknown` targets, used by `micromegas-analytics` (server-side) and `micromegas-datafusion-wasm` (browser-side).

## Functions

### JSONB
- `jsonb_parse` - JSON string to JSONB binary
- `jsonb_format_json` - JSONB to JSON string
- `jsonb_get` - extract nested value by key
- `jsonb_as_string`, `jsonb_as_i64`, `jsonb_as_f64` - type casts
- `jsonb_object_keys` - extract object keys
- `jsonb_array_length` - number of elements in a JSONB array
- `jsonb_path_query_first`, `jsonb_path_query` - JSONPath queries
- `jsonb_array_elements`, `jsonb_each` (UDTF) - expand a JSONB array or object into rows
- `jsonb_entries`, `jsonb_elements`, `jsonb_path_elements` - expand a JSONB value into an Arrow `List`, for per-row expansion via `unnest()`

### Histogram
- `make_histogram` (UDAF) - create histogram from values
- `sum_histograms` (UDAF) - merge histograms
- `expand_histogram` (UDTF) - histogram to rows of (bin_center, count)
- `quantile_from_histogram`, `variance_from_histogram`, `count_from_histogram`, `sum_from_histogram` - scalar accessors

### Properties
- `property_get`, `properties_to_array`, `properties_length` - access Micromegas property lists

### Color
- `rgba`, `lerp_color`, `color_scale` - build, interpolate and map colors

### Math
- `lerp`, `unlerp` - linear interpolation and its inverse

### Binning
- `bin_center` - center of the bin containing a value

## Usage

```rust
use datafusion::execution::FunctionRegistry;
use datafusion::prelude::SessionContext;

let ctx = SessionContext::new();
micromegas_datafusion_extensions::register_extension_udfs(&ctx);
assert!(ctx.udf("jsonb_parse").is_ok());
```

## Documentation

- [Functions Reference](https://micromegas.info/docs/query-guide/functions-reference/)
- [GitHub Repository](https://github.com/madesroches/micromegas)
