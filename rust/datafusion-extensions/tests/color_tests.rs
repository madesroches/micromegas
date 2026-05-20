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

// ---------- color_scale ----------
//
// Endpoint expected values come from `colorous::<NAME>.eval_continuous(0.0)`
// and `(1.0)`; baking them in pins the gradient data so a future `colorous`
// release that silently shifts the tables fails loudly here.

#[tokio::test]
async fn color_scale_viridis_endpoints() {
    let ctx = make_ctx();
    assert_eq!(
        eval_u32(&ctx, "color_scale('viridis', 0.0, 1.0)").await,
        vec![Some(0x440154ff)]
    );
    assert_eq!(
        eval_u32(&ctx, "color_scale('viridis', 1.0, 1.0)").await,
        vec![Some(0xfde725ff)]
    );
}

#[tokio::test]
async fn color_scale_magma_endpoints() {
    let ctx = make_ctx();
    assert_eq!(
        eval_u32(&ctx, "color_scale('magma', 0.0, 1.0)").await,
        vec![Some(0x000004ff)]
    );
    assert_eq!(
        eval_u32(&ctx, "color_scale('magma', 1.0, 1.0)").await,
        vec![Some(0xfcfdbfff)]
    );
}

#[tokio::test]
async fn color_scale_plasma_endpoints() {
    let ctx = make_ctx();
    assert_eq!(
        eval_u32(&ctx, "color_scale('plasma', 0.0, 1.0)").await,
        vec![Some(0x0d0887ff)]
    );
    assert_eq!(
        eval_u32(&ctx, "color_scale('plasma', 1.0, 1.0)").await,
        vec![Some(0xf0f921ff)]
    );
}

#[tokio::test]
async fn color_scale_inferno_endpoints() {
    let ctx = make_ctx();
    assert_eq!(
        eval_u32(&ctx, "color_scale('inferno', 0.0, 1.0)").await,
        vec![Some(0x000004ff)]
    );
    assert_eq!(
        eval_u32(&ctx, "color_scale('inferno', 1.0, 1.0)").await,
        vec![Some(0xfcffa4ff)]
    );
}

#[tokio::test]
async fn color_scale_cividis_endpoints() {
    let ctx = make_ctx();
    assert_eq!(
        eval_u32(&ctx, "color_scale('cividis', 0.0, 1.0)").await,
        vec![Some(0x002051ff)]
    );
    assert_eq!(
        eval_u32(&ctx, "color_scale('cividis', 1.0, 1.0)").await,
        vec![Some(0xfde945ff)]
    );
}

#[tokio::test]
async fn color_scale_turbo_endpoints() {
    let ctx = make_ctx();
    assert_eq!(
        eval_u32(&ctx, "color_scale('turbo', 0.0, 1.0)").await,
        vec![Some(0x22171bff)]
    );
    assert_eq!(
        eval_u32(&ctx, "color_scale('turbo', 1.0, 1.0)").await,
        vec![Some(0x900c00ff)]
    );
}

#[tokio::test]
async fn color_scale_alpha_is_honored() {
    let ctx = make_ctx();
    let full = eval_u32(&ctx, "color_scale('viridis', 0.0, 1.0)").await;
    let half = eval_u32(&ctx, "color_scale('viridis', 0.0, 0.5)").await;
    // Same RGB, alpha low byte goes from 0xff -> 0x80 (round-half-up of 127.5).
    assert_eq!(full, vec![Some(0x440154ff)]);
    assert_eq!(half, vec![Some(0x44015480)]);
}

#[tokio::test]
async fn color_scale_alpha_clamps() {
    let ctx = make_ctx();
    assert_eq!(
        eval_u32(&ctx, "color_scale('viridis', 0.0, -0.5)").await,
        vec![Some(0x44015400)]
    );
    assert_eq!(
        eval_u32(&ctx, "color_scale('viridis', 0.0, 1.5)").await,
        vec![Some(0x440154ff)]
    );
}

#[tokio::test]
async fn color_scale_t_clamps_below_zero() {
    let ctx = make_ctx();
    let with_negative = eval_u32(&ctx, "color_scale('viridis', -0.5, 1.0)").await;
    let with_zero = eval_u32(&ctx, "color_scale('viridis', 0.0, 1.0)").await;
    assert_eq!(with_negative, with_zero);
}

