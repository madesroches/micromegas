//! `list_audience_grants()` -- caller-scoped UDTF listing rows of the `audience_grants` table.
//! Modeled on `list_query_denials_table_function.rs`'s shape (a
//! `TableFunctionImpl` whose sync `call_with_args` returns a lazy `TableProvider`, an async
//! `scan` that runs the DB query and builds arrays), but registered **outside** the admin gate
//! in `register_lakehouse_functions`, next to `list_view_sets()` -- every authenticated caller
//! (and, per `is_admin(metadata)`'s documented `--disable-auth` convention, every caller when
//! auth is disabled) can call it, scoped by [`GrantVisibility`].
//!
//! **Visibility.** Admin: every row. Non-admin: every grant on each `(audience, axis)` pair the
//! caller holds a matching grant on -- deliberately wider than "rows whose selector matches me":
//! if a caller may read an audience, they may see who else may, which is exactly the "who can
//! see this audience" question the Audience Access page (and this function) exists to answer.
//! An empty selector list (`CallerContext::internal()`, or an identity-less caller once the
//! always-present `"*"` is stripped -- see `scan`) yields zero rows. A maintenance caller and a
//! request with no `AuthContext` at all (`--disable-auth`) are both `is_admin: true` and take
//! the `All` branch below instead, never `Held`.
//!
//! **What this is not.** This function reads the DB table directly on every call, never
//! `DbAudienceGrantsSource`'s TTL snapshot, so a write is visible immediately. It also does not
//! (and structurally cannot) apply `analytics-web-srv`'s `self_service_mint_enabled` knob --
//! see the REST `GET .../audience-grants/visible` route for the knob-aware narrowing that backs
//! the Audience Access page's own list; this function is the always-on SQL auditing surface,
//! like `list_query_denials()`.

use async_trait::async_trait;
use datafusion::arrow::array::{RecordBatch, StringArray, TimestampNanosecondArray};
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use datafusion::catalog::Session;
use datafusion::catalog::TableFunctionArgs;
use datafusion::catalog::TableFunctionImpl;
use datafusion::catalog::TableProvider;
use datafusion::datasource::TableType;
use datafusion::datasource::memory::{DataSourceExec, MemorySourceConfig};
use datafusion::error::DataFusionError;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::prelude::Expr;
use sqlx::Row;
use std::sync::Arc;

/// How much of the `audience_grants` table [`ListAudienceGrantsTableFunction::scan`] returns,
/// decided once at registration time in `register_lakehouse_functions` from
/// `CallerContext::is_admin`/`grant_selectors`.
#[derive(Debug, Clone)]
pub enum GrantVisibility {
    /// Every row -- an admin caller.
    All,
    /// Every grant on each `(audience, axis)` pair covered by one of these selectors -- a
    /// non-admin caller's `grant_selectors`, `"*"` included. Empty for `CallerContext::internal()`;
    /// a maintenance caller and a request with no `AuthContext` at all are `is_admin: true` and
    /// take [`Self::All`] instead, never reaching this variant. `scan` strips the leading `"*"`
    /// before binding it into the held-pair query -- see the note there for why.
    Held(Arc<[String]>),
}

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("audience", DataType::Utf8, false),
        Field::new("axis", DataType::Utf8, false),
        Field::new("selector", DataType::Utf8, false),
        Field::new(
            "created_at",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("created_by", DataType::Utf8, false),
    ]))
}

/// One `audience_grants` row, as returned by either of `scan`'s two queries.
struct GrantRow {
    audience: String,
    axis: String,
    selector: String,
    created_at: chrono::DateTime<chrono::Utc>,
    created_by: String,
}

/// Every row, admin-visibility query -- `ORDER BY audience, axis, selector` matches the schema's
/// stable column order and gives deterministic output.
const SELECT_ALL_SQL: &str = "
    SELECT audience, axis, selector, created_at, created_by
    FROM audience_grants
    ORDER BY audience, axis, selector";

