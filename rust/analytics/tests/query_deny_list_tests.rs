//! Unit tests for the admin-managed query deny list (`tasks/query_deny_list_plan.md`), exercised
//! entirely through `micromegas-analytics`'s `pub` surface. No live database is touched: rule
//! compilation and `check` never reach Postgres, and the one `QueryDenyList` these tests build
//! uses a `connect_lazy` pool (never actually connects) purely so `QueryDenyList::with_snapshot`
//! has something to hold.

use chrono::{DateTime, TimeZone, Utc};
use datafusion::arrow::array::Array;
use datafusion::execution::context::SessionContext;
use micromegas_analytics::lakehouse::query_deny_list::{
    QueryAttribution, QueryDenyList, QueryDenyRow, QueryDenyRule, compile_match_expr,
    fingerprint_of, match_schema, skip_for_admin_recovery, sorted_snapshot,
};
use std::sync::Arc;
use uuid::Uuid;

fn lazy_pool() -> sqlx::Pool<sqlx::Postgres> {
    sqlx::PgPool::connect_lazy("postgres://user:pass@127.0.0.1:1/db")
        .expect("connect_lazy should not touch the network")
}

fn ts(seconds: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(seconds, 0)
        .single()
        .expect("valid timestamp")
}

/// Builds a compiled rule from a `match_expr`, panicking (test failure) if it doesn't compile.
fn rule(created_at: DateTime<Utc>, match_expr: &str) -> Arc<QueryDenyRule> {
    let ctx = SessionContext::new();
    let expr = compile_match_expr(&ctx, match_expr)
        .unwrap_or_else(|e| panic!("expected {match_expr:?} to compile, got: {e}"));
    Arc::new(QueryDenyRule::new(
        QueryDenyRow {
            rule_id: Uuid::new_v4(),
            created_at,
            created_by: "admin@example.com".to_string(),
            reason: "test rule".to_string(),
            match_expr: match_expr.to_string(),
            last_hit_at: None,
        },
        expr,
    ))
}

fn deny_list_with_rules(rules: Vec<Arc<QueryDenyRule>>) -> QueryDenyList {
    QueryDenyList::with_snapshot(lazy_pool(), sorted_snapshot(rules))
}

/// A fully-populated attribution, overridable field by field via the builder-style setters below
/// -- keeps each test's `QueryAttribution` construction to the one or two fields it cares about.
struct Attr {
    user_id: String,
    email: String,
    service_account: Option<String>,
    client: String,
    agent: String,
    entrypoint: String,
    session: Option<String>,
    notebook: Option<String>,
    cell: Option<String>,
    client_ip: String,
    sql: String,
    sql_hash: String,
}

impl Default for Attr {
    fn default() -> Self {
        Self {
            user_id: "alice".to_string(),
            email: "alice@example.com".to_string(),
            service_account: None,
            client: "python".to_string(),
            agent: "none".to_string(),
            entrypoint: "script".to_string(),
            session: None,
            notebook: None,
            cell: None,
            client_ip: "10.0.0.1".to_string(),
            sql: "SELECT 1".to_string(),
            sql_hash: "deadbeefdeadbeef".to_string(),
        }
    }
}

impl Attr {
    fn as_attribution(&self) -> QueryAttribution<'_> {
        QueryAttribution {
            user_id: &self.user_id,
            email: &self.email,
            service_account: self.service_account.as_deref(),
            client: &self.client,
            agent: &self.agent,
            entrypoint: &self.entrypoint,
            session: self.session.as_deref(),
            notebook: self.notebook.as_deref(),
            cell: self.cell.as_deref(),
            client_ip: &self.client_ip,
            sql: &self.sql,
            sql_hash: &self.sql_hash,
        }
    }
}

// -------------------------------------------------------------------------------------------
// fingerprint_of
// -------------------------------------------------------------------------------------------

#[test]
fn fingerprint_ignores_literal_differences() {
    let a = fingerprint_of(
        "SELECT * FROM log_entries WHERE time >= TIMESTAMP '2024-01-01T00:00:00Z' LIMIT 100",
    );
    let b = fingerprint_of(
        "SELECT * FROM log_entries WHERE time >= TIMESTAMP '2024-06-15T09:30:00Z' LIMIT 250",
    );
    assert_eq!(
        a, b,
        "consecutive dashboard refreshes should collapse to one fingerprint"
    );
}

#[test]
fn fingerprint_differs_on_column_list() {
    let a = fingerprint_of("SELECT a, b FROM t");
    let b = fingerprint_of("SELECT a, b, c FROM t");
    assert_ne!(a, b);
}

