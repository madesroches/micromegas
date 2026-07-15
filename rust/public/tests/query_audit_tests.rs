// Unit tests for `micromegas::servers::query_audit`:
// - `QueryAuditRecord` JSON serialization (required fields, `skip_serializing_if`
//   omission of absent optionals, SQL text containing `{`/`}` and quotes
//   round-tripping through `serde_json`).
// - `aggregate_scan_metrics` walking a hand-built physical plan tree.

use micromegas::datafusion::arrow::datatypes::Schema;
use micromegas::datafusion::error::Result as DataFusionResult;
use micromegas::datafusion::execution::TaskContext;
use micromegas::datafusion::physical_expr::EquivalenceProperties;
use micromegas::datafusion::physical_plan::execution_plan::{Boundedness, EmissionType};
use micromegas::datafusion::physical_plan::metrics::{
    ExecutionPlanMetricsSet, MetricBuilder, MetricsSet,
};
use micromegas::datafusion::physical_plan::{
    DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PlanProperties,
    SendableRecordBatchStream,
};
use micromegas::servers::query_audit::{QueryAuditRecord, aggregate_scan_metrics};
use std::sync::Arc;

/// Minimal `ExecutionPlan` used only to exercise `aggregate_scan_metrics`'s
/// tree walk. Only `metrics` and `children` are ever called by the code under
/// test; every other method is unreachable in these tests.
#[derive(Debug)]
struct FakeExec {
    children: Vec<Arc<dyn ExecutionPlan>>,
    metrics: ExecutionPlanMetricsSet,
    properties: Arc<PlanProperties>,
}

fn fake_properties() -> Arc<PlanProperties> {
    let schema = Arc::new(Schema::empty());
    Arc::new(PlanProperties::new(
        EquivalenceProperties::new(schema),
        Partitioning::UnknownPartitioning(1),
        EmissionType::Incremental,
        Boundedness::Bounded,
    ))
}

impl FakeExec {
    fn leaf(bytes_scanned: usize) -> Arc<dyn ExecutionPlan> {
        let metrics = ExecutionPlanMetricsSet::new();
        MetricBuilder::new(&metrics)
            .counter("bytes_scanned", 0)
            .add(bytes_scanned);
        Arc::new(Self {
            children: vec![],
            metrics,
            properties: fake_properties(),
        })
    }

    fn node(
        children: Vec<Arc<dyn ExecutionPlan>>,
        output_rows: Option<usize>,
    ) -> Arc<dyn ExecutionPlan> {
        let metrics = ExecutionPlanMetricsSet::new();
        if let Some(rows) = output_rows {
            MetricBuilder::new(&metrics).output_rows(0).add(rows);
        }
        Arc::new(Self {
            children,
            metrics,
            properties: fake_properties(),
        })
    }

    fn empty() -> Arc<dyn ExecutionPlan> {
        Arc::new(Self {
            children: vec![],
            metrics: ExecutionPlanMetricsSet::new(),
            properties: fake_properties(),
        })
    }
}

impl DisplayAs for FakeExec {
    fn fmt_as(&self, _t: DisplayFormatType, _f: &mut std::fmt::Formatter) -> std::fmt::Result {
        unimplemented!("not exercised by aggregate_scan_metrics tests")
    }
}

impl ExecutionPlan for FakeExec {
    fn name(&self) -> &str {
        "FakeExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        self.children.iter().collect()
    }

