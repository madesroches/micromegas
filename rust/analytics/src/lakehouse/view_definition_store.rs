//! Postgres-backed persistence for `lakehouse_view_set_definitions`.
//!
//! [`ViewDefinitionStore`] is reduced to the single pool-backed [`ViewDefinitionStore::list`]
//! method [`super::view_registry::ViewRegistry::reload`] needs -- its only implementors are
//! [`PgViewDefinitionStore`] and a test fake. The DDL executor
//! (`rust/public/src/servers/view_ddl.rs`) instead calls the free functions below directly,
//! against its own open `sqlx::Transaction`, so a `CREATE`/`DROP`'s existence check, upsert/delete
//! and dependent-protection re-read all run inside one transaction, alongside `retire_partitions`
//! on a `DROP` -- with no trait indirection for methods a test fake could never implement.

use super::view_definition::{ViewDefinition, ViewOptions};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::Row;
use std::fmt::Debug;

/// One row of `lakehouse_view_set_definitions`, exactly as read back. `view_options` is kept as
/// its raw JSON text here (parsed into a [`ViewOptions`] only by [`ViewDefinitionRow::into_definition`])
/// so a row that fails to parse can still be reported by name rather than losing its identity.
#[derive(Debug, Clone)]
pub struct ViewDefinitionRow {
    pub view_set_name: String,
    pub definition_sql: String,
    pub extract_query: String,
    pub count_src_query: String,
    pub merge_partitions_query: String,
    pub update_group: i32,
    pub view_options: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<String>,
}

impl ViewDefinitionRow {
    /// Parses `view_options` and assembles the normalized [`ViewDefinition`] the DDL executor and
    /// registry loader both validate and build a `SqlBatchView` from.
    pub fn into_definition(self) -> Result<ViewDefinition> {
        let options: ViewOptions = serde_json::from_str(&self.view_options).with_context(|| {
            format!(
                "'{}': parsing view_options {:?}",
                self.view_set_name, self.view_options
            )
        })?;
        Ok(ViewDefinition {
            view_set_name: self.view_set_name,
            extract_query: self.extract_query,
            count_src_query: self.count_src_query,
            merge_partitions_query: self.merge_partitions_query,
            update_group: self.update_group,
            options,
        })
    }
}

fn row_from_sqlx(row: sqlx::postgres::PgRow) -> Result<ViewDefinitionRow> {
    Ok(ViewDefinitionRow {
        view_set_name: row.try_get("view_set_name")?,
        definition_sql: row.try_get("definition_sql")?,
        extract_query: row.try_get("extract_query")?,
        count_src_query: row.try_get("count_src_query")?,
        merge_partitions_query: row.try_get("merge_partitions_query")?,
        update_group: row.try_get("update_group")?,
        view_options: row.try_get("view_options")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        updated_by: row.try_get("updated_by")?,
    })
}

const SELECT_COLUMNS: &str = "view_set_name, definition_sql, extract_query, count_src_query, \
     merge_partitions_query, update_group, view_options, created_at, updated_at, updated_by";

/// The rows-in seam [`super::view_registry::ViewRegistry::reload`] needs: every definition
/// currently in force, ordered by `(update_group, view_set_name)` -- the order
/// `super::view_registry::build_factory` relies on regardless of which caller fed it the rows.
#[async_trait]
pub trait ViewDefinitionStore: Send + Sync + Debug {
    async fn list(&self) -> Result<Vec<ViewDefinitionRow>>;
}

/// The production, pool-backed implementation.
#[derive(Debug)]
pub struct PgViewDefinitionStore {
    pool: sqlx::Pool<sqlx::Postgres>,
}

impl PgViewDefinitionStore {
    pub fn new(pool: sqlx::Pool<sqlx::Postgres>) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ViewDefinitionStore for PgViewDefinitionStore {
    async fn list(&self) -> Result<Vec<ViewDefinitionRow>> {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM lakehouse_view_set_definitions \
             ORDER BY update_group, view_set_name"
        );
        let rows = sqlx::query(&sql)
            .fetch_all(&self.pool)
            .await
            .with_context(|| "listing lakehouse_view_set_definitions")?;
        rows.into_iter().map(row_from_sqlx).collect()
    }
}