#[test]
fn fingerprint_absorbs_whitespace_comments_and_case() {
    let a = fingerprint_of("SELECT a FROM t WHERE b = 1");
    let b = fingerprint_of("select   a\nfrom t -- a comment\n  where /* inline */ b = 1");
    assert_eq!(a, b);
}

#[test]
fn fingerprint_of_unparseable_sql_still_exists() {
    let fp = fingerprint_of("this is not { valid sql at all [[[");
    assert_eq!(fp.len(), 16);
}

#[test]
fn fingerprint_is_16_hex_chars() {
    let fp = fingerprint_of("SELECT 1");
    assert_eq!(fp.len(), 16);
    assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
}

// -------------------------------------------------------------------------------------------
// compile_match_expr: the two owned checks
// -------------------------------------------------------------------------------------------

#[test]
fn compile_rejects_non_boolean_result() {
    let ctx = SessionContext::new();
    let err = compile_match_expr(&ctx, "user_id").expect_err("user_id alone is not boolean");
    assert!(
        err.to_string().to_lowercase().contains("boolean"),
        "got: {err}"
    );
}

#[test]
fn compile_rejects_true_literal_with_no_column() {
    let ctx = SessionContext::new();
    let err = compile_match_expr(&ctx, "true").expect_err("no column reference");
    assert!(err.to_string().contains("column"), "got: {err}");
}

#[test]
fn compile_rejects_tautology_with_no_column() {
    let ctx = SessionContext::new();
    let err = compile_match_expr(&ctx, "1 = 1").expect_err("no column reference");
    assert!(err.to_string().contains("column"), "got: {err}");
}

#[test]
fn compile_rejects_unknown_column_with_datafusions_own_diagnostic() {
    let ctx = SessionContext::new();
    let err = compile_match_expr(&ctx, "not_a_real_column = 'x'").expect_err("unknown column");
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("not_a_real_column") || msg.to_lowercase().contains("column"),
        "got: {msg}"
    );
}

#[test]
fn compile_rejects_unknown_function_with_datafusions_own_diagnostic() {
    let ctx = SessionContext::new();
    let err = compile_match_expr(&ctx, "not_a_real_function(sql)").expect_err("unknown function");
    assert!(
        err.to_string().to_lowercase().contains("function"),
        "got: {err}"
    );
}

#[test]
fn compile_rejects_aggregate() {
    let ctx = SessionContext::new();
    let err = compile_match_expr(&ctx, "count(sql) > 0").expect_err("aggregate rejected");
    // DataFusion's own diagnostic -- pinned loosely, just that it fails.
    let _ = err;
}

// -------------------------------------------------------------------------------------------
// "No coercion pass": the accepted subset is pinned as a property of DataFusion's physical
// planner, over a batch built from `QueryAttribution::to_batch`.
// -------------------------------------------------------------------------------------------

#[test]
fn to_batch_schema_matches_match_schema_field_for_field() {
    let attr = Attr::default();
    let batch = attr.as_attribution().to_batch();
    let batch_schema = batch.schema();
    let match_arrow_schema = match_schema().inner().clone();
    assert_eq!(
        batch_schema.fields().len(),
        match_arrow_schema.fields().len()
    );
    for (a, b) in batch_schema
        .fields()
        .iter()
        .zip(match_arrow_schema.fields().iter())
    {
        assert_eq!(a.name(), b.name());
        assert_eq!(a.data_type(), b.data_type());
    }
}

fn compiles_and_evaluates(match_expr: &str, attr: &Attr) -> bool {
    let ctx = SessionContext::new();
    let expr = compile_match_expr(&ctx, match_expr)
        .unwrap_or_else(|e| panic!("expected {match_expr:?} to compile, got: {e}"));
    let batch = attr.as_attribution().to_batch();
    let result = expr.evaluate(&batch).expect("evaluate should not error");
    match result {
        datafusion::logical_expr::ColumnarValue::Array(arr) => {
            let bools = arr
                .as_any()
                .downcast_ref::<datafusion::arrow::array::BooleanArray>()
                .expect("boolean result");
            !bools.is_null(0) && bools.value(0)
        }
        datafusion::logical_expr::ColumnarValue::Scalar(
            datafusion::scalar::ScalarValue::Boolean(Some(v)),
        ) => v,
        other => panic!("expected a boolean result, got {other:?}"),
    }
}

