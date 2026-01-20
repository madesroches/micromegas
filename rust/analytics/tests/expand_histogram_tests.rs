use datafusion::arrow::array::{Float64Array, UInt64Array};
use datafusion::arrow::datatypes::DataType;
use datafusion::catalog::TableProvider;
use datafusion::logical_expr::Accumulator;
use datafusion::prelude::*;
use datafusion::scalar::ScalarValue;
use micromegas_analytics::dfext::histogram::{
    accumulator::HistogramAccumulator, expand::ExpandHistogramTableProvider,
    histogram_udaf::HistogramArray,
};

/// Helper to create a histogram ScalarValue from test values
fn create_histogram_scalar(start: f64, end: f64, nb_bins: usize, values: &[f64]) -> ScalarValue {
    let mut acc = HistogramAccumulator::new(start, end, nb_bins);
    let array = Float64Array::from(values.to_vec());
    acc.update_batch_scalars(&array).expect("update failed");
    acc.evaluate().expect("evaluate failed")
}

/// Helper to extract bin counts from a histogram scalar
fn get_bin_counts(scalar: &ScalarValue) -> Vec<u64> {
    if let ScalarValue::Struct(struct_array) = scalar {
        let histo = HistogramArray::new(struct_array.clone());
        let bins = histo.get_bins(0).expect("get_bins failed");
        (0..bins.len()).map(|i| bins.value(i)).collect()
    } else {
        panic!("expected Struct scalar");
    }
}

#[test]
fn test_histogram_accumulator_basic() {
    // Values spread across bins: 5, 15, 25, 35, 45 in 0-50 range with 5 bins
    let scalar = create_histogram_scalar(0.0, 50.0, 5, &[5.0, 15.0, 25.0, 35.0, 45.0]);
    let counts = get_bin_counts(&scalar);

    assert_eq!(counts.len(), 5);
    for (i, count) in counts.iter().enumerate() {
        assert_eq!(*count, 1, "bin {i} should have count 1");
    }
}

#[test]
fn test_histogram_accumulator_multiple_per_bin() {
    // Multiple values in the same bin
    let scalar = create_histogram_scalar(0.0, 20.0, 2, &[1.0, 2.0, 3.0, 11.0, 12.0]);
    let counts = get_bin_counts(&scalar);

    assert_eq!(counts.len(), 2);
    assert_eq!(counts[0], 3); // 0-10 bin: 1, 2, 3
    assert_eq!(counts[1], 2); // 10-20 bin: 11, 12
}

#[tokio::test]
async fn test_expand_histogram_table_provider_basic() {
    let scalar = create_histogram_scalar(0.0, 50.0, 5, &[5.0, 15.0, 25.0, 35.0, 45.0]);

    let provider =
        ExpandHistogramTableProvider::from_scalar(scalar).expect("failed to create provider");

    // Verify schema
    let schema = provider.schema();
    assert_eq!(schema.fields().len(), 2);
    assert_eq!(schema.field(0).name(), "bin_center");
    assert_eq!(schema.field(0).data_type(), &DataType::Float64);
    assert_eq!(schema.field(1).name(), "count");
    assert_eq!(schema.field(1).data_type(), &DataType::UInt64);

    // Execute scan
    let ctx = SessionContext::new();
    let state = ctx.state();
    let plan = provider
        .scan(&state, None, &[], None)
        .await
        .expect("scan failed");

    let results = datafusion::physical_plan::collect(plan, ctx.task_ctx())
        .await
        .expect("collect failed");

    assert_eq!(results.len(), 1);
    let batch = &results[0];
    assert_eq!(batch.num_rows(), 5);

    let bin_centers = batch
        .column(0)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("expected Float64Array");

    let counts = batch
        .column(1)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("expected UInt64Array");

    // Verify bin centers: (start + (i + 0.5) * bin_width) for bin_width = 10
    assert_eq!(bin_centers.value(0), 5.0);
    assert_eq!(bin_centers.value(1), 15.0);
    assert_eq!(bin_centers.value(2), 25.0);
    assert_eq!(bin_centers.value(3), 35.0);
    assert_eq!(bin_centers.value(4), 45.0);

    for i in 0..5 {
        assert_eq!(counts.value(i), 1, "bin {i} should have count 1");
    }
}

