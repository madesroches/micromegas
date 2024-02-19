use crate::lakehouse::span_table::SpanRow;

pub fn span_overlaps(span: &SpanRow, filter_begin_ms: f64, filter_end_ms: f64) -> bool {
    span.end_ms >= filter_begin_ms && span.begin_ms <= filter_end_ms
}
