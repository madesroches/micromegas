use datafusion::arrow::array::{Array, Float64Array, StructArray, UInt64Array};
use datafusion::prelude::{SessionConfig, SessionContext};
use micromegas_datafusion_extensions::register_extension_udfs;

async fn make_ctx() -> SessionContext {
    let ctx = SessionContext::new();
    register_extension_udfs(&ctx);
    ctx
}

async fn make_ctx_batch1() -> SessionContext {
    let config = SessionConfig::new().with_batch_size(1);
    let ctx = SessionContext::new_with_config(config);
    register_extension_udfs(&ctx);
    ctx
}

async fn run_sql(ctx: &SessionContext, sql: &str) -> datafusion::arrow::record_batch::RecordBatch {
    let df = ctx.sql(sql).await.expect("sql parse failed");
    let batches = df.collect().await.expect("collect failed");
    assert!(!batches.is_empty(), "query returned no batches");
    batches.into_iter().next().expect("no batches")
}

// ── Test 1: Runtime bounds happy path ──────────────────────────────────────

#[tokio::test]
async fn test_runtime_bounds_happy_path() {
    let ctx = make_ctx().await;
    ctx.sql("CREATE TABLE vals AS SELECT unnest(range(1, 11)) AS v")
        .await
        .expect("create table failed")
        .collect()
        .await
        .expect("collect failed");

    // lo=1, hi=10, nb_bins=5 via CTE + CROSS JOIN
    let batch = run_sql(
        &ctx,
        "WITH bounds AS (SELECT CAST(MIN(v) AS DOUBLE) AS lo,
                                CAST(MAX(v) AS DOUBLE) AS hi
                         FROM vals)
         SELECT make_histogram(lo, hi, 5, CAST(v AS DOUBLE))
         FROM vals CROSS JOIN bounds
         GROUP BY lo, hi",
    )
    .await;

    assert_eq!(batch.num_rows(), 1);
    let col = batch.column(0);
    assert!(!col.is_null(0), "histogram should not be null");
    let struct_array = col
        .as_any()
        .downcast_ref::<StructArray>()
        .expect("should be StructArray");
    let starts = struct_array
        .column(0)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("should be Float64Array");
    let ends = struct_array
        .column(1)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("should be Float64Array");
    let counts = struct_array
        .column(6)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("should be UInt64Array");
    assert!((starts.value(0) - 1.0).abs() < f64::EPSILON);
    assert!((ends.value(0) - 10.0).abs() < f64::EPSILON);
    assert_eq!(counts.value(0), 10);
}

// ── Test 2: Zero input rows ────────────────────────────────────────────────

#[tokio::test]
async fn test_zero_input_rows_runtime_bounds_null() {
    let ctx = make_ctx().await;
    ctx.sql("CREATE TABLE empty_vals (v DOUBLE)")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");
    ctx.sql("CREATE TABLE bounds_tbl (lo DOUBLE, hi DOUBLE, nb BIGINT)")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");
    ctx.sql("INSERT INTO bounds_tbl VALUES (0.0, 100.0, 10)")
        .await
        .expect("insert failed")
        .collect()
        .await
        .expect("collect failed");

    let batch = run_sql(
        &ctx,
        "SELECT make_histogram(lo, hi, nb, v)
         FROM empty_vals CROSS JOIN bounds_tbl",
    )
    .await;

    assert_eq!(batch.num_rows(), 1);
    assert!(
        batch.column(0).is_null(0),
        "aggregate over empty input should be NULL"
    );
}

// Literal bounds over empty input → eagerly configured accumulator → non-null histogram with count=0
#[tokio::test]
async fn test_zero_input_rows_literal_bounds_empty_histogram() {
    let ctx = make_ctx().await;
    ctx.sql("CREATE TABLE empty_vals2 (v DOUBLE)")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");

    let batch = run_sql(
        &ctx,
        "SELECT make_histogram(0.0, 100.0, 10, v) FROM empty_vals2",
    )
    .await;

    assert_eq!(batch.num_rows(), 1);
    // literal bounds → accumulator is pre-configured → result is non-null with count=0
    assert!(
        !batch.column(0).is_null(0),
        "literal-bounds aggregate over empty should not be null (accumulator is pre-configured)"
    );
    let struct_array = batch
        .column(0)
        .as_any()
        .downcast_ref::<StructArray>()
        .expect("should be StructArray");
    let counts = struct_array
        .column(6)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("should be UInt64Array");
    assert_eq!(counts.value(0), 0, "count should be 0 with no input rows");
}

// ── Test 3: NULL histogram through consumers ──────────────────────────────
// Runtime bounds + empty data → null histogram → each consumer returns null.