/// Held-pair, non-admin-visibility query: every row on a `(audience, axis)` pair
/// the caller holds a matching grant on, not just the caller's own rows -- deliberately wider,
/// since "who else can see this" is the question this function answers, and it is the same set
/// a non-admin may modify (`analytics-web-srv`'s write policy).
const SELECT_HELD_SQL: &str = "
    SELECT g.audience, g.axis, g.selector, g.created_at, g.created_by
    FROM audience_grants g
    WHERE EXISTS (
      SELECT 1 FROM audience_grants h
      WHERE h.audience = g.audience AND h.axis = g.axis AND h.selector = ANY($1)
    )
    ORDER BY g.audience, g.axis, g.selector";

/// A DataFusion `TableFunctionImpl` for `list_audience_grants()`.
#[derive(Debug)]
pub struct ListAudienceGrantsTableFunction {
    pool: sqlx::PgPool,
    visibility: GrantVisibility,
}

impl ListAudienceGrantsTableFunction {
    pub fn new(pool: sqlx::PgPool, visibility: GrantVisibility) -> Self {
        Self { pool, visibility }
    }
}

impl TableFunctionImpl for ListAudienceGrantsTableFunction {
    fn call_with_args(
        &self,
        _args: TableFunctionArgs,
    ) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        Ok(Arc::new(ListAudienceGrantsTableProvider {
            pool: self.pool.clone(),
            visibility: self.visibility.clone(),
        }))
    }
}

#[derive(Debug)]
struct ListAudienceGrantsTableProvider {
    pool: sqlx::PgPool,
    visibility: GrantVisibility,
}

#[async_trait]
impl TableProvider for ListAudienceGrantsTableProvider {
    fn schema(&self) -> SchemaRef {
        schema()
    }

    fn table_type(&self) -> TableType {
        TableType::Temporary
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        let mut rows = match &self.visibility {
            GrantVisibility::All => sqlx::query(SELECT_ALL_SQL)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| DataFusionError::External(e.into()))?,
            GrantVisibility::Held(selectors) => {
                // `caller_selectors` always leads with `"*"` -- even for an identity-less caller
                // (no email, no groups) -- so binding it unfiltered would match every pair that
                // carries a `*` grant row and leak every sibling row on it. Strip `"*"` first,
                // the same way `caller_holds_pair`'s write-side hold check does: a `*` grant is
                // not something a caller individually *holds*, only something that lets them
                // *read* via `AudienceReadPolicy`. An empty list after filtering (or an already-
                // empty list, for internal/maintenance callers) still runs the query -- `=
                // ANY($1)` over an empty array matches no row, giving zero rows rather than a
                // separate short-circuit branch to keep in sync with the SQL above.
                let identity_selectors: Vec<String> = selectors
                    .iter()
                    .filter(|s| s.as_str() != "*")
                    .cloned()
                    .collect();
                sqlx::query(SELECT_HELD_SQL)
                    .bind(&identity_selectors)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(|e| DataFusionError::External(e.into()))?
            }
        }
        .into_iter()
        .map(|row| {
            Ok::<_, sqlx::Error>(GrantRow {
                audience: row.try_get("audience")?,
                axis: row.try_get("axis")?,
                selector: row.try_get("selector")?,
                created_at: row.try_get("created_at")?,
                created_by: row.try_get("created_by")?,
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| DataFusionError::External(e.into()))?;
        if let Some(n) = limit {
            rows.truncate(n);
        }
        let audience: StringArray = rows.iter().map(|r| Some(r.audience.clone())).collect();
        let axis: StringArray = rows.iter().map(|r| Some(r.axis.clone())).collect();
        let selector: StringArray = rows.iter().map(|r| Some(r.selector.clone())).collect();
        let created_at: TimestampNanosecondArray = rows
            .iter()
            .map(|r| r.created_at.timestamp_nanos_opt())
            .collect::<TimestampNanosecondArray>()
            .with_timezone("+00:00".to_string());
        let created_by: StringArray = rows.iter().map(|r| Some(r.created_by.clone())).collect();
        let rb = RecordBatch::try_new(
            schema(),
            vec![
                Arc::new(audience),
                Arc::new(axis),
                Arc::new(selector),
                Arc::new(created_at),
                Arc::new(created_by),
            ],
        )?;
        let source =
            MemorySourceConfig::try_new(&[vec![rb]], schema(), projection.map(|v| v.to_owned()))?;
        Ok(DataSourceExec::from_data_source(source))
    }
}
