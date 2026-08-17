//! Live-DB coverage for the data-lake schema migration's v6 step (#1372, AbAC Stage 4): the
//! `ingestion_api_keys.audience` column. This crate has no migration test today (v5, which
//! created these very tables, shipped with none); this is the first, and it is deliberately
//! narrow -- see `tasks/1372_audience_on_keys_plan.md`'s Testing Strategy for why a live test is
//! the only tool available here (no `testcontainers`, no embedded Postgres, no migration harness
//! anywhere in this workspace) and why it is worth adding anyway (the `ADD COLUMN` → `UPDATE` →
//! `SET NOT NULL` ordering and the `CHECK` regex are pure Postgres behavior, with no Rust-side
//! function to unit-test instead; and `micromegas-ingestion` cannot depend on `micromegas-auth`,
//! so this live comparison is the only executable link between the `CHECK`'s regex and
//! `micromegas_auth::policy::is_valid_audience`'s charset).
//!
//! Runs against a throwaway, `search_path`-scoped schema (the same trick
//! `rust/auth/tests/default_provider_tests.rs` uses) so this never touches the shared
//! `MICROMEGAS_SQL_CONNECTION_STRING` database's real tables. Requires
//! `MICROMEGAS_SQL_CONNECTION_STRING`; not run by default CI (`#[ignore]`d) -- run manually with
//! `cargo test --test sql_migration_test -- --ignored`.

use micromegas_ingestion::sql_migration::{
    LATEST_DATA_LAKE_SCHEMA_VERSION, execute_migration, read_data_lake_schema_version,
    upgrade_data_lake_schema_v2, upgrade_data_lake_schema_v3, upgrade_data_lake_schema_v4,
    upgrade_data_lake_schema_v5,
};
use micromegas_ingestion::sql_telemetry_db::create_tables;
use sqlx::Row;
use std::str::FromStr;
use uuid::Uuid;

const SCHEMA: &str = "mm_1372_sql_migration_test_schema";

/// Builds a throwaway schema pinned to v5 (the pre-#1372 shape: `ingestion_api_keys` exists,
/// with no `audience` column) by running each `upgrade_data_lake_schema_vN` directly rather than
/// through `execute_migration` -- which, now that `LATEST_DATA_LAKE_SCHEMA_VERSION` is 6, would
/// carry a fresh schema all the way to v6 in one call and leave nothing to seed a v5-era row
/// into. The v2→v3 step's `CREATE UNIQUE INDEX CONCURRENTLY` (which `execute_migration` runs
/// outside any transaction) is skipped: it is orthogonal to the `audience` column and this test
/// never exercises it.
async fn build_v5_schema(pool: &sqlx::PgPool) {
    let mut tr = pool.begin().await.expect("begin v1");
    create_tables(&mut tr).await.expect("create v1 schema");
    tr.commit().await.expect("commit v1");

    let mut tr = pool.begin().await.expect("begin v2");
    upgrade_data_lake_schema_v2(&mut tr)
        .await
        .expect("v1 -> v2");
    tr.commit().await.expect("commit v2");

    let mut tr = pool.begin().await.expect("begin v3");
    upgrade_data_lake_schema_v3(&mut tr)
        .await
        .expect("v2 -> v3");
    tr.commit().await.expect("commit v3");

    let mut tr = pool.begin().await.expect("begin v4");
    upgrade_data_lake_schema_v4(&mut tr)
        .await
        .expect("v3 -> v4");
    tr.commit().await.expect("commit v4");

    let mut tr = pool.begin().await.expect("begin v5");
    upgrade_data_lake_schema_v5(&mut tr)
        .await
        .expect("v4 -> v5");
    tr.commit().await.expect("commit v5");
}

async fn schema_version(pool: &sqlx::PgPool) -> i32 {
    let mut tr = pool.begin().await.expect("begin");
    let version = read_data_lake_schema_version(&mut tr).await;
    tr.commit().await.expect("commit");
    version
}