#[tokio::test]
async fn test_null_histogram_consumers() {
    let ctx = make_ctx().await;
    ctx.sql("CREATE TABLE nullhisto_data (v DOUBLE)")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");
    ctx.sql("CREATE TABLE nullhisto_bounds (lo DOUBLE, hi DOUBLE, nb BIGINT)")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");
    ctx.sql("INSERT INTO nullhisto_bounds VALUES (0.0, 100.0, 10)")
        .await
        .expect("insert failed")
        .collect()
        .await
        .expect("collect failed");

    // Subquery that produces a null histogram (runtime bounds, empty data)
    let null_histo_sql = "(SELECT make_histogram(lo, hi, nb, v)
                           FROM nullhisto_data CROSS JOIN nullhisto_bounds)";

    // quantile_from_histogram
    let batch = run_sql(
        &ctx,
        &format!("SELECT quantile_from_histogram({null_histo_sql}, 0.5)"),
    )
    .await;
    assert_eq!(batch.num_rows(), 1);
    assert!(
        batch.column(0).is_null(0),
        "quantile of NULL histogram should be NULL"
    );

    // sum_from_histogram
    let batch = run_sql(
        &ctx,
        &format!("SELECT sum_from_histogram({null_histo_sql})"),
    )
    .await;
    assert_eq!(batch.num_rows(), 1);
    assert!(
        batch.column(0).is_null(0),
        "sum of NULL histogram should be NULL"
    );

    // count_from_histogram
    let batch = run_sql(
        &ctx,
        &format!("SELECT count_from_histogram({null_histo_sql})"),
    )
    .await;
    assert_eq!(batch.num_rows(), 1);
    assert!(
        batch.column(0).is_null(0),
        "count of NULL histogram should be NULL"
    );

    // variance_from_histogram
    let batch = run_sql(
        &ctx,
        &format!("SELECT variance_from_histogram({null_histo_sql})"),
    )
    .await;
    assert_eq!(batch.num_rows(), 1);
    assert!(
        batch.column(0).is_null(0),
        "variance of NULL histogram should be NULL"
    );
}

#[tokio::test]
async fn test_expand_null_histogram_returns_zero_rows() {
    let ctx = make_ctx().await;
    ctx.sql("CREATE TABLE nullexpand_data (v DOUBLE)")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");
    ctx.sql("CREATE TABLE nullexpand_bounds (lo DOUBLE, hi DOUBLE, nb BIGINT)")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");
    ctx.sql("INSERT INTO nullexpand_bounds VALUES (0.0, 100.0, 10)")
        .await
        .expect("insert failed")
        .collect()
        .await
        .expect("collect failed");

    let batch = run_sql(
        &ctx,
        "SELECT * FROM expand_histogram(
           (SELECT make_histogram(lo, hi, nb, v)
            FROM nullexpand_data CROSS JOIN nullexpand_bounds)
         )",
    )
    .await;
    assert_eq!(
        batch.num_rows(),
        0,
        "expand_histogram of NULL should return zero rows"
    );
}

// ── Test 4: Invalid bounds ────────────────────────────────────────────────

#[tokio::test]
async fn test_invalid_literal_nb_bins_zero() {
    let ctx = make_ctx().await;
    ctx.sql("CREATE TABLE inv_tbl (v DOUBLE)")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");
    ctx.sql("INSERT INTO inv_tbl VALUES (1.0)")
        .await
        .expect("insert failed")
        .collect()
        .await
        .expect("collect failed");

    let result = ctx
        .sql("SELECT make_histogram(0.0, 100.0, 0, v) FROM inv_tbl")
        .await;
    let err = match result {
        Err(e) => e.to_string(),
        Ok(df) => df
            .collect()
            .await
            .expect_err("should have errored")
            .to_string(),
    };
    assert!(
        err.contains("nb_bins") || err.contains("bins"),
        "expected nb_bins error, got: {err}"
    );
}

#[tokio::test]
async fn test_invalid_literal_start_gt_end() {
    let ctx = make_ctx().await;
    ctx.sql("CREATE TABLE inv_tbl2 (v DOUBLE)")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");
    ctx.sql("INSERT INTO inv_tbl2 VALUES (1.0)")
        .await
        .expect("insert failed")
        .collect()
        .await
        .expect("collect failed");

    let result = ctx
        .sql("SELECT make_histogram(100.0, 0.0, 10, v) FROM inv_tbl2")
        .await;
    let err = match result {
        Err(e) => e.to_string(),
        Ok(df) => df
            .collect()
            .await
            .expect_err("should have errored")
            .to_string(),
    };
    assert!(
        err.contains("start") || err.contains("end"),
        "expected start>end error, got: {err}"
    );
}

