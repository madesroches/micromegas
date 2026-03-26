use datafusion::arrow::array::{Array, Float64Array, UInt64Array};
use datafusion::arrow::datatypes::DataType;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::catalog::{TableFunctionImpl, TableProvider};
use datafusion::logical_expr::{Accumulator, Cast};
use datafusion::prelude::{Expr, SessionContext};
use datafusion::scalar::ScalarValue;
use micromegas_datafusion_extensions::histogram::accumulator::HistogramAccumulator;
use micromegas_datafusion_extensions::histogram::expand::{
    ExpandHistogramTableFunction, ExpandHistogramTableProvider,
};

fn make_test_histogram(start: f64, end: f64, nb_bins: usize, values: &[f64]) -> ScalarValue {
    let mut acc = HistogramAccumulator::new(start, end, nb_bins);
    let array = datafusion::arrow::array::Float64Array::from(values.to_vec());
    acc.update_batch_scalars(&array)
        .expect("failed to update histogram");
    acc.evaluate().expect("failed to evaluate histogram")
}

async fn collect_expand(
    provider: &ExpandHistogramTableProvider,
    limit: Option<usize>,
) -> RecordBatch {
    let ctx = SessionContext::new();
    let state = ctx.state();
    let plan = provider
        .scan(&state, None, &[], limit)
        .await
        .expect("scan failed");
    let task_ctx = state.task_ctx();
    let batches = datafusion::physical_plan::collect(plan, task_ctx)
        .await
        .expect("collect failed");
    assert_eq!(batches.len(), 1, "expected exactly one batch");
    batches.into_iter().next().expect("no batches")
}

#[test]
fn test_call_accepts_cast_expression() {
    let func = ExpandHistogramTableFunction::new();
    let inner = Expr::Literal(ScalarValue::Null, None);
    let cast_expr = Expr::Cast(Cast::new(Box::new(inner), DataType::Null));
    let result = func.call(&[cast_expr]);
    assert!(
        result.is_ok(),
        "call() should accept Cast expression, got: {result:?}"
    );
}

#[test]
fn test_call_rejects_wrong_arg_count() {
    let func = ExpandHistogramTableFunction::new();
    let result = func.call(&[]);
    assert!(result.is_err(), "call() should reject zero arguments");

    let a = Expr::Literal(ScalarValue::Null, None);
    let b = Expr::Literal(ScalarValue::Null, None);
    let result = func.call(&[a, b]);
    assert!(result.is_err(), "call() should reject two arguments");
}

#[tokio::test]
async fn test_expand_simple_histogram() {
    // 3 bins over [0, 30), values in each bin
    let scalar = make_test_histogram(0.0, 30.0, 3, &[5.0, 15.0, 25.0]);
    let provider = ExpandHistogramTableProvider::from_scalar(scalar).expect("from_scalar failed");
    let batch = collect_expand(&provider, None).await;

    assert_eq!(batch.num_rows(), 3);
    assert_eq!(batch.num_columns(), 2);

    let centers = batch
        .column(0)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("bin_center should be Float64Array");
    let counts = batch
        .column(1)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("count should be UInt64Array");

    // bin_width = 10, centers at 5, 15, 25
    assert!((centers.value(0) - 5.0).abs() < f64::EPSILON);
    assert!((centers.value(1) - 15.0).abs() < f64::EPSILON);
    assert!((centers.value(2) - 25.0).abs() < f64::EPSILON);

    // one value per bin
    assert_eq!(counts.value(0), 1);
    assert_eq!(counts.value(1), 1);
    assert_eq!(counts.value(2), 1);
}

#[tokio::test]
async fn test_expand_with_limit() {
    let scalar = make_test_histogram(0.0, 30.0, 3, &[5.0, 15.0, 25.0]);
    let provider = ExpandHistogramTableProvider::from_scalar(scalar).expect("from_scalar failed");
    let batch = collect_expand(&provider, Some(2)).await;

    assert_eq!(batch.num_rows(), 2, "limit should cap output rows");
}

#[tokio::test]
async fn test_expand_uneven_distribution() {
    // all values land in the first bin
    let scalar = make_test_histogram(0.0, 30.0, 3, &[1.0, 2.0, 3.0, 4.0]);
    let provider = ExpandHistogramTableProvider::from_scalar(scalar).expect("from_scalar failed");
    let batch = collect_expand(&provider, None).await;

    let counts = batch
        .column(1)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("count should be UInt64Array");

    assert_eq!(counts.value(0), 4);
    assert_eq!(counts.value(1), 0);
    assert_eq!(counts.value(2), 0);
}

#[tokio::test]
async fn test_scalar_to_batch_dictionary_wrapped() {
    // Simulate what DataFusion's constant-folding produces: Dictionary(Int32, Struct)
    let func = ExpandHistogramTableFunction::new();
    let inner_scalar = make_test_histogram(0.0, 10.0, 2, &[3.0, 7.0]);
    let dict_scalar = ScalarValue::Dictionary(
        Box::new(DataType::Int32),
        Box::new(inner_scalar),
    );
    let provider = func
        .call(&[Expr::Literal(dict_scalar, None)])
        .expect("call should accept dictionary-wrapped histogram");
    let batch = collect_expand(
        provider
            .as_any()
            .downcast_ref::<ExpandHistogramTableProvider>()
            .expect("should be ExpandHistogramTableProvider"),
        None,
    )
    .await;

    assert_eq!(batch.num_rows(), 2);
    let counts = batch
        .column(1)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("count should be UInt64Array");
    assert_eq!(counts.value(0), 1);
    assert_eq!(counts.value(1), 1);
}
