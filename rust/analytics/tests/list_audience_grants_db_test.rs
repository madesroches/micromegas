//! DB-backed tests for `list_audience_grants()` (#1489, AbAC Stage 6b) -- the visibility rule
//! from the plan's Design §2: admin sees every row, a non-admin sees every grant on each
//! `(audience, axis)` pair covered by one of their identity `grant_selectors` (including a
//! `group:` hold and its sibling rows on the same pair, and never a different pair), and an
//! empty selector list (internal/maintenance callers, or a `["*"]`-only list once the
//! always-present `"*"` is stripped) sees zero rows. Also asserts the schema's column
//! order/types. `#[ignore]`d, requires a live `MICROMEGAS_SQL_CONNECTION_STRING` /
//! `MICROMEGAS_OBJECT_STORE_URI` -- mirrors `query_deny_list_db_test.rs`'s convention; does not
//! run under a plain `cargo test`.

mod common;

use anyhow::{Context, Result};
use common::db_fixtures::ensure_telemetry_guard;
use datafusion::arrow::array::{Array, RecordBatch, StringArray};
use datafusion::arrow::datatypes::{DataType, TimeUnit};
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::NullPartitionProvider;
use micromegas_analytics::lakehouse::query::make_session_context;
use micromegas_analytics::lakehouse::read_scope::{CallerContext, IsolationConfig, ReadScope};
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use std::sync::Arc;
use uuid::Uuid;

async fn lakehouse() -> Result<Arc<LakehouseContext>> {
    ensure_telemetry_guard();
    LakehouseContext::from_env().await
}

fn caller(is_admin: bool, grant_selectors: &[&str]) -> CallerContext {
    CallerContext {
        read_scope: ReadScope::All,
        is_admin,
        isolation_config: Arc::new(IsolationConfig::default()),
        admin_principal_possible: true,
        identity: None,
        grant_selectors: grant_selectors
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into(),
    }
}

/// Plans and executes `sql` under `caller`, returning the schema and collected batches -- a
/// fresh `SessionContext` per call, matching `ownership_rewrite_db_test.rs`'s convention.
async fn query(
    lakehouse: Arc<LakehouseContext>,
    caller: CallerContext,
    sql: &str,
) -> Result<(datafusion::arrow::datatypes::SchemaRef, Vec<RecordBatch>)> {
    let ctx = make_session_context(
        lakehouse,
        Arc::new(NullPartitionProvider {}),
        None,
        Arc::new(ViewFactory::new(vec![])),
        Arc::new(NoOpSessionConfigurator),
        caller,
    )
    .await
    .with_context(|| "make_session_context")?;
    let df = ctx.sql(sql).await?;
    let schema: datafusion::arrow::datatypes::SchemaRef = Arc::new(df.schema().as_arrow().clone());
    let batches = df.collect().await?;
    Ok((schema, batches))
}

fn decode_triples(batches: &[RecordBatch]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for batch in batches {
        let audience = batch
            .column_by_name("audience")
            .expect("audience column")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("audience is Utf8");
        let axis = batch
            .column_by_name("axis")
            .expect("axis column")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("axis is Utf8");
        let selector = batch
            .column_by_name("selector")
            .expect("selector column")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("selector is Utf8");
        for i in 0..batch.num_rows() {
            out.push((
                audience.value(i).to_string(),
                axis.value(i).to_string(),
                selector.value(i).to_string(),
            ));
        }
    }
    out
}

async fn insert_grant(
    pool: &sqlx::PgPool,
    audience: &str,
    axis: &str,
    selector: &str,
    created_by: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
         VALUES ($1, $2, $3, now(), $4)",
    )
    .bind(audience)
    .bind(axis)
    .bind(selector)
    .bind(created_by)
    .execute(pool)
    .await?;
    Ok(())
}

async fn cleanup(pool: &sqlx::PgPool, created_by: &str) {
    let _ = sqlx::query("DELETE FROM audience_grants WHERE created_by = $1")
        .bind(created_by)
        .execute(pool)
        .await;
}

#[ignore]
#[tokio::test]
async fn admin_sees_every_row_across_pairs() -> Result<()> {
    let lakehouse = lakehouse().await?;
    let pool = lakehouse.lake().db_pool.clone();
    let tag = format!("list-audience-grants-db-test-{}", Uuid::new_v4());
    let aud_a = format!("{tag}-a");
    let aud_b = format!("{tag}-b");

    insert_grant(&pool, &aud_a, "read", "group:eng", &tag).await?;
    insert_grant(&pool, &aud_b, "mint", "user:carol@example.com", &tag).await?;

    let (_, batches) = query(
        lakehouse.clone(),
        caller(true, &[]),
        &format!(
            "SELECT audience, axis, selector FROM list_audience_grants() \
             WHERE created_by = '{tag}' ORDER BY audience, axis, selector"
        ),
    )
    .await?;
    let rows = decode_triples(&batches);
    assert_eq!(
        rows,
        vec![
            (aud_a.clone(), "read".to_string(), "group:eng".to_string()),
            (
                aud_b.clone(),
                "mint".to_string(),
                "user:carol@example.com".to_string()
            ),
        ]
    );

    cleanup(&pool, &tag).await;
    Ok(())
}

