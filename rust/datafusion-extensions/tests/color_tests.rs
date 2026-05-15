use datafusion::arrow::array::{Array, UInt32Array};
use datafusion::prelude::SessionContext;

fn make_ctx() -> SessionContext {
    let ctx = SessionContext::new();
    micromegas_datafusion_extensions::register_extension_udfs(&ctx);
    ctx
}

async fn eval_u32(ctx: &SessionContext, expr: &str) -> Vec<Option<u32>> {
    let sql = format!("SELECT {expr} as v");
    let df = ctx.sql(&sql).await.expect("SQL query failed");
    let batches = df.collect().await.expect("failed to collect results");
    assert_eq!(batches.len(), 1);
    let batch = &batches[0];
    let arr = batch
        .column(0)
        .as_any()
        .downcast_ref::<UInt32Array>()
        .expect("should be UInt32Array");
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

// ---------- rgba ----------

#[tokio::test]
async fn rgba_red_full_alpha() {
    let ctx = make_ctx();
    assert_eq!(
        eval_u32(&ctx, "rgba(1.0, 0.0, 0.0, 1.0)").await,
        vec![Some(0xff0000ff)]
    );
}

#[tokio::test]
async fn rgba_black_full_alpha() {
    let ctx = make_ctx();
    assert_eq!(
        eval_u32(&ctx, "rgba(0.0, 0.0, 0.0, 1.0)").await,
        vec![Some(0x000000ff)]
    );
}

#[tokio::test]
async fn rgba_fully_transparent_black() {
    let ctx = make_ctx();
    assert_eq!(
        eval_u32(&ctx, "rgba(0.0, 0.0, 0.0, 0.0)").await,
        vec![Some(0x00000000)]
    );
}

#[tokio::test]
async fn rgba_clamps_out_of_range() {
    let ctx = make_ctx();
    // 2.0 -> 255 (saturate high), -1.0 -> 0 (saturate low),
    // 0.5 -> 128 (round-half-up), 1.0 -> 255.
    assert_eq!(
        eval_u32(&ctx, "rgba(2.0, -1.0, 0.5, 1.0)").await,
        vec![Some(0xff0080ff)]
    );
}

#[tokio::test]
async fn rgba_quantization_is_round_half_up() {
    let ctx = make_ctx();
    // 0.5 * 255 + 0.5 = 128.0 -> 128 on every channel.
    assert_eq!(
        eval_u32(&ctx, "rgba(0.5, 0.5, 0.5, 1.0)").await,
        vec![Some(0x808080ff)]
    );
}

#[tokio::test]
async fn rgba_null_input_yields_null() {
    let ctx = make_ctx();
    assert_eq!(
        eval_u32(&ctx, "rgba(CAST(NULL AS DOUBLE), 0.0, 0.0, 1.0)").await,
        vec![None]
    );
    assert_eq!(
        eval_u32(&ctx, "rgba(1.0, 0.0, 0.0, CAST(NULL AS DOUBLE))").await,
        vec![None]
    );
}

#[tokio::test]
async fn rgba_accepts_int_literals_via_coercion() {
    // DataFusion coerces Int64 -> Float64 under Signature::exact.
    let ctx = make_ctx();
    assert_eq!(
        eval_u32(&ctx, "rgba(1, 0, 0, 1)").await,
        vec![Some(0xff0000ff)]
    );
}

// ---------- lerp_color ----------

#[tokio::test]
async fn lerp_color_endpoint_t0() {
    let ctx = make_ctx();
    assert_eq!(
        eval_u32(&ctx, "lerp_color(rgba(1, 0, 0, 1), rgba(0, 0, 1, 1), 0.0)").await,
        vec![Some(0xff0000ff)]
    );
}

#[tokio::test]
async fn lerp_color_endpoint_t1() {
    let ctx = make_ctx();
    assert_eq!(
        eval_u32(&ctx, "lerp_color(rgba(1, 0, 0, 1), rgba(0, 0, 1, 1), 1.0)").await,
        vec![Some(0x0000ffff)]
    );
}

#[tokio::test]
async fn lerp_color_midpoint_round_half_up() {
    let ctx = make_ctx();
    // rgba(1,0,0,0) = 0xff000000, rgba(0,1,0,0) = 0x00ff0000.
    // Midpoint: R/G = (255 + 0) * 0.5 = 127.5 -> 128, B/A = 0.
    assert_eq!(
        eval_u32(&ctx, "lerp_color(rgba(1, 0, 0, 0), rgba(0, 1, 0, 0), 0.5)").await,
        vec![Some(0x80800000)]
    );
}

#[tokio::test]
async fn lerp_color_midpoint_via_cast_int() {
    let ctx = make_ctx();
    // Same lerp expressed using CAST AS INT UNSIGNED.
    let result = eval_u32(
        &ctx,
        "lerp_color(CAST(4278190080 AS INT UNSIGNED), CAST(16711680 AS INT UNSIGNED), 0.5)",
    )
    .await;
    assert_eq!(result, vec![Some(0x80800000)]);
}

#[tokio::test]
async fn lerp_color_clamps_t_below_zero() {
    let ctx = make_ctx();
    // t = -0.5 behaves like t = 0.0
    let with_negative =
        eval_u32(&ctx, "lerp_color(rgba(1, 0, 0, 1), rgba(0, 0, 1, 1), -0.5)").await;
    let with_zero = eval_u32(&ctx, "lerp_color(rgba(1, 0, 0, 1), rgba(0, 0, 1, 1), 0.0)").await;
    assert_eq!(with_negative, with_zero);
}

#[tokio::test]
async fn lerp_color_clamps_t_above_one() {
    let ctx = make_ctx();
    // t = 1.5 behaves like t = 1.0
    let with_high = eval_u32(&ctx, "lerp_color(rgba(1, 0, 0, 1), rgba(0, 0, 1, 1), 1.5)").await;
    let with_one = eval_u32(&ctx, "lerp_color(rgba(1, 0, 0, 1), rgba(0, 0, 1, 1), 1.0)").await;
    assert_eq!(with_high, with_one);
}

#[tokio::test]
async fn lerp_color_interpolates_alpha() {
    let ctx = make_ctx();
    // rgba(0,0,0,0) -> rgba(0,0,0,1) at t=0.5: only alpha changes (0 -> 128).
    assert_eq!(
        eval_u32(&ctx, "lerp_color(rgba(0, 0, 0, 0), rgba(0, 0, 0, 1), 0.5)").await,
        vec![Some(0x00000080)]
    );
}

#[tokio::test]
async fn lerp_color_null_inputs_yield_null() {
    let ctx = make_ctx();
    assert_eq!(
        eval_u32(
            &ctx,
            "lerp_color(CAST(NULL AS INT UNSIGNED), rgba(0, 0, 1, 1), 0.5)"
        )
        .await,
        vec![None]
    );
    assert_eq!(
        eval_u32(
            &ctx,
            "lerp_color(rgba(1, 0, 0, 1), CAST(NULL AS INT UNSIGNED), 0.5)"
        )
        .await,
        vec![None]
    );
    assert_eq!(
        eval_u32(
            &ctx,
            "lerp_color(rgba(1, 0, 0, 1), rgba(0, 0, 1, 1), CAST(NULL AS DOUBLE))"
        )
        .await,
        vec![None]
    );
}

#[tokio::test]
async fn integration_lerp_red_blue_midpoint() {
    let ctx = make_ctx();
    // R: 255 -> 0 at t=0.5 -> 128; B: 0 -> 255 at t=0.5 -> 128; A: 255 -> 255.
    assert_eq!(
        eval_u32(&ctx, "lerp_color(rgba(1, 0, 0, 1), rgba(0, 0, 1, 1), 0.5)").await,
        vec![Some(0x800080ff)]
    );
}

#[tokio::test]
async fn lerp_color_rejects_bare_int_literals() {
    // Pins the caller contract documented in functions-reference.md:
    // bare Int64/hex literals do not coerce to UInt32 under Signature::exact.
    let ctx = make_ctx();
    let result = ctx.sql("SELECT lerp_color(4278190080, 65280, 0.5)").await;
    assert!(
        result.is_err(),
        "lerp_color must reject bare integer literals (no Int64 -> UInt32 coercion); got Ok",
    );
}
