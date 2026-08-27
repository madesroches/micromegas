//! Unit tests for `cast_to_file_schema`, the type alignment between a Postgres-inferred batch and
//! the declared file schema (#1482 §1). `sql_arrow_bridge` maps a `TEXT` column to plain `Utf8`
//! because its mapping is keyed on the Postgres type name alone, so `blocks.audience` -- declared
//! `Dictionary(Int32, Utf8)` -- only reaches the parquet writer as its declared type through this
//! function.

use datafusion::arrow::array::{Array, ArrayRef, Int64Array, StringArray};
use datafusion::arrow::datatypes::{DataType, Field, Int32Type, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use micromegas_analytics::lakehouse::metadata_partition_spec::{
    cast_to_file_schema, mismatch_excluded_count,
};
use std::sync::Arc;

fn audience_dictionary_type() -> DataType {
    DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
}

/// A batch shaped like `rows_to_record_batch`'s output for the tail of the `blocks` query: a
/// plain column followed by the `TEXT` audience subselect result.
fn inferred_batch() -> RecordBatch {
    let payload_size: ArrayRef = Arc::new(Int64Array::from(vec![Some(11), Some(22), Some(33)]));
    let audience: ArrayRef = Arc::new(StringArray::from(vec![
        Some("public"),
        Some("public"),
        Some("team-alpha"),
    ]));
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("payload_size", DataType::Int64, true),
            Field::new("audience", DataType::Utf8, true),
        ])),
        vec![payload_size, audience],
    )
    .unwrap()
}

#[test]
fn a_utf8_column_is_cast_to_the_declared_dictionary_type() {
    let file_schema = Schema::new(vec![
        Field::new("payload_size", DataType::Int64, false),
        Field::new("audience", audience_dictionary_type(), false),
    ]);
    let batch = cast_to_file_schema(inferred_batch(), &file_schema).unwrap();
    assert_eq!(batch.num_rows(), 3);
    assert_eq!(batch.column(0).data_type(), &DataType::Int64);
    assert_eq!(batch.column(1).data_type(), &audience_dictionary_type());
    let values = batch
        .column(1)
        .as_any()
        .downcast_ref::<datafusion::arrow::array::DictionaryArray<Int32Type>>()
        .expect("the cast column must be a Dictionary(Int32, _)");
    let strings = values
        .values()
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("dictionary values must be Utf8");
    let read: Vec<&str> = (0..values.len())
        .map(|i| strings.value(values.keys().value(i) as usize))
        .collect();
    assert_eq!(read, vec!["public", "public", "team-alpha"]);
}

/// Nullability comes from the batch, never from the declared schema: the cast must not smuggle a
/// `NOT NULL` claim onto a column that carries nulls -- that verdict belongs to
/// `write_partition::check_non_nullable_columns`, which would otherwise never see the null.
#[test]
fn declared_non_nullability_is_not_applied_to_the_batch() {
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new(
            "audience",
            DataType::Utf8,
            true,
        )])),
        vec![Arc::new(StringArray::from(vec![Some("public"), None])) as ArrayRef],
    )
    .unwrap();
    let file_schema = Schema::new(vec![Field::new(
        "audience",
        audience_dictionary_type(),
        false,
    )]);
    let casted = cast_to_file_schema(batch, &file_schema).unwrap();
    assert!(casted.schema().field(0).is_nullable());
    assert_eq!(casted.column(0).null_count(), 1);
}

/// The common case: every type already agrees, so the batch is returned untouched.
#[test]
fn a_matching_schema_is_a_no_op() {
    let batch = inferred_batch();
    let file_schema = Schema::new(vec![
        Field::new("payload_size", DataType::Int64, false),
        Field::new("audience", DataType::Utf8, false),
    ]);
    let casted = cast_to_file_schema(batch.clone(), &file_schema).unwrap();
    assert_eq!(casted.schema(), batch.schema());
}

/// Positional, and tolerant of a declared schema that is shorter than the batch (the parquet
/// writer zips the two positionally with no name check either): the extra column passes through.
#[test]
fn columns_beyond_the_declared_schema_pass_through() {
    let file_schema = Schema::new(vec![Field::new("payload_size", DataType::Int64, false)]);
    let casted = cast_to_file_schema(inferred_batch(), &file_schema).unwrap();
    assert_eq!(casted.column(1).data_type(), &DataType::Utf8);
}

// --- `mismatch_excluded_count`: the arithmetic behind `MetadataPartitionSpec::write`'s
// per-partition `warn!`/`imetric!("block_audience_mismatch_excluded", ...)` pair, tested
// directly rather than only through those side effects.

#[test]
fn mismatch_excluded_count_is_zero_when_counts_agree() {
    assert_eq!(mismatch_excluded_count(42, 42), 0);
    assert_eq!(mismatch_excluded_count(0, 0), 0);
}

#[test]
fn mismatch_excluded_count_is_the_difference_when_they_disagree() {
    assert_eq!(mismatch_excluded_count(10, 7), 3);
    assert_eq!(mismatch_excluded_count(1, 0), 1);
}

#[test]
fn mismatch_excluded_count_clamps_at_zero_rather_than_going_negative() {
    // Should not occur given the single atomic query the two counts come from, but the helper's
    // contract clamps regardless -- a defensive floor, not a case this plan expects to exercise.
    assert_eq!(mismatch_excluded_count(5, 10), 0);
}
