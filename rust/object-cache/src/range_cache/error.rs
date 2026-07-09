use micromegas_tracing::prelude::*;

/// Errors returned by [`RangeCache`] that callers may want to handle distinctly.
#[derive(Debug, thiserror::Error)]
pub enum RangeError {
    /// The requested range extends past the end of the object.
    #[error("requested range end {requested_end} exceeds object size {file_size}")]
    OutOfBounds { requested_end: u64, file_size: u64 },
}

/// Which caller is invoking `RangeCache::stream_ranges`, selecting which of
/// the two distinct `range_cache_get_range_error` / `range_cache_get_ranges_error`
/// counters is emitted on an upfront validation failure or a mid-stream fetch
/// error. `stream_ranges` is the single fill path behind both `get_range`/
/// `get_range_handler` (`Range`) and `get_ranges`/`post_ranges_handler`
/// (`Ranges`), so the tag keeps the two metric names emitting exactly as they
/// did before those call sites shared one implementation.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StreamRangesCaller {
    Range,
    Ranges,
}

impl StreamRangesCaller {
    pub(super) fn emit_error_metric(self) {
        match self {
            StreamRangesCaller::Range => imetric!("range_cache_get_range_error", "count", 1_u64),
            StreamRangesCaller::Ranges => {
                imetric!("range_cache_get_ranges_error", "count", 1_u64)
            }
        }
    }
}
