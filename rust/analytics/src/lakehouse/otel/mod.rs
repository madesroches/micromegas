/// Helpers for OTel attribute / `AnyValue` → JSONB translation
pub mod attrs;
/// Implementation of `BlockProcessor` for OTel logs
pub mod logs_block_processor;
/// Implementation of `BlockProcessor` for OTel metrics (Sum / Gauge → measures)
pub mod metrics_block_processor;
/// Implementation of `BlockProcessor` for OTel spans
pub mod spans_block_processor;
/// Arrow schema for the otel_spans view
pub mod spans_table;
/// JIT-only per-process view of OTel spans
pub mod spans_view;
