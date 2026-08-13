//! Pure, no-DB unit tests for `spec_is_up_to_date` and `PartitionFreshnessRow`
//! (tasks/jit_batched_block_queries_plan.md, Testing Strategy): table-driven over the three
//! `BlockOrder`/range-shape variants the original per-spec SQL used to express
//! (`jit_partitions.rs`'s `is_jit_partition_up_to_date` docs), asserting `spec_is_up_to_date`
//! reproduces that per-variant semantics from a candidate set built directly (no live database).

use chrono::{DateTime, TimeDelta, Utc};
use micromegas_analytics::lakehouse::jit_partitions::{
    BlockOrder, PartitionFreshnessRow, spec_is_up_to_date,
};
use micromegas_analytics::lakehouse::partition_source_data::{
    PartitionSourceBlock, SourceDataBlocksInMemory,
};
use micromegas_analytics::lakehouse::view::ViewMetadata;
use micromegas_analytics::metadata::{ProcessMetadata, StreamMetadata};
use micromegas_telemetry::types::block::BlockMetadata;
use std::sync::Arc;

fn base_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
        .unwrap()
        .to_utc()
}

fn make_block(insert_time: DateTime<Utc>, nb_objects: i32) -> Arc<PartitionSourceBlock> {
    let process_id = uuid::Uuid::new_v4();
    let stream_id = uuid::Uuid::new_v4();
    let block = BlockMetadata {
        block_id: uuid::Uuid::new_v4(),
        stream_id,
        process_id,
        begin_time: insert_time,
        end_time: insert_time,
        begin_ticks: 0,
        end_ticks: 100,
        nb_objects,
        payload_size: 0,
        object_offset: 0,
        insert_time,
    };
    let stream = Arc::new(StreamMetadata {
        process_id,
        stream_id,
        dependencies_metadata: vec![],
        objects_metadata: vec![],
        tags: vec![],
        properties: Arc::new(vec![]),
    });
    let process = Arc::new(ProcessMetadata {
        process_id,
        exe: "test".to_owned(),
        username: "test".to_owned(),
        realname: "test".to_owned(),
        computer: "test".to_owned(),
        distro: "test".to_owned(),
        cpu_brand: "test".to_owned(),
        tsc_frequency: 1_000_000_000,
        start_time: insert_time,
        start_ticks: 0,
        parent_process_id: None,
        properties: Arc::new(vec![]),
    });
    Arc::new(PartitionSourceBlock {
        block,
        stream,
        process,
        format: "test".to_owned(),
    })
}

/// A spec with a single block at `insert_time` and `nb_objects` objects: a degenerate insert range
/// (`min == max == insert_time`), the simplest shape that exercises every branch below.
fn make_degenerate_spec(insert_time: DateTime<Utc>, nb_objects: i32) -> SourceDataBlocksInMemory {
    SourceDataBlocksInMemory {
        blocks: vec![make_block(insert_time, nb_objects)],
        block_ids_hash: (nb_objects as i64).to_le_bytes().to_vec(),
    }
}

/// A spec spanning `[min_insert_time, max_insert_time]` (two blocks, one at each endpoint) with a
/// given total object count.
fn make_ranged_spec(
    min_insert_time: DateTime<Utc>,
    max_insert_time: DateTime<Utc>,
    total_nb_objects: i64,
) -> SourceDataBlocksInMemory {
    SourceDataBlocksInMemory {
        blocks: vec![
            make_block(min_insert_time, (total_nb_objects / 2) as i32),
            make_block(
                max_insert_time,
                (total_nb_objects - total_nb_objects / 2) as i32,
            ),
        ],
        block_ids_hash: total_nb_objects.to_le_bytes().to_vec(),
    }
}

fn view_meta() -> ViewMetadata {
    ViewMetadata {
        view_set_name: Arc::new("test_view".to_owned()),
        view_instance_id: Arc::new("test_instance".to_owned()),
        file_schema_hash: vec![1, 2, 3],
    }
}

fn row(
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    file_schema_hash: Vec<u8>,
    existing_count: i64,
) -> PartitionFreshnessRow {
    PartitionFreshnessRow {
        begin_insert_time: begin,
        end_insert_time: end,
        file_schema_hash,
        source_data_hash: existing_count.to_le_bytes().to_vec(),
    }
}

/// No candidates at all: `rows.len() != 1`, always reports not up to date, for every variant.
#[test]
fn no_candidates_reports_not_up_to_date() {
    let vm = view_meta();
    let t0 = base_time();
    let spec = make_degenerate_spec(t0, 5);
    for order in [BlockOrder::EventTime, BlockOrder::InsertTime] {
        let up_to_date = spec_is_up_to_date(&vm, &spec, order, &[]).unwrap();
        assert!(
            !up_to_date,
            "{order:?}: no candidates must never be up to date"
        );
    }
}

