//! Tests for `OtelMetricsBlockProcessor` fanning OTLP `Summary` data points out into
//! `measures` rows (issue #1359): count/sum/min/max under suffixed metric names, with
//! any other `quantile_values` entry (configured percentiles) dropped.

use datafusion::arrow::array::{Array, StringArray};
use micromegas_analytics::lakehouse::block_partition_spec::BlockProcessor;
use micromegas_analytics::lakehouse::otel::metrics_block_processor::OtelMetricsBlockProcessor;
use micromegas_analytics::lakehouse::partition_source_data::PartitionSourceBlock;
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_telemetry::block_wire_format::BlockPayload;
use micromegas_telemetry::types::block::BlockMetadata;
use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use opentelemetry_proto::tonic::metrics::v1::{
    Metric, ResourceMetrics, ScopeMetrics, Summary, SummaryDataPoint, metric,
    summary_data_point::ValueAtQuantile,
};
use prost::Message;
use std::collections::HashMap;
use std::sync::Arc;

mod test_helpers;
use test_helpers::make_process_metadata;

fn make_in_memory_blob_storage() -> Arc<BlobStorage> {
    Arc::new(BlobStorage::new(
        Arc::new(object_store::memory::InMemory::new()),
        object_store::path::Path::from(""),
    ))
}

async fn make_source_block(
    blob_storage: &BlobStorage,
    payload_bytes: Vec<u8>,
    nb_objects: usize,
) -> anyhow::Result<Arc<PartitionSourceBlock>> {
    let process_id = uuid::Uuid::new_v4();
    let stream_id = uuid::Uuid::new_v4();
    let block_id = uuid::Uuid::new_v4();
    let now = chrono::Utc::now();

    let block_payload = BlockPayload {
        dependencies: vec![],
        objects: payload_bytes,
    };
    let mut buf = Vec::new();
    ciborium::into_writer(&block_payload, &mut buf)?;
    let obj_path = format!("blobs/{process_id}/{stream_id}/{block_id}");
    blob_storage.put(&obj_path, buf.into()).await?;

    let block = BlockMetadata {
        block_id,
        stream_id,
        process_id,
        begin_time: now,
        end_time: now,
        begin_ticks: 0,
        end_ticks: 0,
        nb_objects: nb_objects as i32,
        payload_size: block_payload.objects.len() as i64,
        object_offset: 0,
        insert_time: now,
    };
    let stream = Arc::new(micromegas_analytics::metadata::StreamMetadata {
        process_id,
        stream_id,
        dependencies_metadata: vec![],
        objects_metadata: vec![],
        tags: vec![],
        properties: Arc::new(vec![]),
    });
    let process = Arc::new(make_process_metadata(process_id, None, HashMap::new()));
    Ok(Arc::new(PartitionSourceBlock {
        block,
        stream,
        process,
        format: "otlp/v1/metrics".to_string(),
    }))
}

fn resource_metrics_with_one_summary(dp: SummaryDataPoint) -> ResourceMetrics {
    ResourceMetrics {
        resource: None,
        scope_metrics: vec![ScopeMetrics {
            scope: Some(InstrumentationScope {
                name: "test-scope".to_string(),
                version: String::new(),
                attributes: vec![],
                dropped_attributes_count: 0,
            }),
            metrics: vec![Metric {
                name: "CPUUtilization".to_string(),
                description: String::new(),
                unit: "Percent".to_string(),
                metadata: vec![],
                data: Some(metric::Data::Summary(Summary {
                    data_points: vec![dp],
                })),
            }],
            schema_url: String::new(),
        }],
        schema_url: String::new(),
    }
}

fn names(batch: &datafusion::arrow::record_batch::RecordBatch) -> Vec<String> {
    let names_col = batch
        .column_by_name("name")
        .expect("name column")
        .as_any()
        .downcast_ref::<datafusion::arrow::array::DictionaryArray<
            datafusion::arrow::datatypes::Int32Type,
        >>()
        .expect("name is a dictionary array");
    let values = names_col
        .values()
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("dictionary values are strings");
    names_col
        .keys()
        .iter()
        .map(|k| values.value(k.expect("no nulls") as usize).to_string())
        .collect()
}