#[tokio::test]
async fn color_scale_t_clamps_above_one() {
    let ctx = make_ctx();
    let with_high = eval_u32(&ctx, "color_scale('viridis', 1.5, 1.0)").await;
    let with_one = eval_u32(&ctx, "color_scale('viridis', 1.0, 1.0)").await;
    assert_eq!(with_high, with_one);
}

#[tokio::test]
async fn color_scale_name_is_case_insensitive() {
    let ctx = make_ctx();
    let canon = eval_u32(&ctx, "color_scale('viridis', 0.5, 1.0)").await;
    assert_eq!(
        eval_u32(&ctx, "color_scale('Viridis', 0.5, 1.0)").await,
        canon
    );
    assert_eq!(
        eval_u32(&ctx, "color_scale('VIRIDIS', 0.5, 1.0)").await,
        canon
    );
}

#[tokio::test]
async fn color_scale_null_inputs_yield_null() {
    let ctx = make_ctx();
    assert_eq!(
        eval_u32(&ctx, "color_scale(CAST(NULL AS VARCHAR), 0.5, 1.0)").await,
        vec![None]
    );
    assert_eq!(
        eval_u32(&ctx, "color_scale('viridis', CAST(NULL AS DOUBLE), 1.0)").await,
        vec![None]
    );
    assert_eq!(
        eval_u32(&ctx, "color_scale('viridis', 0.5, CAST(NULL AS DOUBLE))").await,
        vec![None]
    );
}

#[tokio::test]
async fn color_scale_rejects_unknown_name() {
    let ctx = make_ctx();
    let result = ctx.sql("SELECT color_scale('not_a_scale', 0.5, 1.0)").await;
    // Constant-folded literal calls surface the error at plan time
    // (SQL build), but DataFusion may also return it at execute time. Try
    // both, accept whichever yields the error, and pin the recognized-set
    // hint either way.
    let err = match result {
        Err(e) => e.to_string(),
        Ok(df) => match df.collect().await {
            Err(e) => e.to_string(),
            Ok(_) => panic!("color_scale('not_a_scale', ...) should have failed"),
        },
    };
    assert!(
        err.contains("not_a_scale"),
        "error must mention the bad colormap name, got: {err}",
    );
    assert!(
        err.contains("viridis") && err.contains("turbo"),
        "error must list the recognized set, got: {err}",
    );
}

#[tokio::test]
async fn color_scale_int_literal_coerces() {
    // `Signature::exact` coerces Int64 -> Float64; round-tripping 0 and 1
    // through the int path should match the float endpoints.
    let ctx = make_ctx();
    assert_eq!(
        eval_u32(&ctx, "color_scale('viridis', 0, 1)").await,
        vec![Some(0x440154ff)]
    );
}

#[tokio::test]
async fn color_scale_composes_with_arithmetic() {
    // Smoke test: column-driven `t` derived from arithmetic in a small
    // literal table. Asserts no panic and a well-formed UInt32 result.
    let ctx = make_ctx();
    let df = ctx
        .sql(
            "SELECT color_scale('viridis', metric / 100.0, 1.0) AS v \
             FROM (VALUES (0.0), (50.0), (100.0)) AS t(metric)",
        )
        .await
        .expect("SQL query failed");
    let batches = df.collect().await.expect("failed to collect results");
    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total, 3);
    let mut rows: Vec<Option<u32>> = Vec::with_capacity(3);
    for batch in &batches {
        let arr = batch
            .column(0)
            .as_any()
            .downcast_ref::<UInt32Array>()
            .expect("should be UInt32Array");
        for i in 0..arr.len() {
            rows.push(if arr.is_null(i) {
                None
            } else {
                Some(arr.value(i))
            });
        }
    }
    assert_eq!(rows[0], Some(0x440154ff));
    assert_eq!(rows[2], Some(0xfde725ff));
    // Midpoint just has to be non-null and not equal to either endpoint.
    let mid = rows[1].expect("midpoint must not be NULL");
    assert_ne!(mid, 0x440154ff);
    assert_ne!(mid, 0xfde725ff);
}
