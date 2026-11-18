use anyhow::Result;
use arrow::array::{Int32Array, RecordBatch};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use chrono::Utc;
use futures::stream::BoxStream;
use futures::{StreamExt, stream};
use micromegas::client::flightsql_client::QueryEstimate;
use micromegas::servers::mcp::config::McpConfig;
use micromegas::servers::mcp::flight::FlightBackend;
use micromegas::servers::mcp::provider::{CallContext, ToolProvider};
use micromegas::servers::mcp::tools::{QueryToolProvider, parse_range, parse_time};
use micromegas_auth::types::{AuthContext, AuthType};
use micromegas_telemetry::time_range::TimeRange;
use rmcp::model::{CallToolResult, ContentBlock, JsonObject};
use std::sync::{Arc, Mutex};

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new("n", DataType::Int32, false)]))
}

type Estimate = (Option<u64>, Option<String>, Option<bool>);

#[derive(Default)]
struct FakeBackend {
    estimate: Mutex<Option<Estimate>>,
    prepare_error: Mutex<Option<tonic::Status>>,
    execute_error: Mutex<Option<tonic::Status>>,
    rows: usize,
    seen: Mutex<Vec<(String, Option<TimeRange>, usize)>>,
}

#[async_trait]
impl FlightBackend for FakeBackend {
    async fn prepare(
        &self,
        _call: &CallContext,
        sql: &str,
        range: Option<TimeRange>,
    ) -> Result<QueryEstimate> {
        self.seen
            .lock()
            .expect("lock")
            .push((sql.to_string(), range, 0));
        if let Some(status) = self.prepare_error.lock().expect("lock").clone() {
            return Err(status.into());
        }
        let (total_bytes, estimate_precision, within_scan_cap) = self
            .estimate
            .lock()
            .expect("lock")
            .clone()
            .unwrap_or((None, None, None));
        Ok(QueryEstimate {
            schema: schema(),
            total_bytes,
            estimate_precision,
            within_scan_cap,
        })
    }

    async fn execute(
        &self,
        _call: &CallContext,
        sql: &str,
        range: Option<TimeRange>,
        row_limit: usize,
    ) -> Result<BoxStream<'static, Result<RecordBatch>>> {
        self.seen
            .lock()
            .expect("lock")
            .push((sql.to_string(), range, row_limit));
        if let Some(status) = self.execute_error.lock().expect("lock").clone() {
            return Err(status.into());
        }
        let n = self.rows.min(row_limit);
        let batch = RecordBatch::try_new(
            schema(),
            vec![Arc::new(Int32Array::from_iter_values(0..n as i32))],
        )
        .expect("batch");
        Ok(stream::iter(vec![Ok(batch)]).boxed())
    }
}

fn ctx() -> CallContext {
    CallContext {
        auth: AuthContext {
            subject: "s".into(),
            email: None,
            issuer: "i".into(),
            audience: None,
            expires_at: None,
            auth_type: AuthType::ApiKey,
            allow_delegation: false,
            bound_audience: None,
            read_audiences: vec![],
            analytics_key_id: None,
            memberships: Arc::from([]),
        },
        authorization: "Bearer x".into(),
        client_agent: "test".into(),
        client_ip: "127.0.0.1".into(),
    }
}

fn args(v: serde_json::Value) -> JsonObject {
    v.as_object().expect("object").clone()
}

fn text(result: &CallToolResult) -> String {
    match &result.content[0] {
        ContentBlock::Text(t) => t.text.clone(),
        other => panic!("unexpected content {other:?}"),
    }
}

fn provider(backend: Arc<FakeBackend>) -> QueryToolProvider {
    QueryToolProvider::new(backend, McpConfig::default())
}

async fn call(p: &QueryToolProvider, name: &str, a: serde_json::Value) -> CallToolResult {
    p.call_tool(&ctx(), name, args(a))
        .await
        .expect("tool exists")
}

#[tokio::test]
async fn all_tools_are_read_only() {
    let p = provider(Arc::new(FakeBackend::default()));
    let tools = p.tools(&ctx());
    let names: Vec<_> = tools.iter().map(|t| t.name.to_string()).collect();
    assert_eq!(
        names,
        [
            "prepare_query",
            "execute_query",
            "list_views",
            "describe_view"
        ]
    );
    for t in tools {
        assert_eq!(
            t.annotations.expect("annotations").read_only_hint,
            Some(true)
        );
    }
}

#[tokio::test]
async fn same_arguments_accepted_by_prepare_and_execute() {
    let backend = Arc::new(FakeBackend {
        rows: 3,
        ..Default::default()
    });
    let p = provider(backend.clone());
    let a = serde_json::json!({"sql": "SELECT n FROM t", "begin": "1h", "limit": 5});
    let prepared = call(&p, "prepare_query", a.clone()).await;
    let executed = call(&p, "execute_query", a).await;
    assert_ne!(prepared.is_error, Some(true));
    assert_ne!(executed.is_error, Some(true));
    let seen = backend.seen.lock().expect("lock");
    assert_eq!(seen[0].0, seen[1].0);
    assert_eq!(seen[0].1.is_some(), seen[1].1.is_some());
    // one more row than asked for, so the renderer can tell the result was cut
    assert_eq!(seen[1].2, 6);
}