#[ignore]
#[tokio::test]
async fn non_admin_sees_every_grant_on_a_held_pair_including_siblings_and_group_holds() -> Result<()>
{
    let lakehouse = lakehouse().await?;
    let pool = lakehouse.lake().db_pool.clone();
    let tag = format!("list-audience-grants-db-test-{}", Uuid::new_v4());
    let aud_a = format!("{tag}-a");
    let aud_b = format!("{tag}-b");

    // Two rows on the same (aud_a, read) pair -- the caller holds it only via `group:eng`, but
    // must still see the sibling `user:` row on that pair.
    insert_grant(&pool, &aud_a, "read", "group:eng", &tag).await?;
    insert_grant(&pool, &aud_a, "read", "user:bob@example.com", &tag).await?;
    // A different axis on the same audience -- not held, must not appear.
    insert_grant(&pool, &aud_a, "mint", "user:carol@example.com", &tag).await?;
    // A different audience entirely -- not held, must not appear.
    insert_grant(&pool, &aud_b, "read", "user:dave@example.com", &tag).await?;

    let (_, batches) = query(
        lakehouse.clone(),
        caller(false, &["group:eng"]),
        &format!(
            "SELECT audience, axis, selector FROM list_audience_grants() \
             WHERE created_by = '{tag}' ORDER BY audience, axis, selector"
        ),
    )
    .await?;
    let rows = decode_triples(&batches);
    assert_eq!(
        rows,
        vec![
            (aud_a.clone(), "read".to_string(), "group:eng".to_string()),
            (
                aud_a.clone(),
                "read".to_string(),
                "user:bob@example.com".to_string()
            ),
        ]
    );

    cleanup(&pool, &tag).await;
    Ok(())
}

#[ignore]
#[tokio::test]
async fn empty_selector_list_yields_zero_rows() -> Result<()> {
    let lakehouse = lakehouse().await?;
    let pool = lakehouse.lake().db_pool.clone();
    let tag = format!("list-audience-grants-db-test-{}", Uuid::new_v4());
    let aud_a = format!("{tag}-a");

    insert_grant(&pool, &aud_a, "read", "group:eng", &tag).await?;

    // An internal/maintenance-shaped caller (`grant_selectors: []`, non-admin) sees nothing,
    // even though a matching row exists.
    let (_, batches) = query(
        lakehouse.clone(),
        caller(false, &[]),
        &format!(
            "SELECT audience, axis, selector FROM list_audience_grants() WHERE created_by = '{tag}'"
        ),
    )
    .await?;
    assert!(decode_triples(&batches).is_empty());

    cleanup(&pool, &tag).await;
    Ok(())
}

#[ignore]
#[tokio::test]
async fn star_only_selector_list_yields_zero_rows() -> Result<()> {
    let lakehouse = lakehouse().await?;
    let pool = lakehouse.lake().db_pool.clone();
    let tag = format!("list-audience-grants-db-test-{}", Uuid::new_v4());
    let aud_a = format!("{tag}-a");

    // A `*` grant makes this pair publicly readable, but a caller with no email/groups holds
    // no identity selector of their own -- `caller_selectors` always leads with `"*"`, so this
    // guards against binding that `"*"` unfiltered into the held-pair query, which would match
    // every pair carrying a `*` row and leak every sibling row on it.
    insert_grant(&pool, &aud_a, "read", "*", &tag).await?;
    insert_grant(&pool, &aud_a, "read", "user:bob@example.com", &tag).await?;

    let (_, batches) = query(
        lakehouse.clone(),
        caller(false, &["*"]),
        &format!(
            "SELECT audience, axis, selector FROM list_audience_grants() WHERE created_by = '{tag}'"
        ),
    )
    .await?;
    assert!(decode_triples(&batches).is_empty());

    cleanup(&pool, &tag).await;
    Ok(())
}

#[ignore]
#[tokio::test]
async fn schema_column_order_and_types() -> Result<()> {
    let lakehouse = lakehouse().await?;
    let (schema, _) = query(
        lakehouse.clone(),
        caller(true, &[]),
        "SELECT * FROM list_audience_grants() WHERE audience = '__no_such_audience__'",
    )
    .await?;
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(
        names,
        vec!["audience", "axis", "selector", "created_at", "created_by"]
    );
    assert_eq!(schema.field(0).data_type(), &DataType::Utf8);
    assert_eq!(schema.field(1).data_type(), &DataType::Utf8);
    assert_eq!(schema.field(2).data_type(), &DataType::Utf8);
    assert_eq!(
        schema.field(3).data_type(),
        &DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into()))
    );
    assert_eq!(schema.field(4).data_type(), &DataType::Utf8);
    assert!(!schema.field(0).is_nullable());
    assert!(!schema.field(3).is_nullable());
    Ok(())
}
