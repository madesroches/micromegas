//! Structured per-query audit record for the FlightSQL service.
//!
//! `execute_query` (see `flight_sql_service_impl`) emits one JSON-serialized
//! [`QueryAuditRecord`] per query, at completion, under the dedicated
//! `flightsql_query_audit` log target. Unlike the untagged `imetric!` cost
//! metrics (whose `PropertySet` can't carry high-cardinality values such as
//! SQL text), a free-text log `msg` has no cardinality constraint, so it can
//! carry both attribution and cost in one self-contained, queryable record.
//!
//! `peak_memory_bytes`/`spilled_bytes`/`spill_count` complement the cost
//! fields above: the peak comes from the query's `ScopedMemoryPool` (see
//! `micromegas_analytics::lakehouse::scoped_memory_pool`), while the spill
//! counters are summed from the physical plan tree, same as `bytes_scanned`.

use datafusion::physical_plan::ExecutionPlan;

/// Aggregated DataFusion plan metrics for one query, read after the stream drains.
pub struct ScanMetrics {
    pub output_rows: Option<u64>,
    pub bytes_scanned: u64,
    pub spilled_bytes: u64,
    pub spill_count: u64,
}

/// Walk the physical-plan tree: `output_rows` from the root node (final result
/// grain), `bytes_scanned`/`spilled_bytes`/`spill_count` summed across every
/// node (leaf `DataSourceExec` nodes carry `bytes_scanned`; spilling operators
/// such as `ExternalSorter` carry the other two). `sum_by_name` cannot be used
/// for the spill counters: `MetricsSet::sum_by_name` explicitly returns
/// `false` for `MetricValue::SpillCount`/`SpilledBytes`, so it would silently
/// report zero; the dedicated `MetricsSet::spill_count()`/`spilled_bytes()`
/// accessors are used instead.
pub fn aggregate_scan_metrics(plan: &dyn ExecutionPlan) -> ScanMetrics {
    fn sum_bytes(plan: &dyn ExecutionPlan) -> u64 {
        let mut total = plan
            .metrics()
            .and_then(|m| m.sum_by_name("bytes_scanned"))
            .map(|v| v.as_usize() as u64)
            .unwrap_or(0);
        for child in plan.children() {
            total += sum_bytes(child.as_ref());
        }
        total
    }
    fn sum_spills(plan: &dyn ExecutionPlan) -> (u64, u64) {
        let metrics = plan.metrics();
        let mut spilled_bytes = metrics
            .as_ref()
            .and_then(|m| m.spilled_bytes())
            .map(|v| v as u64)
            .unwrap_or(0);
        let mut spill_count = metrics
            .as_ref()
            .and_then(|m| m.spill_count())
            .map(|v| v as u64)
            .unwrap_or(0);
        for child in plan.children() {
            let (child_bytes, child_count) = sum_spills(child.as_ref());
            spilled_bytes += child_bytes;
            spill_count += child_count;
        }
        (spilled_bytes, spill_count)
    }
    let (spilled_bytes, spill_count) = sum_spills(plan);
    ScanMetrics {
        output_rows: plan
            .metrics()
            .and_then(|m| m.output_rows())
            .map(|r| r as u64),
        bytes_scanned: sum_bytes(plan),
        spilled_bytes,
        spill_count,
    }
}

/// One structured record per FlightSQL query, emitted as a JSON log line under
/// the `flightsql_query_audit` target when the query completes, fails, or is
/// abandoned mid-drain (client disconnect/cancel).
#[derive(serde::Serialize)]
pub struct QueryAuditRecord {
    /// Minted as the first statement of `execute_query`, before any fallible
    /// step -- always present, so a failure's server-log line (which also
    /// carries it) and this record can be correlated by grepping the id.
    pub query_id: String,
    /// The rightmost `X-Forwarded-For` entry (the address the trusted proxy in front of this
    /// service observed), falling back to `X-Real-IP` and then the gRPC peer address for a
    /// direct connection -- see `http_utils::get_client_ip`. Unlike `client`/`agent`/`entrypoint`
    /// below, this comes from the network / trusted-proxy layer rather than from
    /// client-controlled attribution headers, so it's grouped with `query_id` rather than with
    /// the self-reported fields -- but that guarantee holds only for requests that actually
    /// traversed the trusted proxy; for a direct connection with no proxy in front, both
    /// `X-Forwarded-For` and `X-Real-IP` are just as caller-chosen as the self-reported fields.
    pub client_ip: String,
    pub client: String,
    pub agent: String,
    pub entrypoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// Originating notebook name, if the query was issued from a notebook cell.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notebook: Option<String>,
    /// Originating cell name within the notebook, if the query was issued from a notebook cell.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cell: Option<String>,
    pub user: String,
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub service_account: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_account_name: Option<String>,
    pub sql: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range_begin: Option<String>, // RFC3339
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    pub context_init_ms: f64,
    pub planning_ms: f64,
    pub execution_ms: f64, // stream construction (matches query_execution_duration semantics)
    pub setup_ms: f64,     // parse+attribution+context+planning+stream-build (query_setup_duration)
    pub total_ms: f64,     // end-to-end incl. drain
    pub status: &'static str, // "ok" | "error" | "incomplete"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// "user" | "resource" | "internal" | "denied", derived from the gRPC status code
    /// (see `flight_sql_service_impl::error_class`) -- "denied" is stamped directly by the
    /// query-deny-list check rather than derived from a `DataFusionError`. Omitted when
    /// `status == "ok"`, matching the `error` field's own "on error" convention.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_class: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_rows: Option<u64>,
    pub bytes_scanned: u64,
    pub peak_memory_bytes: u64,
    pub spilled_bytes: u64,
    pub spill_count: u64,
    /// Normalized SQL fingerprint -- the first 16 hex chars of the SHA-256 of the
    /// literal-stripped, whitespace-collapsed token stream. Computed once
    /// per query and emitted on every terminal path regardless of whether the deny list is in
    /// active use, since it's what an operator pastes into `deny_queries` after finding an
    /// offender in the audit log. Appended last so existing JSON consumers are unaffected.
    pub sql_hash: String,
}