#[tokio::test]
async fn degraded_prepare_reports_unknown_estimate() {
    let p = provider(Arc::new(FakeBackend::default()));
    let r = call(&p, "prepare_query", serde_json::json!({"sql": "SELECT 1"})).await;
    let t = text(&r);
    assert!(t.contains("- n: Int32"));
    assert!(t.contains("estimated_scan_bytes: unknown"));
    assert!(t.contains("estimate_precision: unknown"));
    assert!(t.contains("within_scan_cap: unknown"));
    assert!(!t.contains("hint:"));
}

#[tokio::test]
async fn over_cap_carries_a_hint() {
    let backend = Arc::new(FakeBackend::default());
    *backend.estimate.lock().expect("lock") =
        Some((Some(1_000_000_000), Some("footer".into()), Some(false)));
    let p = provider(backend);
    let r = call(
        &p,
        "prepare_query",
        serde_json::json!({"sql": "SELECT 1", "begin": "1h"}),
    )
    .await;
    let t = text(&r);
    assert!(t.contains("estimated_scan_bytes: 1000000000"));
    assert!(t.contains("within_scan_cap: false"));
    assert!(t.contains("hint: the estimated scan exceeds the cap"));
}

#[tokio::test]
async fn opaque_without_range_carries_a_hint() {
    let backend = Arc::new(FakeBackend::default());
    *backend.estimate.lock().expect("lock") = Some((Some(10), Some("opaque".into()), None));
    let p = provider(backend);
    let r = call(&p, "prepare_query", serde_json::json!({"sql": "SELECT 1"})).await;
    assert!(text(&r).contains("hint: the scan cannot be estimated without a time range"));
    let r = call(
        &p,
        "prepare_query",
        serde_json::json!({"sql": "SELECT 1", "begin": "1h"}),
    )
    .await;
    assert!(!text(&r).contains("hint:"));
}

#[tokio::test]
async fn status_errors_become_is_error_results() {
    for status in [
        tonic::Status::unauthenticated("bad token"),
        tonic::Status::invalid_argument("bad sql"),
    ] {
        let backend = Arc::new(FakeBackend::default());
        *backend.prepare_error.lock().expect("lock") = Some(status.clone());
        *backend.execute_error.lock().expect("lock") = Some(status.clone());
        let p = provider(backend);
        for tool in ["prepare_query", "execute_query"] {
            let r = call(&p, tool, serde_json::json!({"sql": "SELECT 1"})).await;
            assert_eq!(r.is_error, Some(true));
            let t = text(&r);
            assert!(t.contains(status.message()), "{t}");
            assert!(t.contains(&format!("{:?}", status.code())), "{t}");
        }
    }
}

#[tokio::test]
async fn read_only_check_gates_both_tools() {
    let backend = Arc::new(FakeBackend::default());
    let p = provider(backend.clone());
    for tool in ["prepare_query", "execute_query"] {
        for sql in ["DROP TABLE t", "SELECT * FROM retire_partitions('a')"] {
            let r = call(&p, tool, serde_json::json!({"sql": sql})).await;
            assert_eq!(r.is_error, Some(true));
        }
    }
    assert!(backend.seen.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn execute_truncates_at_the_requested_limit() {
    let backend = Arc::new(FakeBackend {
        rows: 100,
        ..Default::default()
    });
    let p = provider(backend);
    let r = call(
        &p,
        "execute_query",
        serde_json::json!({"sql": "SELECT n FROM t", "limit": 4}),
    )
    .await;
    let t = text(&r);
    assert!(t.contains("truncated at 4 rows"));
}

#[tokio::test]
async fn describe_view_rejects_odd_names() {
    let p = provider(Arc::new(FakeBackend::default()));
    let r = call(
        &p,
        "describe_view",
        serde_json::json!({"name": "x' OR '1'='1"}),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn bad_arguments_are_tool_errors() {
    let p = provider(Arc::new(FakeBackend::default()));
    let r = call(&p, "execute_query", serde_json::json!({"begin": "1h"})).await;
    assert_eq!(r.is_error, Some(true));
    let r = call(
        &p,
        "execute_query",
        serde_json::json!({"sql": "SELECT 1", "end": "1h"}),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
    assert!(
        p.call_tool(&ctx(), "nope", JsonObject::new())
            .await
            .is_none()
    );
}

#[test]
fn time_parsing() {
    let now = Utc::now();
    assert_eq!(
        parse_time("1h", now).expect("1h"),
        now - chrono::Duration::hours(1)
    );
    assert_eq!(
        parse_time("7d", now).expect("7d"),
        now - chrono::Duration::days(7)
    );
    assert!(parse_time("2024-01-01T00:00:00Z", now).is_ok());
    assert!(parse_time("yesterday", now).is_err());
    assert!(parse_range(None, None, now).expect("none").is_none());
    assert!(parse_range(Some("1h"), None, now).expect("range").is_some());
    assert!(parse_range(Some("1h"), Some("2h"), now).is_err());
}