#[test]
fn in_list_compiles_and_evaluates() {
    let attr = Attr {
        client: "grafana".to_string(),
        ..Attr::default()
    };
    assert!(compiles_and_evaluates(
        "client IN ('grafana', 'python')",
        &attr
    ));
}

#[test]
fn like_compiles_and_evaluates() {
    let attr = Attr {
        sql: "SELECT * FROM thread_spans_view".to_string(),
        ..Attr::default()
    };
    assert!(compiles_and_evaluates("sql LIKE '%thread_spans%'", &attr));
}

#[test]
fn ilike_compiles_and_evaluates() {
    let attr = Attr {
        sql: "SELECT * FROM THREAD_SPANS_VIEW".to_string(),
        ..Attr::default()
    };
    assert!(compiles_and_evaluates("sql ILIKE '%thread_spans%'", &attr));
}

#[test]
fn regexp_like_compiles_and_evaluates() {
    let attr = Attr {
        sql: "SELECT * FROM view_instance('x')".to_string(),
        ..Attr::default()
    };
    assert!(compiles_and_evaluates(
        "regexp_like(sql, '(?i)from\\s+view_instance')",
        &attr
    ));
}

#[test]
fn is_not_null_compiles_and_evaluates() {
    let attr = Attr {
        notebook: Some("fleet-overview".to_string()),
        ..Attr::default()
    };
    assert!(compiles_and_evaluates("notebook IS NOT NULL", &attr));
    let attr_no_notebook = Attr::default();
    assert!(!compiles_and_evaluates(
        "notebook IS NOT NULL",
        &attr_no_notebook
    ));
}

#[test]
fn top_level_not_compiles_and_evaluates() {
    let attr = Attr {
        client: "grafana".to_string(),
        ..Attr::default()
    };
    assert!(compiles_and_evaluates("NOT (client = 'python')", &attr));
}

// -------------------------------------------------------------------------------------------
// Type mismatches fail loudly, at compile time
// -------------------------------------------------------------------------------------------

#[test]
fn compile_rejects_utf8_vs_int_comparison() {
    let ctx = SessionContext::new();
    let err = compile_match_expr(&ctx, "client = 42").expect_err("Utf8 vs Int64 mismatch");
    let msg = err.to_string();
    assert!(
        msg.contains("Utf8") || msg.to_lowercase().contains("type"),
        "expected a type-mismatch diagnostic, got: {msg}"
    );
}

#[test]
fn compile_rejects_utf8_vs_timestamp_comparison() {
    let ctx = SessionContext::new();
    let err = compile_match_expr(&ctx, "notebook = now()").expect_err("Utf8 vs Timestamp mismatch");
    let msg = err.to_string();
    assert!(
        msg.contains("Timestamp") || msg.to_lowercase().contains("type"),
        "expected a type-mismatch diagnostic, got: {msg}"
    );
}

// -------------------------------------------------------------------------------------------
// The identity column is named `user_id`, not `user`
// -------------------------------------------------------------------------------------------

#[test]
fn user_id_column_compiles_and_matches() {
    let attr = Attr {
        user_id: "svc-acct".to_string(),
        ..Attr::default()
    };
    assert!(compiles_and_evaluates("user_id = 'svc-acct'", &attr));
}

#[test]
fn bare_user_fails_to_parse_as_a_column() {
    let ctx = SessionContext::new();
    let err = compile_match_expr(&ctx, "user = 'jean'")
        .expect_err("bare `user` parses as the zero-arg function user(), not a column");
    let msg = err.to_string();
    assert!(msg.contains("user"), "got: {msg}");
}

// -------------------------------------------------------------------------------------------
// `check` semantics over a rule set
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn check_returns_none_on_empty_snapshot() {
    let list = deny_list_with_rules(vec![]);
    let attr = Attr::default();
    assert!(list.check(&attr.as_attribution()).is_none());
}

#[tokio::test]
async fn check_null_attribute_does_not_match_equality() {
    let list = deny_list_with_rules(vec![rule(ts(1), "notebook = 'fleet-overview'")]);
    let attr = Attr::default(); // no notebook header sent
    assert!(list.check(&attr.as_attribution()).is_none());
}

#[tokio::test]
async fn check_top_level_or_denies_when_either_side_matches() {
    let list = deny_list_with_rules(vec![rule(
        ts(1),
        "sql LIKE '%thread_spans%' OR client_ip = '10.4.9.221'",
    )]);
    let attr = Attr {
        client_ip: "10.4.9.221".to_string(),
        ..Attr::default()
    };
    assert!(list.check(&attr.as_attribution()).is_some());
}

