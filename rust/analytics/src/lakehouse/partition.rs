use super::view::{ScanSortColumn, ViewMetadata};
use crate::time::TimeRange;
use chrono::{DateTime, Utc};

/// Partition metadata (without embedded file_metadata for performance)
/// Use load_partition_metadata() to load metadata on-demand when needed
#[derive(Clone, Debug)]
pub struct Partition {
    /// Metadata about the view this partition belongs to.
    pub view_metadata: ViewMetadata,
    /// The insert time range for this partition.
    pub insert_time_range: TimeRange,
    /// The event time range for this partition. None for empty partitions.
    pub event_time_range: Option<TimeRange>,
    /// The last time this partition was updated.
    pub updated: DateTime<Utc>,
    /// The path to the Parquet file for this partition. None for empty partitions.
    pub file_path: Option<String>,
    /// The size of the Parquet file in bytes. 0 for empty partitions.
    pub file_size: i64,
    /// A hash of the source data that generated this partition.
    pub source_data_hash: Vec<u8>,
    /// The number of rows in this partition. 0 for empty partitions.
    pub num_rows: i64,
    /// The sort guarantee this partition's rows carry, if any (e.g. `Some(["insert_time"])`).
    /// `None` means no ordering guarantee is recorded -- true for every partition written before
    /// this field existed, and for any view/merge that doesn't declare one. See
    /// `View::get_merged_partition_sort_order` and the `blocks_view_ordered_merges_plan.md` plan.
    pub sort_order: Option<Vec<String>>,
    /// The true maximum value of the view's declared `Concatenated` leading sort column across
    /// this partition's rows (`begin`, for `thread_spans`) -- as opposed to `max_event_time`,
    /// which for `thread_spans` is the max span *end* and only a loose stand-in for it. `None`
    /// means "not recorded": every partition written before this field existed, and every
    /// partition from a view that doesn't declare a `Concatenated` event-time ordering (only
    /// `ThreadSpansView` populates it today). See
    /// `tasks/thread_spans_segment_boundary_overlap_plan.md`, Part B.
    pub max_sort_key_time: Option<DateTime<Utc>>,
}

impl Partition {
    /// Returns true if this partition has no data (num_rows = 0).
    pub fn is_empty(&self) -> bool {
        self.num_rows == 0
    }

    /// Returns the min event time, if this partition has data.
    pub fn min_event_time(&self) -> Option<DateTime<Utc>> {
        self.event_time_range.as_ref().map(|r| r.begin)
    }

    /// Returns the max event time, if this partition has data.
    pub fn max_event_time(&self) -> Option<DateTime<Utc>> {
        self.event_time_range.as_ref().map(|r| r.end)
    }

    /// Returns the recorded true maximum leading-sort-column value, if any. See the field's doc
    /// for what "recorded" means and why it can be `None` even for a non-empty partition.
    pub fn max_sort_key_time(&self) -> Option<DateTime<Utc>> {
        self.max_sort_key_time
    }

    /// Returns the beginning of the insert time range.
    pub fn begin_insert_time(&self) -> DateTime<Utc> {
        self.insert_time_range.begin
    }

    /// Returns the end of the insert time range.
    pub fn end_insert_time(&self) -> DateTime<Utc> {
        self.insert_time_range.end
    }

    /// True when this partition's recorded `sort_order` certifies `columns` as an ordering its
    /// rows already satisfy: `columns` (ascending-only) must name a prefix of the recorded,
    /// ascending-implied sort order. Empty partitions certify vacuously -- there are no rows to
    /// violate the ordering. An empty (non-empty-partition) `columns` never certifies: it names no
    /// ordering to satisfy, so declaring it certified would let a `ScanOrdering::PerFile { columns:
    /// vec![] }` degrade to `Unordered` instead of silently planning and recording a vacuous
    /// ordering (see `ScanOrdering::PerFile`).
    pub fn certifies_sort_order(&self, columns: &[ScanSortColumn]) -> bool {
        if self.is_empty() {
            return true;
        }
        if columns.is_empty() {
            return false;
        }
        let Some(recorded) = &self.sort_order else {
            return false;
        };
        columns.len() <= recorded.len()
            && columns
                .iter()
                .zip(recorded.iter())
                .all(|(declared, recorded_col)| {
                    !declared.descending && *declared.column == *recorded_col
                })
    }

    /// Validates partition invariants. Returns error if partition is inconsistent.
    ///
    /// Invariants:
    /// - Non-empty partitions (num_rows > 0) MUST have both event_time_range and file_path
    /// - Empty partitions (num_rows = 0) MUST NOT have event_time_range or file_path
    /// - num_rows must not be negative
    /// - A partition with no event_time_range MUST NOT carry a max_sort_key_time (an empty
    ///   partition has no sort-key bound to record)
    ///
    /// Deliberately **not** checked here: any ordering relationship between `max_sort_key_time`
    /// and `min_event_time`/`max_event_time` (e.g. `min_event_time <= max_sort_key_time <=
    /// max_event_time`). That holds by construction for `thread_spans` today, but two properties
    /// of this function rule out asserting it as an invariant:
    ///
    /// 1. `validate` runs only on the read path (its callers are all in `partition_cache.rs`,
    ///    propagating with `?`); nothing on the write path calls it. A violating partition would
    ///    therefore be written and committed silently, then hard-fail *reads* -- and since the
    ///    fetch this feeds is not view-scoped, one bad row would fail materialization for every
    ///    view, not just the offending one. The invariant must assert only what the writer
    ///    guarantees by construction, never a hopeful expectation.
    /// 2. The bound would depend on data the writer doesn't control: `min_event_time`/
    ///    `max_event_time` derive straight from client-supplied block ticks with no validation
    ///    anywhere in ingestion or analytics, so a malformed client (e.g. `begin_ticks >
    ///    end_ticks`) could make an ordering clause unsatisfiable -- turning one bad row into a
    ///    lakehouse-wide read outage. And the invariant would apply to every future view
    ///    populating `max_sort_key_time`, not just `thread_spans`: `BlocksView`, for instance, has
    ///    a `Concatenated` sort key of `insert_time` while its `event_time_range` is `[min
    ///    begin_time, max insert_time]`, so a client clock skewed ahead of the server could put a
    ///    block's `begin_time` above its `insert_time`.
    ///
    /// The row-level property this plan actually relies on (the partition's true max `begin`
    /// really is bounded by its `event_time_range`) is asserted where it is cheap and safe
    /// instead: at write time (`ensure_begin_non_decreasing`) and pinned by a no-DB test
    /// (`call_tree_tests.rs`).
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.num_rows > 0 {
            // Non-empty partition must have event_time_range and file_path
            if self.event_time_range.is_none() {
                anyhow::bail!(
                    "non-empty partition (num_rows={}) has no event_time_range",
                    self.num_rows
                );
            }
            if self.file_path.is_none() {
                anyhow::bail!(
                    "non-empty partition (num_rows={}) has no file_path",
                    self.num_rows
                );
            }
        } else if self.num_rows == 0 {
            // Empty partition must NOT have event_time_range or file_path
            if self.event_time_range.is_some() {
                anyhow::bail!("empty partition has event_time_range");
            }
            if self.file_path.is_some() {
                anyhow::bail!("empty partition has file_path");
            }
        } else {
            anyhow::bail!("partition has negative num_rows: {}", self.num_rows);
        }
        if self.event_time_range.is_none() && self.max_sort_key_time.is_some() {
            anyhow::bail!("partition has no event_time_range but carries a max_sort_key_time");
        }
        Ok(())
    }
}