    fn with_new_children(
        self: Arc<Self>,
        _children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> DataFusionResult<Arc<dyn ExecutionPlan>> {
        unimplemented!("not exercised by aggregate_scan_metrics tests")
    }

    fn execute(
        &self,
        _partition: usize,
        _context: Arc<TaskContext>,
    ) -> DataFusionResult<SendableRecordBatchStream> {
        unimplemented!("not exercised by aggregate_scan_metrics tests")
    }

    fn metrics(&self) -> Option<MetricsSet> {
        Some(self.metrics.clone_inner())
    }
}

#[test]
fn aggregate_scan_metrics_sums_bytes_across_the_tree_and_reads_output_rows_from_root() {
    // root(output_rows=42) -> [leaf(bytes_scanned=100), leaf(bytes_scanned=250)]
    let left = FakeExec::leaf(100);
    let right = FakeExec::leaf(250);
    let root = FakeExec::node(vec![left, right], Some(42));

    let scan = aggregate_scan_metrics(root.as_ref());

    assert_eq!(scan.output_rows, Some(42));
    assert_eq!(scan.bytes_scanned, 350);
}

#[test]
fn aggregate_scan_metrics_on_empty_metrics_plan_reports_none_and_zero() {
    let plan = FakeExec::empty();

    let scan = aggregate_scan_metrics(plan.as_ref());

    assert_eq!(scan.output_rows, None);
    assert_eq!(scan.bytes_scanned, 0);
}

#[test]
fn aggregate_scan_metrics_ignores_output_rows_on_non_root_nodes() {
    // output_rows is only read from the root; a child's output_rows metric
    // must not leak into the aggregated result.
    let leaf = FakeExec::node(vec![], Some(7));
    let root = FakeExec::node(vec![leaf], None);

    let scan = aggregate_scan_metrics(root.as_ref());

    assert_eq!(scan.output_rows, None);
}

fn full_record(sql: &str) -> QueryAuditRecord {
    QueryAuditRecord {
        client: "python".to_string(),
        user: "alice".to_string(),
        email: "alice@example.com".to_string(),
        name: Some("Alice".to_string()),
        service_account: false,
        service_account_name: None,
        sql: sql.to_string(),
        range_begin: Some("2024-01-01T00:00:00+00:00".to_string()),
        range_end: Some("2024-01-02T00:00:00+00:00".to_string()),
        limit: Some(100),
        context_init_ms: 1.5,
        planning_ms: 2.5,
        execution_ms: 3.5,
        setup_ms: 7.5,
        total_ms: 42.0,
        status: "ok",
        error: None,
        output_rows: Some(123),
        bytes_scanned: 4096,
    }
}

#[test]
fn query_audit_record_serializes_required_fields() {
    let record = full_record("SELECT 1");
    let json = serde_json::to_string(&record).expect("serialization should succeed");
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");

    assert_eq!(value["client"], "python");
    assert_eq!(value["user"], "alice");
    assert_eq!(value["email"], "alice@example.com");
    assert_eq!(value["name"], "Alice");
    assert_eq!(value["service_account"], false);
    assert_eq!(value["sql"], "SELECT 1");
    assert_eq!(value["range_begin"], "2024-01-01T00:00:00+00:00");
    assert_eq!(value["range_end"], "2024-01-02T00:00:00+00:00");
    assert_eq!(value["limit"], 100);
    assert_eq!(value["context_init_ms"], 1.5);
    assert_eq!(value["planning_ms"], 2.5);
    assert_eq!(value["execution_ms"], 3.5);
    assert_eq!(value["setup_ms"], 7.5);
    assert_eq!(value["total_ms"], 42.0);
    assert_eq!(value["status"], "ok");
    assert_eq!(value["output_rows"], 123);
    assert_eq!(value["bytes_scanned"], 4096);
}

#[test]
fn query_audit_record_omits_absent_optionals() {
    let record = QueryAuditRecord {
        client: "grpc".to_string(),
        user: "unknown".to_string(),
        email: "unknown".to_string(),
        name: None,
        service_account: true,
        service_account_name: Some("svc-ci".to_string()),
        sql: "SELECT 2".to_string(),
        range_begin: None,
        range_end: None,
        limit: None,
        context_init_ms: 0.0,
        planning_ms: 0.0,
        execution_ms: 0.0,
        setup_ms: 0.0,
        total_ms: 1.0,
        status: "error",
        error: Some("boom".to_string()),
        output_rows: None,
        bytes_scanned: 0,
    };

    let json = serde_json::to_string(&record).expect("serialization should succeed");
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    let object = value
        .as_object()
        .expect("record serializes as a JSON object");

    assert!(!object.contains_key("name"));
    assert!(!object.contains_key("range_begin"));
    assert!(!object.contains_key("range_end"));
    assert!(!object.contains_key("limit"));
    assert!(!object.contains_key("output_rows"));
    assert_eq!(value["service_account"], true);
    assert_eq!(value["service_account_name"], "svc-ci");
    assert_eq!(value["status"], "error");
    assert_eq!(value["error"], "boom");
    assert_eq!(value["bytes_scanned"], 0);
}

#[test]
fn query_audit_record_round_trips_sql_with_braces_and_quotes() {
    let sql = r#"SELECT jsonb_get(msg, 'key') FROM t WHERE msg = '{"a": 1}' -- {comment}"#;
    let record = full_record(sql);

    let json = serde_json::to_string(&record).expect("serialization should succeed");
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");

    assert_eq!(value["sql"], sql);
}