/// `BlockOrder::EventTime` requires *exact* insert-range equality: a row that merely overlaps
/// (wider on either side) must not match, even though its object count would otherwise satisfy
/// the exact-equality check.
#[test]
fn event_time_requires_exact_range_equality() {
    let vm = view_meta();
    let t0 = base_time();
    let spec = make_ranged_spec(t0, t0 + TimeDelta::seconds(10), 20);

    // Exact match: up to date.
    let exact = row(
        t0,
        t0 + TimeDelta::seconds(10),
        vm.file_schema_hash.clone(),
        20,
    );
    assert!(spec_is_up_to_date(&vm, &spec, BlockOrder::EventTime, &[exact]).unwrap());

    // Wider overlapping row (starts earlier): must not match under EventTime.
    let wider = row(
        t0 - TimeDelta::seconds(5),
        t0 + TimeDelta::seconds(10),
        vm.file_schema_hash.clone(),
        30,
    );
    assert!(!spec_is_up_to_date(&vm, &spec, BlockOrder::EventTime, &[wider]).unwrap());
}

/// `BlockOrder::InsertTime` (non-degenerate range) uses an inclusive overlap test: a row that
/// merely overlaps (and is wider) still matches, and is up to date as long as its object count is
/// `>=` the spec's required count -- not exact equality.
#[test]
fn insert_time_overlap_matches_a_wider_row_with_count_at_least() {
    let vm = view_meta();
    let t0 = base_time();
    let spec = make_ranged_spec(t0, t0 + TimeDelta::seconds(10), 20);

    // A wider overlapping row with a higher count: up to date (>=, not exact equality).
    let wider = row(
        t0 - TimeDelta::seconds(5),
        t0 + TimeDelta::seconds(20),
        vm.file_schema_hash.clone(),
        50,
    );
    assert!(spec_is_up_to_date(&vm, &spec, BlockOrder::InsertTime, &[wider]).unwrap());

    // A non-overlapping row (entirely before the spec's range): not up to date (rows.len() == 0).
    let disjoint = row(
        t0 - TimeDelta::seconds(100),
        t0 - TimeDelta::seconds(50),
        vm.file_schema_hash.clone(),
        50,
    );
    assert!(!spec_is_up_to_date(&vm, &spec, BlockOrder::InsertTime, &[disjoint]).unwrap());
}

/// `BlockOrder::InsertTime` with a *degenerate* spec range (`min == max`) narrows to an
/// exact-point match: an overlapping-but-not-exact row (even one containing the point) must not
/// match, to avoid picking a wider row that a later, non-degenerate write could also match.
#[test]
fn insert_time_degenerate_range_requires_an_exact_point_match() {
    let vm = view_meta();
    let t0 = base_time();
    let spec = make_degenerate_spec(t0, 5);

    // Exact point match: up to date.
    let exact = row(t0, t0, vm.file_schema_hash.clone(), 5);
    assert!(spec_is_up_to_date(&vm, &spec, BlockOrder::InsertTime, &[exact]).unwrap());

    // A wider row that merely contains the point: must not match for a degenerate spec.
    let containing = row(
        t0 - TimeDelta::seconds(5),
        t0 + TimeDelta::seconds(5),
        vm.file_schema_hash.clone(),
        100,
    );
    assert!(!spec_is_up_to_date(&vm, &spec, BlockOrder::InsertTime, &[containing]).unwrap());
}

/// More than one matching candidate row is ambiguous: not up to date, regardless of variant.
#[test]
fn multiple_matching_rows_reports_not_up_to_date() {
    let vm = view_meta();
    let t0 = base_time();
    let spec = make_degenerate_spec(t0, 5);
    let a = row(t0, t0, vm.file_schema_hash.clone(), 5);
    let b = row(t0, t0, vm.file_schema_hash.clone(), 5);
    let up_to_date = spec_is_up_to_date(&vm, &spec, BlockOrder::InsertTime, &[a, b]).unwrap();
    assert!(
        !up_to_date,
        "two exact-matching rows for one spec is ambiguous"
    );
}

/// A single matching row with a different `file_schema_hash` is never up to date, regardless of
/// object counts -- avoids silently reusing a partition written under an old file schema.
#[test]
fn schema_hash_mismatch_reports_not_up_to_date() {
    let vm = view_meta();
    let t0 = base_time();
    let spec = make_degenerate_spec(t0, 5);
    let mismatched = row(t0, t0, vec![9, 9, 9], 5);
    let up_to_date = spec_is_up_to_date(&vm, &spec, BlockOrder::InsertTime, &[mismatched]).unwrap();
    assert!(!up_to_date);
}

/// Object-count boundary cases: below the required count is always stale; exactly equal is always
/// up to date; strictly above is up to date under `InsertTime` (`>=`) but stale under `EventTime`
/// (exact equality only).
#[test]
fn object_count_boundary_cases() {
    let vm = view_meta();
    let t0 = base_time();
    let required = 10i64;
    let spec = make_degenerate_spec(t0, required as i32);

    for (existing, expect_insert_time_up_to_date, expect_event_time_up_to_date) in [
        (required - 1, false, false),
        (required, true, true),
        (required + 1, true, false),
    ] {
        let r = row(t0, t0, vm.file_schema_hash.clone(), existing);
        let insert_time_result =
            spec_is_up_to_date(&vm, &spec, BlockOrder::InsertTime, std::slice::from_ref(&r))
                .unwrap();
        assert_eq!(
            insert_time_result, expect_insert_time_up_to_date,
            "InsertTime: existing={existing}, required={required}"
        );
        let event_time_result =
            spec_is_up_to_date(&vm, &spec, BlockOrder::EventTime, std::slice::from_ref(&r))
                .unwrap();
        assert_eq!(
            event_time_result, expect_event_time_up_to_date,
            "EventTime: existing={existing}, required={required}"
        );
    }
}
