use datafusion::arrow::array::{Array, Float64Array};
use datafusion::prelude::SessionContext;

fn make_ctx() -> SessionContext {
    let ctx = SessionContext::new();
    micromegas_datafusion_extensions::register_extension_udfs(&ctx);
    ctx
}

async fn eval_f64(ctx: &SessionContext, expr: &str) -> Vec<Option<f64>> {
    let sql = format!("SELECT {expr} as v");
    let df = ctx.sql(&sql).await.expect("SQL query failed");
    let batches = df.collect().await.expect("failed to collect results");
    assert_eq!(batches.len(), 1);
    let batch = &batches[0];
    let arr = batch
        .column(0)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("should be Float64Array");
    (0..arr.len())
        .map(|i| {
            if arr.is_null(i) {
                None
            } else {
                Some(arr.value(i))
            }
        })
        .collect()
}

#[tokio::test]
async fn bin_center_at_origin() {
    let ctx = make_ctx();
    assert_eq!(
        eval_f64(&ctx, "bin_center(0.0, 10.0)").await,
        vec![Some(0.0)]
    );
}

#[tokio::test]
async fn bin_center_inside_bin_no_rounding() {
    // 3 lies in [-5, 5), so center is 0.
    let ctx = make_ctx();
    assert_eq!(
        eval_f64(&ctx, "bin_center(3.0, 10.0)").await,
        vec![Some(0.0)]
    );
}

#[tokio::test]
async fn bin_center_negative_side() {
    let ctx = make_ctx();
    // -3 falls in [-5, 5).
    assert_eq!(
        eval_f64(&ctx, "bin_center(-3.0, 10.0)").await,
        vec![Some(0.0)]
    );
    // -5 is the inclusive lower bound of [-5, 5).
    assert_eq!(
        eval_f64(&ctx, "bin_center(-5.0, 10.0)").await,
        vec![Some(0.0)]
    );
    // Just below -5 belongs to the previous bin, [-15, -5).
    assert_eq!(
        eval_f64(&ctx, "bin_center(-5.0001, 10.0)").await,
        vec![Some(-10.0)]
    );
}

#[tokio::test]
async fn bin_center_upper_edge_lands_in_next_bin() {
    // Half-open: 5 belongs to [5, 15).
    let ctx = make_ctx();
    assert_eq!(
        eval_f64(&ctx, "bin_center(5.0, 10.0)").await,
        vec![Some(10.0)]
    );
}

#[tokio::test]
async fn bin_center_two_axis_composition() {
    let ctx = make_ctx();
    let sql = "SELECT bin_center(x, 10.0) AS bx, bin_center(y, 10.0) AS by \
               FROM (VALUES (3.0, 7.0), (-2.0, 12.0), (4.99, 4.99)) t(x, y)";
    let df = ctx.sql(sql).await.expect("SQL query failed");
    let batches = df.collect().await.expect("failed to collect results");
    assert_eq!(batches.len(), 1);
    let batch = &batches[0];
    let bx = batch
        .column(0)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("bx should be Float64Array");
    let by = batch
        .column(1)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("by should be Float64Array");
    let pairs: Vec<(f64, f64)> = (0..bx.len()).map(|i| (bx.value(i), by.value(i))).collect();
    assert_eq!(pairs, vec![(0.0, 10.0), (0.0, 10.0), (0.0, 0.0)]);
}

#[tokio::test]
async fn bin_center_null_propagation() {
    let ctx = make_ctx();
    assert_eq!(
        eval_f64(&ctx, "bin_center(CAST(NULL AS DOUBLE), 10.0)").await,
        vec![None]
    );
    assert_eq!(
        eval_f64(&ctx, "bin_center(3.0, CAST(NULL AS DOUBLE))").await,
        vec![None]
    );
}

#[tokio::test]
async fn bin_center_accepts_int_literals_via_coercion() {
    // DataFusion coerces Int64 -> Float64 under Signature::exact.
    let ctx = make_ctx();
    assert_eq!(eval_f64(&ctx, "bin_center(3, 10)").await, vec![Some(0.0)]);
}

#[tokio::test]
async fn bin_center_scalar_literal_with_column_coord() {
    // Sanity check: scalar cell_size literal expands per row without
    // broadcasting incorrectly.
    let ctx = make_ctx();
    let df = ctx
        .sql(
            "SELECT bin_center(x, 10.0) AS bx \
             FROM (VALUES (3.0), (12.0), (-7.0)) t(x)",
        )
        .await
        .expect("SQL query failed");
    let batches = df.collect().await.expect("failed to collect results");
    let batch = &batches[0];
    let arr = batch
        .column(0)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("should be Float64Array");
    let values: Vec<f64> = (0..arr.len()).map(|i| arr.value(i)).collect();
    assert_eq!(values, vec![0.0, 10.0, -10.0]);
}

#[tokio::test]
async fn bin_center_group_by_smoke() {
    // Regression guard: grouping by bin_center over both axes collapses
    // points into the expected number of cells.
    let ctx = make_ctx();
    let df = ctx
        .sql(
            "SELECT bin_center(x, 10.0) AS bx, bin_center(y, 10.0) AS by, COUNT(*) AS cnt \
             FROM (VALUES \
                 (1.0, 1.0), \
                 (2.0, 2.0), \
                 (12.0, 12.0), \
                 (13.0, 13.0), \
                 (-3.0, -3.0)) t(x, y) \
             GROUP BY 1, 2",
        )
        .await
        .expect("SQL query failed");
    let batches = df.collect().await.expect("failed to collect results");
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    // (1,1), (2,2), (-3,-3) collapse to (0,0); (12,12), (13,13) collapse
    // to (10,10) — two distinct cells.
    assert_eq!(total_rows, 2);
}