#[tokio::test]
async fn check_returns_the_oldest_matching_rule() {
    let older = rule(ts(1), "client = 'python'");
    let newer = rule(ts(2), "client = 'python'");
    let older_id = older.row.rule_id;
    let list = deny_list_with_rules(vec![newer, older]);
    let attr = Attr::default(); // client defaults to "python"
    let matched = list.check(&attr.as_attribution()).expect("should match");
    assert_eq!(matched.row.rule_id, older_id);
}

#[tokio::test]
async fn check_matching_rule_is_returned_intact() {
    let r = rule(ts(1), "client_ip = '10.4.9.221'");
    let rule_id = r.row.rule_id;
    let list = deny_list_with_rules(vec![r]);
    let attr = Attr {
        client_ip: "10.4.9.221".to_string(),
        ..Attr::default()
    };
    let matched = list.check(&attr.as_attribution()).expect("should match");
    assert_eq!(matched.row.rule_id, rule_id);
}

// -------------------------------------------------------------------------------------------
// Every example expression in the docs compiles and evaluates as documented
// -------------------------------------------------------------------------------------------

#[test]
fn doc_examples_compile() {
    let ctx = SessionContext::new();
    let examples = [
        "sql_hash = '9f2c41ab73de0155' AND entrypoint = 'grafana-alert'",
        "user_id = 'dashboards-svc' AND notebook = 'fleet-overview'",
        "client_ip = '10.4.9.221' AND sql LIKE '%thread_spans%'",
        "client = 'grafana' AND regexp_like(sql, '(?i)from\\s+view_instance')",
        "email = 'jean@example.com' AND (notebook IS NOT NULL OR entrypoint = 'notebook')",
        "sql LIKE '%thread_spans%' OR client_ip = '10.4.9.221'",
        "sql LIKE '%thread_spans%'",
    ];
    for expr in examples {
        compile_match_expr(&ctx, expr)
            .unwrap_or_else(|e| panic!("{expr:?} failed to compile: {e}"));
    }
}

#[test]
fn doc_example_sql_hash_and_entrypoint_matches_as_documented() {
    let attr = Attr {
        sql_hash: "9f2c41ab73de0155".to_string(),
        entrypoint: "grafana-alert".to_string(),
        ..Attr::default()
    };
    assert!(compiles_and_evaluates(
        "sql_hash = '9f2c41ab73de0155' AND entrypoint = 'grafana-alert'",
        &attr
    ));
}

#[test]
fn doc_example_email_and_notebook_or_entrypoint_matches_as_documented() {
    let attr = Attr {
        email: "jean@example.com".to_string(),
        entrypoint: "notebook".to_string(),
        ..Attr::default()
    };
    assert!(compiles_and_evaluates(
        "email = 'jean@example.com' AND (notebook IS NOT NULL OR entrypoint = 'notebook')",
        &attr
    ));
}

// -------------------------------------------------------------------------------------------
// skip_for_admin_recovery: the primary recovery path
// -------------------------------------------------------------------------------------------

#[test]
fn admin_statement_calling_remove_query_denial_is_exempt() {
    assert!(skip_for_admin_recovery(
        "SELECT remove_query_denial('00000000-0000-0000-0000-000000000000')",
        true,
        true,
    ));
}

#[test]
fn same_statement_from_non_admin_is_not_exempt() {
    assert!(!skip_for_admin_recovery(
        "SELECT remove_query_denial('00000000-0000-0000-0000-000000000000')",
        false,
        true,
    ));
}

#[test]
fn non_admin_recovery_statement_exempt_when_no_admin_principal_possible() {
    // Matches register_lakehouse_functions' own gate: `is_admin || !admin_principal_possible`.
    assert!(skip_for_admin_recovery(
        "SELECT remove_query_denial('00000000-0000-0000-0000-000000000000')",
        false,
        false,
    ));
}

#[test]
fn statement_not_mentioning_any_recovery_function_is_never_exempt() {
    assert!(!skip_for_admin_recovery(
        "SELECT * FROM log_entries",
        true,
        true
    ));
}

#[test]
fn deny_queries_and_list_query_denials_are_also_recognized() {
    assert!(skip_for_admin_recovery(
        "SELECT * FROM deny_queries('client = ''x''', 'r')",
        true,
        true
    ));
    assert!(skip_for_admin_recovery(
        "SELECT * FROM list_query_denials()",
        true,
        true
    ));
}
