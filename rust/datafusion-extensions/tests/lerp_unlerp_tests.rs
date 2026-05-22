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

// --- lerp ---------------------------------------------------------------

#[tokio::test]
async fn lerp_endpoints() {
    let ctx = make_ctx();
    assert_eq!(
        eval_f64(&ctx, "lerp(0.0, 10.0, 0.0)").await,
        vec![Some(0.0)]
    );
    assert_eq!(
        eval_f64(&ctx, "lerp(0.0, 10.0, 1.0)").await,
        vec![Some(10.0)]
    );
}

#[tokio::test]
async fn lerp_midpoint() {
    let ctx = make_ctx();
    assert_eq!(
        eval_f64(&ctx, "lerp(0.0, 10.0, 0.5)").await,
        vec![Some(5.0)]
    );
}

#[tokio::test]
async fn lerp_extrapolation_no_clamping() {
    let ctx = make_ctx();
    assert_eq!(
        eval_f64(&ctx, "lerp(0.0, 10.0, 2.0)").await,
        vec![Some(20.0)]
    );
    assert_eq!(
        eval_f64(&ctx, "lerp(0.0, 10.0, -0.5)").await,
        vec![Some(-5.0)]
    );
}

#[tokio::test]
async fn lerp_reversed_endpoints() {
    let ctx = make_ctx();
    assert_eq!(
        eval_f64(&ctx, "lerp(10.0, 0.0, 0.25)").await,
        vec![Some(7.5)]
    );
}

#[tokio::test]
async fn lerp_null_propagation() {
    let ctx = make_ctx();
    assert_eq!(
        eval_f64(&ctx, "lerp(CAST(NULL AS DOUBLE), 1.0, 0.5)").await,
        vec![None]
    );
    assert_eq!(
        eval_f64(&ctx, "lerp(0.0, CAST(NULL AS DOUBLE), 0.5)").await,
        vec![None]
    );
    assert_eq!(
        eval_f64(&ctx, "lerp(0.0, 1.0, CAST(NULL AS DOUBLE))").await,
        vec![None]
    );
}

#[tokio::test]
async fn lerp_accepts_int_literals_via_coercion() {
    let ctx = make_ctx();
    assert_eq!(eval_f64(&ctx, "lerp(0, 10, 0.5)").await, vec![Some(5.0)]);
}

// --- unlerp -------------------------------------------------------------

#[tokio::test]
async fn unlerp_endpoints() {
    let ctx = make_ctx();
    assert_eq!(
        eval_f64(&ctx, "unlerp(0.0, 10.0, 0.0)").await,
        vec![Some(0.0)]
    );
    assert_eq!(
        eval_f64(&ctx, "unlerp(0.0, 10.0, 10.0)").await,
        vec![Some(1.0)]
    );
}

#[tokio::test]
async fn unlerp_midpoint() {
    let ctx = make_ctx();
    assert_eq!(
        eval_f64(&ctx, "unlerp(0.0, 10.0, 5.0)").await,
        vec![Some(0.5)]
    );
}

#[tokio::test]
async fn unlerp_outside_range() {
    let ctx = make_ctx();
    assert_eq!(
        eval_f64(&ctx, "unlerp(0.0, 10.0, 15.0)").await,
        vec![Some(1.5)]
    );
    assert_eq!(
        eval_f64(&ctx, "unlerp(0.0, 10.0, -2.0)").await,
        vec![Some(-0.2)]
    );
}

#[tokio::test]
async fn unlerp_degenerate_a_eq_b() {
    let ctx = make_ctx();
    // 0/0 -> NaN.
    let nan_row = eval_f64(&ctx, "unlerp(5.0, 5.0, 5.0)").await;
    assert_eq!(nan_row.len(), 1);
    let v = nan_row[0].expect("non-null");
    assert!(v.is_nan(), "expected NaN, got {v}");

    // positive / 0 -> +Inf.
    let pos_inf = eval_f64(&ctx, "unlerp(5.0, 5.0, 7.0)").await;
    let v = pos_inf[0].expect("non-null");
    assert!(v.is_infinite() && v > 0.0, "expected +Inf, got {v}");

    // negative / 0 -> -Inf.
    let neg_inf = eval_f64(&ctx, "unlerp(5.0, 5.0, 3.0)").await;
    let v = neg_inf[0].expect("non-null");
    assert!(v.is_infinite() && v < 0.0, "expected -Inf, got {v}");
}

#[tokio::test]
async fn unlerp_nanvl_fallback_recipe() {
    let ctx = make_ctx();
    // Documented fallback for degenerate unlerp.
    assert_eq!(
        eval_f64(&ctx, "nanvl(unlerp(5.0, 5.0, 5.0), 0.0)").await,
        vec![Some(0.0)]
    );
}

#[tokio::test]
async fn unlerp_null_propagation() {
    let ctx = make_ctx();
    assert_eq!(
        eval_f64(&ctx, "unlerp(CAST(NULL AS DOUBLE), 1.0, 0.5)").await,
        vec![None]
    );
    assert_eq!(
        eval_f64(&ctx, "unlerp(0.0, CAST(NULL AS DOUBLE), 0.5)").await,
        vec![None]
    );
    assert_eq!(
        eval_f64(&ctx, "unlerp(0.0, 1.0, CAST(NULL AS DOUBLE))").await,
        vec![None]
    );
}

#[tokio::test]
async fn unlerp_accepts_int_literals_via_coercion() {
    let ctx = make_ctx();
    assert_eq!(eval_f64(&ctx, "unlerp(0, 10, 5)").await, vec![Some(0.5)]);
}

// --- composition / inverse property ------------------------------------

#[tokio::test]
async fn unlerp_is_inverse_of_lerp() {
    let ctx = make_ctx();
    let result = eval_f64(&ctx, "unlerp(2.0, 8.0, lerp(2.0, 8.0, 0.3))").await;
    let v = result[0].expect("non-null");
    assert!(
        (v - 0.3).abs() < 1e-12,
        "expected ~0.3 (within 1e-12), got {v}"
    );
}

#[tokio::test]
async fn lerp_is_inverse_of_unlerp() {
    let ctx = make_ctx();
    let result = eval_f64(&ctx, "lerp(2.0, 8.0, unlerp(2.0, 8.0, 4.5))").await;
    let v = result[0].expect("non-null");
    assert!(
        (v - 4.5).abs() < 1e-12,
        "expected ~4.5 (within 1e-12), got {v}"
    );
}

#[tokio::test]
async fn canonical_remap_via_lerp_unlerp() {
    let ctx = make_ctx();
    // Maps [10, 20] -> [0, 1]; 15 is the midpoint, so expect 0.5.
    assert_eq!(
        eval_f64(&ctx, "lerp(0.0, 1.0, unlerp(10.0, 20.0, 15.0))").await,
        vec![Some(0.5)]
    );
}

// --- column path --------------------------------------------------------

#[tokio::test]
async fn lerp_scalar_literals_with_column_input() {
    // Exercises the scalar->array expansion path through values_to_arrays.
    let ctx = make_ctx();
    let df = ctx
        .sql(
            "SELECT lerp(0.0, 100.0, t) AS v \
             FROM (VALUES (0.0), (0.5), (1.0)) tt(t)",
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
    assert_eq!(values, vec![0.0, 50.0, 100.0]);
}