#[ignore]
#[tokio::test]
async fn v6_backfills_existing_rows_and_rejects_invalid_audiences() {
    let base_conn_str = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .expect("MICROMEGAS_SQL_CONNECTION_STRING must point at a live Postgres");

    let setup_pool = sqlx::PgPool::connect(&base_conn_str)
        .await
        .expect("connecting to metadata Postgres");
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {SCHEMA} CASCADE"))
        .execute(&setup_pool)
        .await
        .expect("dropping any stale throwaway schema from a previous failed run");
    sqlx::query(&format!("CREATE SCHEMA {SCHEMA}"))
        .execute(&setup_pool)
        .await
        .expect("creating throwaway schema");

    let opts = sqlx::postgres::PgConnectOptions::from_str(&base_conn_str)
        .expect("valid connection string")
        .options([("search_path", SCHEMA)]);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(opts)
        .await
        .expect("connecting with a throwaway search_path");

    let test_result: Result<(), String> = async {
        build_v5_schema(&pool).await;
        if schema_version(&pool).await != 5 {
            return Err("expected the throwaway schema to start at v5".to_string());
        }

        // A v5-era row: minted before this stage existed, with no `audience` column to set.
        let key_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO ingestion_api_keys (key_id, key_hash, name, created_at, created_by)
             VALUES ($1, $2, 'v5-era-key', now(), 'test')",
        )
        .bind(key_id)
        .bind(Uuid::new_v4().as_bytes().to_vec())
        .execute(&pool)
        .await
        .map_err(|e| format!("seeding a v5-era row: {e:#}"))?;

        execute_migration(pool.clone())
            .await
            .map_err(|e| format!("migrating v5 -> v6: {e:#}"))?;

        let version = schema_version(&pool).await;
        if version != LATEST_DATA_LAKE_SCHEMA_VERSION {
            return Err(format!(
                "expected schema version {LATEST_DATA_LAKE_SCHEMA_VERSION} after migrating, got {version} \
                 (a forgotten `UPDATE migration SET version=6;` would surface as a startup panic \
                 in production, not a graceful error -- this is what catches it here instead)"
            ));
        }

        let audience: String = sqlx::query("SELECT audience FROM ingestion_api_keys WHERE key_id = $1")
            .bind(key_id)
            .fetch_one(&pool)
            .await
            .map_err(|e| format!("reading back the backfilled row: {e:#}"))?
            .try_get("audience")
            .map_err(|e| format!("reading audience column: {e:#}"))?;
        if audience != "public" {
            return Err(format!(
                "expected the v5-era row to be backfilled to 'public', got {audience:?} -- the \
                 issue's acceptance criterion is that it backfills rather than staying NULL"
            ));
        }

        // The CHECK mirrors `is_valid_audience`'s charset: assert agreement on a couple of
        // representative values, not the full accept/reject table (which
        // `rust/auth/tests/policy_tests.rs`'s `is_valid_audience_*` tests already pin) -- a
        // second enumeration here could rot independently of that one.
        for invalid in ["", "group:everyone"] {
            let result = sqlx::query(
                "INSERT INTO ingestion_api_keys (key_id, key_hash, name, created_at, created_by, audience)
                 VALUES ($1, $2, 'invalid-audience-key', now(), 'test', $3)",
            )
            .bind(Uuid::new_v4())
            .bind(Uuid::new_v4().as_bytes().to_vec())
            .bind(invalid)
            .execute(&pool)
            .await;
            if result.is_ok() {
                return Err(format!(
                    "expected the CHECK constraint to reject audience {invalid:?}"
                ));
            }
        }

        Ok(())
    }
    .await;

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {SCHEMA} CASCADE"))
        .execute(&setup_pool)
        .await
        .expect("dropping throwaway schema");

    test_result.expect("test assertions");
}