fn units(batch: &datafusion::arrow::record_batch::RecordBatch) -> Vec<String> {
    let units_col = batch
        .column_by_name("unit")
        .expect("unit column")
        .as_any()
        .downcast_ref::<datafusion::arrow::array::DictionaryArray<
            datafusion::arrow::datatypes::Int32Type,
        >>()
        .expect("unit is a dictionary array");
    let values = units_col
        .values()
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("dictionary values are strings");
    units_col
        .keys()
        .iter()
        .map(|k| values.value(k.expect("no nulls") as usize).to_string())
        .collect()
}

fn values_f64(batch: &datafusion::arrow::record_batch::RecordBatch) -> Vec<f64> {
    let col = batch
        .column_by_name("value")
        .expect("value column")
        .as_any()
        .downcast_ref::<datafusion::arrow::array::Float64Array>()
        .expect("value is Float64");
    col.iter().map(|v| v.expect("no nulls")).collect()
}

/// One `SummaryDataPoint` with `count`, `sum`, `q=0.0` (min), `q=1.0` (max), and an extra
/// `q=0.9` (a CloudWatch configured-percentile entry). Must fan out to exactly 4 rows —
/// count/sum/min/max — under suffixed names, dropping the `0.9` quantile.
#[tokio::test]
async fn summary_fans_out_to_four_suffixed_rows_and_drops_non_min_max_quantiles() {
    let dp = SummaryDataPoint {
        attributes: vec![],
        start_time_unix_nano: 0,
        time_unix_nano: 1_000,
        count: 42,
        sum: 123.5,
        quantile_values: vec![
            ValueAtQuantile {
                quantile: 0.0,
                value: 1.0,
            },
            ValueAtQuantile {
                quantile: 1.0,
                value: 99.0,
            },
            ValueAtQuantile {
                quantile: 0.9,
                value: 90.0,
            },
        ],
        flags: 0,
    };
    let resource_metrics = resource_metrics_with_one_summary(dp);
    let payload_bytes = resource_metrics.encode_to_vec();

    let blob_storage = make_in_memory_blob_storage();
    let src_block = make_source_block(&blob_storage, payload_bytes, 1)
        .await
        .expect("make_source_block");

    let processor = OtelMetricsBlockProcessor {};
    let result = processor
        .process(blob_storage, src_block)
        .await
        .expect("process must not fail");
    let row_set = result.expect("expected Some(row_set)");
    let batch = row_set.rows;

    assert_eq!(batch.num_rows(), 4, "count/sum/min/max only, 0.9 dropped");

    let mut rows: Vec<(String, String, f64)> = names(&batch)
        .into_iter()
        .zip(units(&batch))
        .zip(values_f64(&batch))
        .map(|((n, u), v)| (n, u, v))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    assert_eq!(
        rows,
        vec![
            ("CPUUtilization_count".to_string(), "".to_string(), 42.0),
            (
                "CPUUtilization_max".to_string(),
                "Percent".to_string(),
                99.0
            ),
            ("CPUUtilization_min".to_string(), "Percent".to_string(), 1.0),
            (
                "CPUUtilization_sum".to_string(),
                "Percent".to_string(),
                123.5
            ),
        ]
    );
}

