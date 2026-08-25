//! The single `micromegas.audience` property name and SQL fragment shared by the writer
//! (`lakehouse::blocks_view`) and both enforcement prongs (`lakehouse::audience_guard`,
//! `lakehouse::ownership_rewrite`) -- and by `metadata.rs`, the JIT / per-process path (#1482
//! §1).

/// The reserved process property key an ingestion credential's audience is stamped under.
/// Re-exported here (rather than each consumer importing `micromegas_telemetry` directly) so the
/// property name stays single-sourced across the write side and both readers.
pub use micromegas_telemetry::property_names::PROPERTY_AUDIENCE as AUDIENCE_PROPERTY;

/// Builds `(SELECT value FROM unnest(<properties_expr>) WHERE key = '<AUDIENCE_PROPERTY>' LIMIT 1)`
/// -- the correlated scalar subselect that extracts a process's audience out of its
/// `micromegas_property[]` properties array. `properties_expr` is inlined as SQL text (not
/// bound), so it must be a trusted column reference, never user input.
pub fn audience_subselect(properties_expr: &str) -> String {
    format!(
        "(SELECT value FROM unnest({properties_expr}) WHERE key = '{AUDIENCE_PROPERTY}' LIMIT 1)"
    )
}
