//! Idempotent startup backfill that stamps `micromegas.audience` onto legacy `processes` rows
//! (#1482 §0.2, the first-class `audience` column).
//!
//! Not a versioned migration (`sql_migration.rs`'s `LATEST_DATA_LAKE_SCHEMA_VERSION` is
//! untouched): a version-gated backfill runs exactly once, at the first upgraded replica's
//! startup, and every row an old replica writes afterward during a rolling upgrade would be
//! permanently unstamped with no repair path. This statement is safe to re-run at every
//! ingestion-role startup instead -- a zero-row run is one sequential scan of a
//! retention-bounded table with no row locks.

use anyhow::Context;
use micromegas_telemetry::property_names::PROPERTY_AUDIENCE;
use micromegas_tracing::prelude::*;

use crate::write_audience::WriteAudience;

/// Appends a `micromegas.audience` property carrying `default_audience` to every `processes` row
/// that does not already have one. Safe to run any number of times: a row that already has the
/// property (any value, not just `default_audience`) is left untouched.
///
/// `properties` is nullable (`sql_telemetry_db.rs`) -- `array_append(NULL, x)` and
/// `unnest(NULL)` both do the right thing, so a `NULL`-properties row becomes a one-element
/// array rather than needing special-casing.
pub async fn backfill_default_audience(
    pool: &sqlx::PgPool,
    default_audience: &WriteAudience,
) -> anyhow::Result<()> {
    let sql = format!(
        "UPDATE processes
            SET properties = array_append(properties, ROW('{PROPERTY_AUDIENCE}', $1::text)::micromegas_property)
          WHERE NOT EXISTS (SELECT 1 FROM unnest(properties) WHERE key = '{PROPERTY_AUDIENCE}');"
    );
    let result = sqlx::query(&sql)
        .bind(default_audience.as_str())
        .execute(pool)
        .await
        .with_context(|| "backfilling micromegas.audience onto legacy processes rows")?;
    let rows = result.rows_affected();
    if rows > 0 {
        info!(
            "backfilled {rows} processes row(s) with default audience {:?}",
            default_audience.as_str()
        );
    } else {
        debug!("audience backfill: no processes rows needed stamping");
    }
    Ok(())
}