/// A `SummaryDataPoint` with only a `q=0.5` entry (no `q=0.0`/`q=1.0`) still produces
/// count/sum rows; the 0.5 quantile is dropped, not materialized as min or max.
#[tokio::test]
async fn summary_without_min_max_quantiles_still_produces_count_and_sum() {
    let dp = SummaryDataPoint {
        attributes: vec![],
        start_time_unix_nano: 0,
        time_unix_nano: 1_000,
        count: 7,
        sum: 10.0,
        quantile_values: vec![ValueAtQuantile {
            quantile: 0.5,
            value: 5.0,
        }],
        flags: 0,
    };
    let resource_metrics = resource_metrics_with_one_summary(dp);
    let payload_bytes = resource_metrics.encode_to_vec();

    let blob_storage = make_in_memory_blob_storage();
    let src_block = make_source_block(&blob_storage, payload_bytes, 1)
        .await
        .expect("make_source_block");

    let processor = OtelMetricsBlockProcessor {};
    let result = processor
        .process(blob_storage, src_block)
        .await
        .expect("process must not fail");
    let row_set = result.expect("expected Some(row_set)");
    let batch = row_set.rows;

    assert_eq!(batch.num_rows(), 2);
    let mut got_names = names(&batch);
    got_names.sort();
    assert_eq!(
        got_names,
        vec![
            "CPUUtilization_count".to_string(),
            "CPUUtilization_sum".to_string()
        ]
    );
}

/// A `SummaryDataPoint` with duplicate `q=0.0`/`q=1.0` entries must still produce exactly
/// one `_min` and one `_max` row (the first occurrence of each), not one per entry.
#[tokio::test]
async fn summary_with_duplicate_min_max_quantiles_emits_one_row_each() {
    let dp = SummaryDataPoint {
        attributes: vec![],
        start_time_unix_nano: 0,
        time_unix_nano: 1_000,
        count: 7,
        sum: 10.0,
        quantile_values: vec![
            ValueAtQuantile {
                quantile: 0.0,
                value: 1.0,
            },
            ValueAtQuantile {
                quantile: 0.0,
                value: 2.0,
            },
            ValueAtQuantile {
                quantile: 1.0,
                value: 9.0,
            },
            ValueAtQuantile {
                quantile: 1.0,
                value: 8.0,
            },
        ],
        flags: 0,
    };
    let resource_metrics = resource_metrics_with_one_summary(dp);
    let payload_bytes = resource_metrics.encode_to_vec();

    let blob_storage = make_in_memory_blob_storage();
    let src_block = make_source_block(&blob_storage, payload_bytes, 1)
        .await
        .expect("make_source_block");

    let processor = OtelMetricsBlockProcessor {};
    let result = processor
        .process(blob_storage, src_block)
        .await
        .expect("process must not fail");
    let row_set = result.expect("expected Some(row_set)");
    let batch = row_set.rows;

    assert_eq!(
        batch.num_rows(),
        4,
        "count/sum + one min + one max, duplicates dropped"
    );

    let mut rows: Vec<(String, f64)> = names(&batch).into_iter().zip(values_f64(&batch)).collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    assert_eq!(
        rows,
        vec![
            ("CPUUtilization_count".to_string(), 7.0),
            ("CPUUtilization_max".to_string(), 9.0),
            ("CPUUtilization_min".to_string(), 1.0),
            ("CPUUtilization_sum".to_string(), 10.0),
        ]
    );
}

/// `time_unix_nano == 0` skips the whole data point — zero rows, mirroring the existing
/// Sum/Gauge zero-timestamp skip.
#[tokio::test]
async fn summary_data_point_with_zero_timestamp_produces_no_rows() {
    let dp = SummaryDataPoint {
        attributes: vec![],
        start_time_unix_nano: 0,
        time_unix_nano: 0,
        count: 1,
        sum: 1.0,
        quantile_values: vec![
            ValueAtQuantile {
                quantile: 0.0,
                value: 1.0,
            },
            ValueAtQuantile {
                quantile: 1.0,
                value: 1.0,
            },
        ],
        flags: 0,
    };
    let resource_metrics = resource_metrics_with_one_summary(dp);
    let payload_bytes = resource_metrics.encode_to_vec();

    let blob_storage = make_in_memory_blob_storage();
    let src_block = make_source_block(&blob_storage, payload_bytes, 1)
        .await
        .expect("make_source_block");

    let processor = OtelMetricsBlockProcessor {};
    let result = processor
        .process(blob_storage, src_block)
        .await
        .expect("process must not fail");
    assert!(
        result.is_none(),
        "zero-timestamp data point should yield no rows at all"
    );
}