#[tokio::test]
async fn test_invalid_runtime_nb_bins_zero() {
    let ctx = make_ctx().await;
    ctx.sql("CREATE TABLE rt_inv (v DOUBLE, nb BIGINT)")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");
    ctx.sql("INSERT INTO rt_inv VALUES (1.0, 0)")
        .await
        .expect("insert failed")
        .collect()
        .await
        .expect("collect failed");

    let err = ctx
        .sql("SELECT make_histogram(0.0, 100.0, nb, v) FROM rt_inv")
        .await
        .expect("parse ok")
        .collect()
        .await
        .expect_err("should have errored")
        .to_string();
    assert!(
        err.contains("nb_bins") || err.contains("bins"),
        "expected nb_bins error, got: {err}"
    );
}

#[tokio::test]
async fn test_invalid_runtime_start_gt_end() {
    let ctx = make_ctx().await;
    ctx.sql("CREATE TABLE rt_inv2 (v DOUBLE, lo DOUBLE, hi DOUBLE)")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");
    ctx.sql("INSERT INTO rt_inv2 VALUES (1.0, 100.0, 0.0)")
        .await
        .expect("insert failed")
        .collect()
        .await
        .expect("collect failed");

    let err = ctx
        .sql("SELECT make_histogram(lo, hi, 10, v) FROM rt_inv2")
        .await
        .expect("parse ok")
        .collect()
        .await
        .expect_err("should have errored")
        .to_string();
    assert!(
        err.contains("start") || err.contains("end"),
        "expected start>end error, got: {err}"
    );
}

#[tokio::test]
async fn test_invalid_runtime_null_bound() {
    let ctx = make_ctx().await;
    ctx.sql("CREATE TABLE rt_null (v DOUBLE, lo DOUBLE)")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");
    ctx.sql("INSERT INTO rt_null VALUES (1.0, NULL)")
        .await
        .expect("insert failed")
        .collect()
        .await
        .expect("collect failed");

    let err = ctx
        .sql("SELECT make_histogram(lo, 100.0, 10, v) FROM rt_null")
        .await
        .expect("parse ok")
        .collect()
        .await
        .expect_err("should have errored")
        .to_string();
    assert!(
        err.contains("null") || err.contains("start"),
        "expected null-bound error, got: {err}"
    );
}

// ── Test 5: Degenerate point histogram (start == end) ────────────────────

#[tokio::test]
async fn test_point_histogram_runtime_bounds() {
    let ctx = make_ctx().await;
    ctx.sql("CREATE TABLE const_vals AS SELECT 42.0 AS v FROM range(5)")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");

    // min(v) == max(v) == 42.0 — start == end
    let batch = run_sql(
        &ctx,
        "WITH bounds AS (SELECT CAST(MIN(v) AS DOUBLE) AS lo,
                                CAST(MAX(v) AS DOUBLE) AS hi
                         FROM const_vals)
         SELECT make_histogram(lo, hi, 3, v)
         FROM const_vals CROSS JOIN bounds
         GROUP BY lo, hi",
    )
    .await;

    assert_eq!(batch.num_rows(), 1);
    assert!(
        !batch.column(0).is_null(0),
        "point histogram should not be null"
    );
    let struct_array = batch
        .column(0)
        .as_any()
        .downcast_ref::<StructArray>()
        .expect("should be StructArray");
    let counts = struct_array
        .column(6)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("should be UInt64Array");
    assert_eq!(counts.value(0), 5, "all 5 values should be counted");
}

#[tokio::test]
async fn test_point_histogram_literal_bounds() {
    let ctx = make_ctx().await;
    ctx.sql("CREATE TABLE const_vals2 AS SELECT 42.0 AS v FROM range(5)")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");

    let batch = run_sql(
        &ctx,
        "SELECT make_histogram(42.0, 42.0, 3, v) FROM const_vals2",
    )
    .await;
    assert_eq!(batch.num_rows(), 1);
    assert!(
        !batch.column(0).is_null(0),
        "point histogram should not be null"
    );
    let struct_array = batch
        .column(0)
        .as_any()
        .downcast_ref::<StructArray>()
        .expect("should be StructArray");
    let counts = struct_array
        .column(6)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("should be UInt64Array");
    assert_eq!(counts.value(0), 5);
}

