use micromegas::servers::mcp::read_only::{ADMIN_ONLY_READ_FUNCTIONS, check_read_only};
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::NullPartitionProvider;
use micromegas_analytics::lakehouse::query::{SessionRange, make_session_context};
use micromegas_analytics::lakehouse::read_scope::{CallerContext, IsolationConfig, ReadScope};
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::view_definition::MUTATING_FUNCTIONS;
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use std::collections::BTreeSet;
use std::sync::Arc;

#[test]
fn accepts_plain_queries() {
    for sql in [
        "SELECT 1",
        "SELECT time, msg FROM log_entries WHERE level <= 3 ORDER BY time DESC LIMIT 10",
        "WITH a AS (SELECT 1 AS x) SELECT * FROM a",
        "SELECT * FROM view_instance('thread_spans', 'abc') LIMIT 5",
        "SELECT * FROM list_view_sets()",
        "SELECT x FROM (SELECT 1 AS x UNION ALL SELECT 2) t",
    ] {
        assert!(check_read_only(sql).is_ok(), "should accept: {sql}");
    }
}

#[test]
fn rejects_other_statements() {
    for sql in [
        "CREATE TABLE t (a INT)",
        "INSERT INTO t VALUES (1)",
        "DELETE FROM t",
        "UPDATE t SET a = 1",
        "SET datafusion.execution.batch_size = 1",
        "COPY t TO 'x.csv'",
        "DROP TABLE t",
        "CREATE MATERIALIZED VIEW v WITH (a = 'b')",
        "DROP MATERIALIZED VIEW v",
        "SELECT 1; SELECT 2",
        "SELECT 1; DROP TABLE t",
        "not sql at all",
        "",
        "WITH x AS (INSERT INTO t VALUES (1) RETURNING a) SELECT * FROM x",
    ] {
        assert!(check_read_only(sql).is_err(), "should reject: {sql}");
    }
}

fn denied_names() -> Vec<&'static str> {
    MUTATING_FUNCTIONS
        .iter()
        .chain(ADMIN_ONLY_READ_FUNCTIONS.iter())
        .copied()
        .collect()
}

#[test]
fn rejects_every_denied_function_in_every_position() {
    for name in denied_names() {
        let upper = name.to_uppercase();
        let quoted = format!("\"{name}\"");
        for spelling in [name.to_string(), upper, quoted] {
            for sql in [
                format!("SELECT * FROM {spelling}('a')"),
                format!("SELECT {spelling}('a')"),
                format!("SELECT * FROM (SELECT {spelling}('a') AS x) t"),
                format!("SELECT * FROM t WHERE x IN (SELECT * FROM {spelling}('a'))"),
                format!("WITH c AS (SELECT * FROM {spelling}('a')) SELECT * FROM c"),
                format!("SELECT * FROM public.{spelling}('a')"),
                format!("SELECT 1 WHERE EXISTS (SELECT {spelling}('a'))"),
            ] {
                assert!(check_read_only(&sql).is_err(), "should reject: {sql}");
            }
        }
    }
}

async fn session_functions(is_admin: bool) -> BTreeSet<String> {
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
    let lakehouse = Arc::new(LakehouseContext::new(lake, runtime).expect("LakehouseContext::new"));
    let caller = CallerContext {
        read_scope: ReadScope::All,
        is_admin,
        isolation_config: Arc::new(IsolationConfig::default()),
        identity: Some("test-caller".to_string()),
        grant_selectors: Arc::from([]),
    };
    let ctx = make_session_context(
        lakehouse,
        Arc::new(NullPartitionProvider {}),
        SessionRange::Snapshot,
        Arc::new(ViewFactory::new(vec![])),
        Arc::new(NoOpSessionConfigurator),
        caller,
    )
    .await
    .expect("make_session_context");
    let state = ctx.state();
    state
        .table_functions()
        .keys()
        .cloned()
        .chain(state.scalar_functions().keys().cloned())
        .collect()
}

#[tokio::test]
async fn denies_every_function_only_admins_get() {
    let admin = session_functions(true).await;
    let regular = session_functions(false).await;
    let admin_only: Vec<_> = admin.difference(&regular).collect();
    assert!(
        !admin_only.is_empty(),
        "admin context registers extra functions"
    );
    for name in admin_only {
        assert!(
            check_read_only(&format!("SELECT * FROM {name}()")).is_err(),
            "admin-only function {name} is not denied"
        );
        assert!(
            check_read_only(&format!("SELECT {name}()")).is_err(),
            "admin-only function {name} is not denied"
        );
    }
}
