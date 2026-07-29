# OTLP Summary Metrics → `measures` Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1359

## Overview

CloudWatch Metric Streams configured for `opentelemetry1.0` output encode every data point as
an OTLP `Summary`, which `OtelMetricsBlockProcessor` currently drops with a `debug!` log. The
route added in #1299/#1300 therefore accepts CloudWatch metric payloads, acks 200, and writes
zero rows to `measures` — silently. This plan makes `Summary` materialize: each
`SummaryDataPoint` fans out into exactly four `measures` rows — count, sum, min, max — tagged
via `properties`, and bumps `SCHEMA_VERSION` so already-ingested (and previously dropped)
Summary blocks re-materialize on next partition rebuild.

Scope is deliberately narrow: only the four fixed statistics land. Any additional
`quantile_values` entries beyond `q=0.0`/`q=1.0` (i.e. configured percentiles like p90/p99) are
dropped with a `debug!` log, same treatment as Histogram/ExponentialHistogram — count/sum/min/max
is enough for now, and arbitrary percentile fan-out can be revisited later if needed.

Histogram and ExponentialHistogram remain out of scope and continue to be dropped — a
histogram-aware schema would need pre-aggregated bucket rows to avoid one row per bucket per
scrape, and that's a larger, separate design (already flagged as future work in the docs).
Summary is different: CloudWatch always emits it, and count/sum/min/max decomposes into a
small, fixed set of scalar statistics, so it fits the existing one-row-per-scalar `measures`
shape directly.

## Current State

### Where Summary is dropped

`rust/analytics/src/lakehouse/otel/metrics_block_processor.rs:121-128` — the `Data::Summary`
arm only logs and drops:

```rust
Some(Data::Summary(s)) => {
    debug!(
        "OTel summary dropped (deprecated in OTel): name={} unit={} points={}",
        metric.name, metric.unit, s.data_points.len()
    );
}
```

`Data::Sum` (:75-95) and `Data::Gauge` (:96-104) are the only arms that call
`builder.append(&scope_name, &metric.name, &metric.unit, dp, &extras)`, where `dp` is a
`NumberDataPoint` — one `f64` per row via `MeasuresRowBuilder::append`
(:199-244), which pulls `dp.value` (`AsDouble`/`AsInt`), stamps `dp.time_unix_nano`, encodes
`dp.attributes` + `extras` into `properties` via `attrs_to_jsonb`, and appends one row across
all `measures` columns.

### `SummaryDataPoint` shape (`opentelemetry-proto` v0.32, `metrics.v1.rs:656-706`)

```rust
pub struct SummaryDataPoint {
    pub attributes: Vec<KeyValue>,       // tag 7
    pub start_time_unix_nano: u64,       // tag 2
    pub time_unix_nano: u64,             // tag 3
    pub count: u64,                      // tag 4 — SampleCount
    pub sum: f64,                        // tag 5 — Sum
    pub quantile_values: Vec<summary_data_point::ValueAtQuantile>, // tag 6
    pub flags: u32,                      // tag 8
}
// summary_data_point::ValueAtQuantile { quantile: f64, value: f64 }
```

Per the proto's documented convention (also quoted in the issue): `quantile == 0.0` is the
observed minimum, `quantile == 1.0` is the observed maximum. CloudWatch's default Metric
Stream output always includes `count`, `sum`, and the `q=0.0`/`q=1.0` pair; a
`statistics_configuration` on the stream adds extra `ValueAtQuantile` entries for configured
percentiles (e.g. p90, p99) — this plan drops those (see Design), covering only the four
default statistics.

### Why the current `append` can't take a `SummaryDataPoint`

`MeasuresRowBuilder::append` (:199-244) is typed to `NumberDataPoint` and pulls exactly one
`value` out of it. A `SummaryDataPoint` has no single value — it carries 2 + N numbers (count,
sum, N quantiles). The fix needs a builder entry point keyed by an already-extracted
`(time_nanos, attributes, value)` triple, callable once per statistic.

### `measures` schema and versioning (unchanged by this plan)

`rust/analytics/src/metrics_table.rs:18-87` — `measures` is scalar: one `value: Float64` per
row, no schema change needed here (rules out issue Option 2, "widen with count/sum/min/max
columns", which would touch every metrics consumer and put nulls on every Sum/Gauge row —
rejected per the "why this isn't just a missing match arm" framing in the issue).

