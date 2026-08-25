//! Offline (no live DB) regression tests for `tasks/admin_gate_mutating_lakehouse_functions_plan.md`:
//! `make_session_context`'s `CallerContext::is_admin`/`admin_principal_possible` gate
//! registration of the eight admin-gated lakehouse UDTFs/UDFs (`retire_partitions`,
//! `materialize_partitions`, `regenerate_partitions`, `retire_partition_by_file`,
//! `retire_partition_by_metadata`, and the query deny list's `list_query_denials`,
//! `deny_queries`, `remove_query_denial`). These tests only assert on DataFusion *planning*,
//! never execution: the gated functions' own `call_with_args` implementations only parse
//! arguments and return a lazy provider (or, for `deny_queries`, additionally compile the match
//! expression and read `rule_count()` from the in-memory, empty deny-list snapshot), so
//! planning-only assertions never touch the lazy Postgres pool or the in-memory object store
//! below.

use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::NullPartitionProvider;
use micromegas_analytics::lakehouse::query::make_session_context;
use micromegas_analytics::lakehouse::read_scope::{CallerContext, IsolationConfig};
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use std::sync::Arc;

async fn make_offline_lakehouse_context() -> Arc<LakehouseContext> {
    let db_pool = sqlx::PgPool::connect_lazy("postgres://user:pass@127.0.0.1:1/db")
        .expect("connect_lazy should not touch the network");
    let object_store: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::memory::InMemory::new());
    let blob_storage = Arc::new(BlobStorage::new(
        object_store,
        object_store::path::Path::from("lakehouse"),
    ));
    let lake = Arc::new(DataLakeConnection::new(db_pool, blob_storage));
    let runtime = Arc::new(make_runtime_env().expect("make_runtime_env"));
    Arc::new(LakehouseContext::new(lake, runtime))
}

async fn make_gated_session_context(
    is_admin: bool,
    admin_principal_possible: bool,
) -> datafusion::execution::context::SessionContext {
    let lakehouse = make_offline_lakehouse_context().await;
    let caller = CallerContext {
        read_scope: micromegas_analytics::lakehouse::read_scope::ReadScope::All,
        is_admin,
        isolation_config: Arc::new(IsolationConfig::default()),
        admin_principal_possible,
        // `deny_queries`'s own plan-time identity check (`CallerContext::identity` must be
        // `Some`) is not what this test suite is about -- give every fixture an identity so a
        // gated `deny_queries` call fails (or succeeds) on the *registration gate* under test,
        // never on this unrelated check.
        identity: Some("test-caller".to_string()),
        grant_selectors: Arc::from([]),
    };
    make_session_context(
        lakehouse,
        Arc::new(NullPartitionProvider {}),
        None,
        Arc::new(ViewFactory::new(vec![])),
        Arc::new(NoOpSessionConfigurator),
        caller,
    )
    .await
    .expect("make_session_context")
}

const MUTATING_UDTF_CALLS: &[&str] = &[
    "SELECT * FROM retire_partitions('log_entries', 'i', TIMESTAMP '2024-01-01T00:00:00Z', TIMESTAMP '2024-01-02T00:00:00Z')",
    "SELECT * FROM materialize_partitions('log_entries', TIMESTAMP '2024-01-01T00:00:00Z', TIMESTAMP '2024-01-02T00:00:00Z', 86400)",
    "SELECT * FROM regenerate_partitions('log_entries', TIMESTAMP '2024-01-01T00:00:00Z', TIMESTAMP '2024-01-02T00:00:00Z', 86400)",
    "SELECT * FROM list_query_denials()",
    "SELECT * FROM deny_queries('client = ''x''', 'r')",
];

const MUTATING_UDF_CALLS: &[&str] = &[
    "SELECT retire_partition_by_file('s3://bucket/x/file.parquet')",
    "SELECT retire_partition_by_metadata('log_entries', 'global', TIMESTAMP '2024-01-01T00:00:00Z', TIMESTAMP '2024-01-02T00:00:00Z')",
    "SELECT remove_query_denial('9f2c41ab-73de-4015-9d2e-000000000000')",
];

const NON_MUTATING_CALLS: &[&str] = &[
    "SELECT * FROM list_partitions()",
    "SELECT * FROM list_view_sets()",
    "SELECT * FROM list_audience_grants()",
];

#[tokio::test]
async fn non_admin_session_cannot_plan_mutating_udtfs() {
    let ctx = make_gated_session_context(false, true).await;
    for sql in MUTATING_UDTF_CALLS {
        let err = ctx
            .sql(sql)
            .await
            .expect_err(&format!("expected planning to fail for non-admin: {sql}"));
        let msg = err.to_string();
        assert!(
            msg.contains("table function") && msg.contains("not found"),
            "expected a 'table function ... not found' error for {sql}, got: {msg}"
        );
    }
}

#[tokio::test]
async fn non_admin_session_cannot_plan_mutating_udfs() {
    let ctx = make_gated_session_context(false, true).await;
    for sql in MUTATING_UDF_CALLS {
        let err = ctx
            .sql(sql)
            .await
            .expect_err(&format!("expected planning to fail for non-admin: {sql}"));
        let msg = err.to_string();
        assert!(
            msg.contains("Invalid function"),
            "expected an 'Invalid function' error for {sql}, got: {msg}"
        );
    }
}

#[tokio::test]
async fn admin_session_can_plan_all_mutating_functions() {
    let ctx = make_gated_session_context(true, true).await;
    for sql in MUTATING_UDTF_CALLS.iter().chain(MUTATING_UDF_CALLS.iter()) {
        ctx.sql(sql)
            .await
            .unwrap_or_else(|e| panic!("expected admin session to plan {sql}, got: {e}"));
    }
}

/// #1371: the registration gate -- `caller.is_admin || !caller.admin_principal_possible` --
/// lets a non-admin plan the mutating functions whenever the deployment can never produce an
/// admin principal at all, the API-key-only deployment's way back after #1382 gated them on
/// `is_admin` alone.
#[tokio::test]
async fn non_admin_session_without_admin_principal_can_plan_all_mutating_functions() {
    let ctx = make_gated_session_context(false, false).await;
    for sql in MUTATING_UDTF_CALLS.iter().chain(MUTATING_UDF_CALLS.iter()) {
        ctx.sql(sql).await.unwrap_or_else(|e| {
            panic!("expected non-admin session to plan {sql} when no admin principal is possible, got: {e}")
        });
    }
}

/// An admin session is unaffected by `admin_principal_possible` either way -- `is_admin` alone
/// is already sufficient, and the fallback is additive, never a restriction.
#[tokio::test]
async fn admin_session_can_plan_all_mutating_functions_regardless_of_admin_principal_possible() {
    for admin_principal_possible in [false, true] {
        let ctx = make_gated_session_context(true, admin_principal_possible).await;
        for sql in MUTATING_UDTF_CALLS.iter().chain(MUTATING_UDF_CALLS.iter()) {
            ctx.sql(sql).await.unwrap_or_else(|e| {
                panic!(
                    "expected admin session to plan {sql} with admin_principal_possible={admin_principal_possible}, got: {e}"
                )
            });
        }
    }
}

#[tokio::test]
async fn non_mutating_functions_plan_identically_for_admin_and_non_admin() {
    for is_admin in [false, true] {
        let ctx = make_gated_session_context(is_admin, true).await;
        for sql in NON_MUTATING_CALLS {
            ctx.sql(sql).await.unwrap_or_else(|e| {
                panic!("expected {sql} to plan with is_admin={is_admin}, got: {e}")
            });
        }
    }
}
