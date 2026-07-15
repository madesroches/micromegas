//! Structured per-query audit record for the FlightSQL service.
//!
//! `execute_query` (see `flight_sql_service_impl`) emits one JSON-serialized
//! [`QueryAuditRecord`] per query, at completion, under the dedicated
//! `flightsql_query_audit` log target. Unlike the untagged `imetric!` cost
//! metrics (whose `PropertySet` can't carry high-cardinality values such as
//! SQL text), a free-text log `msg` has no cardinality constraint, so it can
//! carry both attribution and cost in one self-contained, queryable record.

use datafusion::physical_plan::ExecutionPlan;

/// Aggregated DataFusion plan metrics for one query, read after the stream drains.
pub struct ScanMetrics {
    pub output_rows: Option<u64>,
    pub bytes_scanned: u64,
}

/// Walk the physical-plan tree: `output_rows` from the root node (final result
/// grain), `bytes_scanned` summed across every node (leaf `DataSourceExec`
/// nodes carry it).
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
    ScanMetrics {
        output_rows: plan
            .metrics()
            .and_then(|m| m.output_rows())
            .map(|r| r as u64),
        bytes_scanned: sum_bytes(plan),
    }
}

/// One structured record per FlightSQL query, emitted at completion (success or
/// error) as a JSON log line under the `flightsql_query_audit` target.
#[derive(serde::Serialize)]
pub struct QueryAuditRecord<'a> {
    pub client: &'a str,
    pub user: &'a str,
    pub email: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'a str>,
    pub service_account: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_account_name: Option<&'a str>,
    pub sql: &'a str,
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
    pub status: &'static str, // "ok" | "error"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_rows: Option<u64>,
    pub bytes_scanned: u64,
}