#[tokio::test]
async fn test_expand_histogram_with_limit() {
    let scalar = create_histogram_scalar(0.0, 50.0, 5, &[5.0, 15.0, 25.0, 35.0, 45.0]);

    let provider =
        ExpandHistogramTableProvider::from_scalar(scalar).expect("failed to create provider");

    let ctx = SessionContext::new();
    let state = ctx.state();
    let plan = provider
        .scan(&state, None, &[], Some(3))
        .await
        .expect("scan failed");

    let results = datafusion::physical_plan::collect(plan, ctx.task_ctx())
        .await
        .expect("collect failed");

    assert_eq!(results.len(), 1);
    let batch = &results[0];
    assert_eq!(batch.num_rows(), 3);
}

#[tokio::test]
async fn test_expand_histogram_empty_data() {
    // Histogram with no values - all bins should be zero
    let scalar = create_histogram_scalar(0.0, 100.0, 10, &[]);

    let provider =
        ExpandHistogramTableProvider::from_scalar(scalar).expect("failed to create provider");

    let ctx = SessionContext::new();
    let state = ctx.state();
    let plan = provider
        .scan(&state, None, &[], None)
        .await
        .expect("scan failed");

    let results = datafusion::physical_plan::collect(plan, ctx.task_ctx())
        .await
        .expect("collect failed");

    assert_eq!(results.len(), 1);
    let batch = &results[0];
    assert_eq!(batch.num_rows(), 10);

    let counts = batch
        .column(1)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("expected UInt64Array");

    for i in 0..10 {
        assert_eq!(
            counts.value(i),
            0,
            "empty histogram bin {i} should have count 0"
        );
    }
}

#[tokio::test]
async fn test_expand_histogram_single_bin() {
    let scalar = create_histogram_scalar(0.0, 10.0, 1, &[5.0, 3.0, 7.0]);

    let provider =
        ExpandHistogramTableProvider::from_scalar(scalar).expect("failed to create provider");

    let ctx = SessionContext::new();
    let state = ctx.state();
    let plan = provider
        .scan(&state, None, &[], None)
        .await
        .expect("scan failed");

    let results = datafusion::physical_plan::collect(plan, ctx.task_ctx())
        .await
        .expect("collect failed");

    assert_eq!(results.len(), 1);
    let batch = &results[0];
    assert_eq!(batch.num_rows(), 1);

    let bin_centers = batch
        .column(0)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("expected Float64Array");

    let counts = batch
        .column(1)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .expect("expected UInt64Array");

    assert_eq!(bin_centers.value(0), 5.0); // center of 0-10
    assert_eq!(counts.value(0), 3);
}

#[test]
fn test_expand_histogram_invalid_scalar() {
    let invalid_scalar = ScalarValue::Float64(Some(42.0));

    let result = ExpandHistogramTableProvider::from_scalar(invalid_scalar);
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("must be a struct"),
        "error should mention struct requirement: {err}"
    );
}

#[tokio::test]
async fn test_expand_histogram_projection() {
    let scalar = create_histogram_scalar(0.0, 20.0, 2, &[5.0, 15.0]);

    let provider =
        ExpandHistogramTableProvider::from_scalar(scalar).expect("failed to create provider");

    let ctx = SessionContext::new();
    let state = ctx.state();

    // Project only the count column (index 1)
    let plan = provider
        .scan(&state, Some(&vec![1]), &[], None)
        .await
        .expect("scan failed");

    let results = datafusion::physical_plan::collect(plan, ctx.task_ctx())
        .await
        .expect("collect failed");

    assert_eq!(results.len(), 1);
    let batch = &results[0];
    assert_eq!(batch.num_columns(), 1);
    assert_eq!(batch.num_rows(), 2);
}