/// The transaction-scoped counterpart of [`ViewDefinitionStore::list`], for a caller (the DDL
/// executor) already holding an open `sqlx::Transaction` -- so a pre-mutation read cannot miss a
/// concurrent, uncommitted write from a sibling transaction, and a post-mutation read sees this
/// transaction's own write.
pub async fn list_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<Vec<ViewDefinitionRow>> {
    let sql = format!(
        "SELECT {SELECT_COLUMNS} FROM lakehouse_view_set_definitions \
         ORDER BY update_group, view_set_name"
    );
    let rows = sqlx::query(&sql)
        .fetch_all(&mut **tx)
        .await
        .with_context(|| "listing lakehouse_view_set_definitions (tx)")?;
    rows.into_iter().map(row_from_sqlx).collect()
}

/// `true` when a row named `view_set_name` already exists -- what `CREATE` (without `OR REPLACE`)
/// checks before refusing to overwrite it.
pub async fn exists_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    view_set_name: &str,
) -> Result<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM lakehouse_view_set_definitions WHERE view_set_name = $1",
    )
    .bind(view_set_name)
    .fetch_one(&mut **tx)
    .await
    .with_context(|| format!("checking existence of '{view_set_name}'"))?;
    Ok(count > 0)
}

/// Inserts a new row, or -- on a name collision -- updates every column including `updated_at`/
/// `updated_by` explicitly: the column defaults only fire on `INSERT`, and `reload()`'s digest is
/// over `(view_set_name, updated_at)`, so a `REPLACE` that left it unset would go live only on the
/// replacing node while every other replica kept serving/materializing the old definition.
pub async fn upsert_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    def: &ViewDefinition,
    definition_sql: &str,
    updated_by: &str,
) -> Result<()> {
    let view_options = serde_json::to_string(&def.options)
        .with_context(|| format!("serializing view_options for '{}'", def.view_set_name))?;
    sqlx::query(
        "INSERT INTO lakehouse_view_set_definitions
             (view_set_name, definition_sql, extract_query, count_src_query,
              merge_partitions_query, update_group, view_options, updated_at, updated_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, now(), $8)
         ON CONFLICT (view_set_name) DO UPDATE SET
             definition_sql = EXCLUDED.definition_sql,
             extract_query = EXCLUDED.extract_query,
             count_src_query = EXCLUDED.count_src_query,
             merge_partitions_query = EXCLUDED.merge_partitions_query,
             update_group = EXCLUDED.update_group,
             view_options = EXCLUDED.view_options,
             updated_at = now(),
             updated_by = EXCLUDED.updated_by;",
    )
    .bind(&def.view_set_name)
    .bind(definition_sql)
    .bind(&def.extract_query)
    .bind(&def.count_src_query)
    .bind(&def.merge_partitions_query)
    .bind(def.update_group)
    .bind(view_options)
    .bind(updated_by)
    .execute(&mut **tx)
    .await
    .with_context(|| format!("upserting '{}'", def.view_set_name))?;
    Ok(())
}

/// Deletes a row by name; `true` if a row was actually deleted.
pub async fn delete_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    view_set_name: &str,
) -> Result<bool> {
    let result = sqlx::query("DELETE FROM lakehouse_view_set_definitions WHERE view_set_name = $1")
        .bind(view_set_name)
        .execute(&mut **tx)
        .await
        .with_context(|| format!("deleting '{view_set_name}'"))?;
    Ok(result.rows_affected() > 0)
}

/// The exact insert-time range currently materialized for `view_set_name`'s `'global'` instance --
/// `None` when it has no partitions -- for `DROP`'s call to `retire_partitions`. Deliberately
/// exact rather than an unbounded range: `retire_partitions` is hash-agnostic but still needs an
/// explicit range to key its deletion on.
pub async fn partition_insert_range(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    view_set_name: &str,
) -> Result<Option<(DateTime<Utc>, DateTime<Utc>)>> {
    let row = sqlx::query(
        "SELECT min(begin_insert_time) AS begin, max(end_insert_time) AS end
         FROM lakehouse_partitions
         WHERE view_set_name = $1 AND view_instance_id = 'global';",
    )
    .bind(view_set_name)
    .fetch_one(&mut **tx)
    .await
    .with_context(|| format!("reading partition insert-time range for '{view_set_name}'"))?;
    let begin: Option<DateTime<Utc>> = row.try_get("begin")?;
    let end: Option<DateTime<Utc>> = row.try_get("end")?;
    Ok(begin.zip(end))
}