`rust/analytics/src/lakehouse/metrics_view.rs:39` — `const SCHEMA_VERSION: u8 = 6;`, referenced
by both `MetricsViewMaker::get_schema_hash` (:66-68) and `MetricsView::get_file_schema_hash`
(:139-141). `verify_overlapping_partitions` matches existing partitions against this hash, so
bumping it excludes every existing `measures` partition from future queries and makes any range
that gets (re-)materialized pick up the fix — which is exactly what's needed here, since the
pre-fix backlog's raw `ResourceMetrics` bytes are already durably stored in lake blocks (only the
*materialized* rows were dropped). This is **not** an automatic backfill, though: `measures` is
the "global" view instance, whose `MetricsView::jit_update` is a no-op (`metrics_view.rs:152-156`,
"this view instance is updated using the deamon") — only the daemon's trailing-window tasks
(`EveryDayTask`/`EveryHourTask`/`EveryMinuteTask`/`EverySecondTask` in
`rust/public/src/servers/maintenance.rs`) re-materialize `measures`, and they only cover a
short recent window (e.g. the day task is now − 2 days to now). Older ranges — including the
CloudWatch backlog, which likely predates that window — stay invisible under the new hash until
someone runs `regenerate_partitions('measures', <begin>, <end>, <delta>)` for the desired
historical range (optionally followed by `retire_partitions` to drop the stale rows), same as
the precedent in `tasks/completed/dictionary_key_overflow_plan.md` (PR #521, PR #934). That
backfill is a deploy-time operational step, not part of this plan's code change. Per the issue's
"Backfill consequence" section: this plan ships **no parquet schema change**, so the version
bump must be deliberate, not incidental.

### Docs to update

`mkdocs/docs/otlp/index.md:144` (Metrics → `measures` mapping table intro) and `:540`
("Limitations" bullet) both currently state Summary is skipped with a debug log.

## Design

### Fan out to four fixed statistics (issue Option 1, narrowed)

Each `SummaryDataPoint` becomes exactly four rows — count, sum, min, max — all sharing the
metric's `name`/`time` and the data point's own `attributes`, distinguished by two new
`properties` keys:

| `properties` key | Value |
|---|---|
| `otel.metric.kind` | `"summary"` (same slot Sum/Gauge already populate with `"sum"`/`"gauge"`) |
| `otel.metric.statistic` | `"count"` \| `"sum"` \| `"min"` \| `"max"` |

Statistic tagging (not a name suffix) was chosen so `name` stays a clean grouping key across
all metric kinds — see Trade-offs.

Mapping from `SummaryDataPoint` fields to rows:

- `count` → one row, `value = count as f64`, `statistic = "count"`, `unit = ""` (a sample count
  isn't a quantity in the metric's unit — see Trade-offs).
- `sum` → one row, `value = sum`, `statistic = "sum"`, `unit = metric.unit`.
- `quantile_values` entry with `quantile == 0.0` → one row, `statistic = "min"`,
  `unit = metric.unit`.
- `quantile_values` entry with `quantile == 1.0` → one row, `statistic = "max"`,
  `unit = metric.unit`.
- any other `quantile_values` entry (configured percentiles, e.g. p90/p99) → dropped with a
  `debug!` log naming the metric and the quantile — same "logged, not silent-in-aggregate"
  treatment as Histogram/ExponentialHistogram. Not an error: the four fixed statistics still
  materialize from the same data point.

A CloudWatch Metric Stream — with or without `statistics_configuration` — therefore always
produces exactly 4 rows per scrape per metric (assuming `q=0.0`/`q=1.0` are present, which
CloudWatch's default output guarantees). If a `SummaryDataPoint` is missing the `q=0.0` or
`q=1.0` entry (non-CloudWatch producer), that statistic is simply absent for that row — no
error, fewer than 4 rows.

`time_unix_nano == 0` skips the whole data point (all its rows), same short-circuit the
existing `append` uses for Sum/Gauge — one point, one timestamp, one skip check.

### Builder refactor: split value-extraction from row-append

`MeasuresRowBuilder::append` currently does two things in one method: pull `(time_nanos,
value)` out of a `NumberDataPoint`, then append a row. Split those so the new Summary path can
supply an already-known `value` without a `NumberDataPoint` to unwrap:

```rust
/// Appends one `measures` row for an already-extracted point. Shared tail of
/// `append` (Sum/Gauge) and `append_summary_stat` (Summary).
fn append_row(
    &mut self,
    scope_name: &str,
    metric_name: &str,
    unit: &str,
    time_nanos: i64,
    attributes: &[KeyValue],
    value: f64,
    extras: &[(String, JsonbValue<'static>)],
) -> Result<()> {
    // body = current `append` from `self.min_time = ...` (line 222) through
    // `self.nb_appended += 1` (line 242), reading `attributes`/`value`/`time_nanos`
    // params instead of `dp.attributes`/`value`/`time_nanos`.
}

fn append(
    &mut self,
    scope_name: &str,
    metric_name: &str,
    unit: &str,
    dp: &NumberDataPoint,
    extras: &[(String, JsonbValue<'static>)],
) -> Result<()> {
    let time_nanos = dp.time_unix_nano as i64;
    if time_nanos == 0 {
        debug!("OTel metric data point for {metric_name} dropped (time_unix_nano=0)");
        return Ok(());
    }
    let value = match dp.value.as_ref() {
        Some(number_data_point::Value::AsDouble(d)) => *d,
        Some(number_data_point::Value::AsInt(i)) => *i as f64,
        None => {
            debug!("OTel data point for {metric_name} has no value, skipping");
            return Ok(());
        }
    };
    self.append_row(scope_name, metric_name, unit, time_nanos, &dp.attributes, value, extras)
}

/// Fans a `SummaryDataPoint` out into rows for the four fixed statistics
/// (count, sum, min, max). Any `quantile_values` entry other than `q=0.0`/
/// `q=1.0` is logged and dropped — configured percentiles are out of scope.
/// `kind` is the shared `("otel.metric.kind", "summary")` pair; `statistic`
/// is layered on per row.
fn append_summary(
    &mut self,
    scope_name: &str,
    metric_name: &str,
    unit: &str,
    dp: &SummaryDataPoint,
) -> Result<()> {
    let time_nanos = dp.time_unix_nano as i64;
    if time_nanos == 0 {
        debug!("OTel summary data point for {metric_name} dropped (time_unix_nano=0)");
        return Ok(());
    }
    let kind = ("otel.metric.kind".to_string(), JsonbValue::String(Cow::Borrowed("summary")));

    let stat = |s: &'static str| {
        [kind.clone(), ("otel.metric.statistic".to_string(), JsonbValue::String(Cow::Borrowed(s)))]
    };
    self.append_row(scope_name, metric_name, "", time_nanos, &dp.attributes, dp.count as f64, &stat("count"))?;
    self.append_row(scope_name, metric_name, unit, time_nanos, &dp.attributes, dp.sum, &stat("sum"))?;

    for q in &dp.quantile_values {
        if q.quantile == 0.0 {
            self.append_row(scope_name, metric_name, unit, time_nanos, &dp.attributes, q.value, &stat("min"))?;
        } else if q.quantile == 1.0 {
            self.append_row(scope_name, metric_name, unit, time_nanos, &dp.attributes, q.value, &stat("max"))?;
        } else {
            debug!(
                "OTel summary quantile dropped (only count/sum/min/max are materialized): \
                 name={metric_name} quantile={}",
                q.quantile
            );
        }
    }
    Ok(())
}
```

(Exact signatures/closures above are illustrative — implement with whatever's most idiomatic;
the field-by-field mapping and the `append_row` extraction are the load-bearing parts.)

### `OtelMetricsBlockProcessor::process` — new `Data::Summary` arm

Replace the current drop-and-log arm (:121-128) with:

```rust
Some(Data::Summary(s)) => {
    for dp in &s.data_points {
        builder.append_summary(&scope_name, &metric.name, &metric.unit, dp)?;
    }
}
```

Histogram/ExponentialHistogram arms (:105-119) are unchanged — still dropped with `debug!`,
per the explicit decision that histogram support would need bucket-level rows and is out of
scope here.

Update the module doc comment (`metrics_block_processor.rs:1-5`) to drop "and Summary" from the
"deferred to v2" list and describe the new count/sum/min/max fan-out instead (noting that
non-min/max quantiles are still dropped).

### `SCHEMA_VERSION` bump

`rust/analytics/src/lakehouse/metrics_view.rs:39` — bump `SCHEMA_VERSION` from `6` to `7`. No
field in `metrics_table_schema()` changes; the bump exists purely to invalidate existing
`measures` partitions so blocks containing Summary data points (previously materialized as
zero rows) are rebuilt from the still-retained source blocks and their rows finally appear.

## Implementation Steps

1. **Builder refactor** — in `rust/analytics/src/lakehouse/otel/metrics_block_processor.rs`,
   extract `append_row` from `MeasuresRowBuilder::append`, keeping `append`'s public behavior
   identical (verify via the existing dictionary-overflow test still passing unchanged).
2. **`append_summary`** — add the new method per Design, using `s.data_points` iteration inside
   `process`'s `Data::Summary` arm.
3. **Wire the new arm** — replace the `Data::Summary` drop arm in `process` with the fan-out
   call; update the module doc comment at the top of the file.
4. **Bump `SCHEMA_VERSION`** — `6` → `7` in `rust/analytics/src/lakehouse/metrics_view.rs`; update
   the `metrics_processors()` doc comment at `metrics_view.rs:44` (currently "Sum/Gauge" only) to
   mention Summary alongside them.
5. **Update `otel-ingestion` doc comment** — `rust/otel-ingestion/src/block.rs:101-104`'s
   `metrics_bounds()` comment currently says Summary points are skipped; update it to say
   Histogram/ExponentialHistogram are dropped by the materialization processor while Summary now
   fans out count/sum/min/max (only non-min/max quantiles are still dropped).
6. **Unit tests** — extend `rust/analytics/tests/` (see Testing) with a Summary-specific test
   alongside the existing `otel_metrics_block_processor_survives_target_dictionary_overflow`
   pattern in `dictionary_key_overflow_tests.rs`, plus a focused fan-out test (new file or
   alongside `otel_attrs_tests.rs`).
7. **Docs** — update `mkdocs/docs/otlp/index.md:144` and `:540` (see Documentation).
8. **CI** — `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`, then
   `python3 build/rust_ci.py` (from `rust/`).

## Files to Modify

- `rust/analytics/src/lakehouse/otel/metrics_block_processor.rs` — `append_row` extraction,
  `append_summary`, new `Data::Summary` arm, module doc comment.
- `rust/analytics/src/lakehouse/metrics_view.rs` — `SCHEMA_VERSION` `6` → `7`; update the
  `metrics_processors()` doc comment at `:44` to mention Summary alongside Sum/Gauge.
- `rust/otel-ingestion/src/block.rs` — update the `metrics_bounds()` doc comment at `:101-104`
  (currently says Summary is skipped) to reflect that Summary now fans out count/sum/min/max.
- `rust/analytics/tests/dictionary_key_overflow_tests.rs` or a new test file — Summary fan-out
  coverage.
- `mkdocs/docs/otlp/index.md` — Metrics mapping section (:144) and Limitations bullet (:540).

## Trade-offs

- **Tag statistic in `properties` vs. suffix the metric `name`.** Tagging keeps `name` a stable
  grouping key identical across Sum/Gauge/Summary — a dashboard already filtering
  `WHERE name = 'CPUUtilization'` gets all statistics back and filters further on
  `otel.metric.statistic`, rather than needing to know about `CPUUtilization.max` /
  `CPUUtilization_max` naming. Cost: any query that doesn't filter on `otel.metric.statistic`
  now gets 4 rows per timestamp per metric instead of 1 — must be called out prominently in
  docs (done below).
- **Only count/sum/min/max; configured percentiles dropped.** CloudWatch's
  `statistics_configuration` can add arbitrary extra `ValueAtQuantile` entries (p90, p99, ...).
  Fanning those out too was considered but deferred — count/sum/min/max covers the default
  CloudWatch output and the acceptance criteria as given; arbitrary percentile fan-out is easy
  to add later (same `append_row` shape, just another `statistic` tag) if a concrete need shows
  up. Dropped quantiles are logged at `debug!`, not silently discarded, consistent with the
  Histogram/ExponentialHistogram treatment.
- **Fan-out (Option 1) vs. widened schema (Option 2) vs. histogram-aware table (Option 3).**
  Fan-out needs no schema change and reuses `measures` as-is; Option 2 would add 4 mostly-null
  columns to every Sum/Gauge row and touch every metrics consumer; Option 3 (a proper
  histogram/summary-aware view) is real future work for Histogram/ExponentialHistogram, which
  don't fit the scalar-row shape at all (a histogram has O(buckets) numbers, not O(1)) — solving
  that is out of scope for unblocking CloudWatch, which only ever emits Summary.
- **`count`'s `unit` is `""`, not the metric's unit.** A sample count is dimensionless; carrying
  the metric's unit (e.g. `"ms"`) on the count row would misleadingly imply the count itself is
  measured in milliseconds. `sum`/`min`/`max`/`quantile` keep the metric's unit since they're
  genuine values on that scale.
- **Deliberate `SCHEMA_VERSION` bump despite no schema change.** Ships no parquet column change,
  but the issue explicitly flags that skipping the bump would leave the pre-fix backlog
  permanently unrecoverable even though the raw bytes are retained. Bumping is a one-line,
  low-risk way to make historical Summary blocks *eligible* for backfill — actually backfilling
  the CloudWatch backlog still requires the explicit `regenerate_partitions` deploy-time step
  described in Current State.
- **No special-casing for `count == 0` (where `sum` must be zero per spec).** Emitting the
  `sum` row unconditionally (value 0.0) is simpler than adding a conditional and matches
  "SampleCount/Sum/Min/Max each reachable from SQL" from the issue's acceptance criteria without
  exceptions.

## Documentation

`mkdocs/docs/otlp/index.md`:
- **:144** (Metrics → `measures` mapping) — replace "Sum and Gauge data points are materialized
  directly. Histogram, ExponentialHistogram, and Summary are skipped..." with: Sum, Gauge, and
  Summary (count/sum/min/max only) are materialized, via `otel.metric.kind = "summary"` +
  `otel.metric.statistic`; Histogram/ExponentialHistogram, and any Summary quantile other than
  min/max, remain skipped with a debug log (bucket-level/arbitrary-percentile data doesn't fit
  a scalar `value` column without further design). Add the `otel.metric.statistic` values to
  the field-mapping table.
- **:540** (Limitations) — update the bullet to reflect Summary now materializing
  count/sum/min/max; keep the Histogram/ExponentialHistogram limitation and add that configured
  percentile statistics beyond min/max are not materialized.
- Add a short "Selecting one CloudWatch statistic" example under the Metrics section:
  ```sql
  SELECT time, value
  FROM measures
  WHERE name = 'CPUUtilization'
    AND jsonb_as_string(jsonb_get(properties, 'otel.metric.statistic')) = 'max'
  ```
- In the existing "CloudWatch Metric Streams" section (`:373+`), add a line noting that
  `opentelemetry1.0` output is Summary-only and that each scrape now lands as 4 `measures` rows
  per metric (count/sum/min/max), not 1 — and that any additional configured percentile
  statistics are not materialized.

## Testing Strategy

- **Unit — fan-out shape** (new test, e.g. `rust/analytics/tests/otel_metrics_summary_tests.rs`
  or alongside `dictionary_key_overflow_tests.rs`): build a `ResourceMetrics` with one
  `Metric { data: Some(Data::Summary(...)) }` containing `count`, `sum`, and
  `quantile_values = [{0.0, min}, {1.0, max}, {0.9, p90}]`; run it through
  `OtelMetricsBlockProcessor::process`; assert the resulting `RecordBatch` has exactly 4 rows
  (the `0.9` quantile is dropped), each sharing `name`/`time`, and that `properties` decodes to
  the expected `otel.metric.statistic` (`"count"`, `"sum"`, `"min"`, `"max"`). Assert `value`
  matches `count`/`sum`/min/max respectively, and that the `count` row's `unit` is empty while
  the others keep the metric's unit.
- **Unit — non-min/max quantile dropped**: a `SummaryDataPoint` with only a `quantile_values`
  entry at `q=0.5` (no `q=0.0`/`q=1.0`) still produces `count`+`sum` rows, drops the `0.5`
  entry, and logs at `debug!` — mirrors the Histogram/ExponentialHistogram drop convention.
- **Unit — dictionary overflow regression**: extend
  `otel_metrics_block_processor_survives_target_dictionary_overflow` in
  `dictionary_key_overflow_tests.rs` (or add a sibling test) with a Summary variant to confirm
  the refactored `append_row` still handles the `Int32` dictionary path at scale, mirroring the
  existing Gauge coverage.
- **Unit — zero-timestamp skip**: a `SummaryDataPoint` with `time_unix_nano == 0` produces zero
  rows (mirrors the existing Sum/Gauge behavior and the debug-log convention).
- **Existing tests**: confirm `otel_metrics_block_processor_survives_target_dictionary_overflow`
  (Gauge path) still passes unchanged after the `append`/`append_row` split — it's the
  regression guard that the refactor didn't change Sum/Gauge behavior.
- **CI**: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`,
  `python3 build/rust_ci.py`.

## Open Questions

None blocking. Noted for awareness:
- **Configured percentiles (p90/p99/...) are silently out of `measures` today, and stay that
  way after this fix** — dropped at `debug!`, same as before. If a concrete need for querying
  those percentiles shows up, revisit as a follow-up (same `append_row` mechanism, one more
  `statistic` value).