#[tokio::test]
async fn test_point_histogram_expand() {
    let ctx = make_ctx().await;
    ctx.sql("CREATE TABLE const_vals3 AS SELECT 42.0 AS v FROM range(5)")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");

    let batch = run_sql(
        &ctx,
        "SELECT * FROM expand_histogram(
           (SELECT make_histogram(42.0, 42.0, 3, v) FROM const_vals3)
         )",
    )
    .await;
    assert_eq!(batch.num_rows(), 3, "3 bins expected");
}

// ── Test 6: Inconsistent bounds ───────────────────────────────────────────
// Uses batch_size=1 so that each row arrives in its own batch, ensuring the
// per-batch consistency check fires on the second row.

#[tokio::test]
async fn test_inconsistent_runtime_bounds_error() {
    let ctx = make_ctx_batch1().await;
    ctx.sql("CREATE TABLE mixed_bounds (v DOUBLE, lo DOUBLE, hi DOUBLE)")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");
    ctx.sql("INSERT INTO mixed_bounds VALUES (1.0, 0.0, 100.0), (2.0, 1.0, 100.0)")
        .await
        .expect("insert failed")
        .collect()
        .await
        .expect("collect failed");

    let err = ctx
        .sql("SELECT make_histogram(lo, hi, 10, v) FROM mixed_bounds")
        .await
        .expect("parse ok")
        .collect()
        .await
        .expect_err("should have errored on inconsistent bounds")
        .to_string();
    // May fire in update_batch ("bounds/bins changed") or in merge_histograms
    // ("incompatible histograms"), depending on how DataFusion batches the rows.
    assert!(
        err.contains("bounds") || err.contains("changed") || err.contains("incompatible"),
        "expected incompatible-bounds error, got: {err}"
    );
}

// ── Test 7: Literal bounds happy path (regression guard) ──────────────────

#[tokio::test]
async fn test_literal_bounds_happy_path() {
    let ctx = make_ctx().await;
    ctx.sql("CREATE TABLE lit_tbl AS SELECT unnest(range(1, 101)) AS v")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");

    let batch = run_sql(
        &ctx,
        "SELECT make_histogram(0.0, 100.0, 10, CAST(v AS DOUBLE)) FROM lit_tbl",
    )
    .await;

    assert_eq!(batch.num_rows(), 1);
    assert!(
        !batch.column(0).is_null(0),
        "literal-bounds histogram should not be null"
    );
    let struct_array = batch
        .column(0)
        .as_any()
        .downcast_ref::<StructArray>()
        .expect("should be StructArray");
    let starts = struct_array
        .column(0)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("should be Float64Array");
    let ends = struct_array
        .column(1)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("should be Float64Array");
    let counts = struct_array
        .column(6)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("should be UInt64Array");
    assert!((starts.value(0) - 0.0).abs() < f64::EPSILON);
    assert!((ends.value(0) - 100.0).abs() < f64::EPSILON);
    assert_eq!(counts.value(0), 100);
}

#[tokio::test]
async fn test_literal_bounds_expand() {
    let ctx = make_ctx().await;
    ctx.sql("CREATE TABLE lit_expand AS SELECT unnest(range(1, 11)) AS v")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");

    let batch = run_sql(
        &ctx,
        "SELECT * FROM expand_histogram(
           (SELECT make_histogram(0.0, 100.0, 10, CAST(v AS DOUBLE)) FROM lit_expand)
         )",
    )
    .await;

    assert_eq!(batch.num_rows(), 10, "10 bins expected");
    let counts = batch
        .column(1)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("should be UInt64Array");
    let total: u64 = (0..10).map(|i| counts.value(i)).sum();
    assert_eq!(total, 10);
}

// ── Test 8: sum_histograms over null ──────────────────────────────────────

#[tokio::test]
async fn test_sum_histograms_over_null() {
    let ctx = make_ctx().await;
    ctx.sql("CREATE TABLE nullsum_data (v DOUBLE)")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");
    ctx.sql("CREATE TABLE nullsum_bounds (lo DOUBLE, hi DOUBLE, nb BIGINT)")
        .await
        .expect("create failed")
        .collect()
        .await
        .expect("collect failed");
    ctx.sql("INSERT INTO nullsum_bounds VALUES (0.0, 100.0, 10)")
        .await
        .expect("insert failed")
        .collect()
        .await
        .expect("collect failed");

    // sum_histograms over a null histogram should yield null
    let batch = run_sql(
        &ctx,
        "SELECT sum_histograms(h)
         FROM (SELECT make_histogram(lo, hi, nb, v) AS h
               FROM nullsum_data CROSS JOIN nullsum_bounds)",
    )
    .await;

    assert_eq!(batch.num_rows(), 1);
    assert!(
        batch.column(0).is_null(0),
        "sum_histograms over null should be null"
    );
}
