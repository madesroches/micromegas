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
    upgrade_data_lake_schema_v5, upgrade_data_lake_schema_v6, upgrade_data_lake_schema_v7,
    upgrade_data_lake_schema_v8,
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

/// Builds a throwaway schema pinned to v6 (the pre-#1489 shape: no `audience_grants` table yet)
/// by chaining `build_v5_schema` with the v6 step directly, for the same reason
/// `build_v5_schema` bypasses `execute_migration` -- which, now that
/// `LATEST_DATA_LAKE_SCHEMA_VERSION` is 7, would carry a fresh schema all the way to v7 in one
/// call and leave nothing at v6 to migrate from.
async fn build_v6_schema(pool: &sqlx::PgPool) {
    build_v5_schema(pool).await;
    let mut tr = pool.begin().await.expect("begin v6");
    upgrade_data_lake_schema_v6(&mut tr)
        .await
        .expect("v5 -> v6");
    tr.commit().await.expect("commit v6");
}

/// Builds a throwaway schema pinned to v7 (no `audience` column on `processes`/`streams`/`blocks`
/// yet) by chaining `build_v6_schema` with the v7 step directly, bypassing `execute_migration`
/// so it doesn't carry the fresh schema straight to v8, leaving nothing at v7 to migrate from.
async fn build_v7_schema(pool: &sqlx::PgPool) {
    build_v6_schema(pool).await;
    let mut tr = pool.begin().await.expect("begin v7");
    upgrade_data_lake_schema_v7(&mut tr)
        .await
        .expect("v6 -> v7");
    tr.commit().await.expect("commit v7");
}

/// Builds a throwaway schema pinned to v8 (no seeded `public` read/mint rows in `audience_grants`
/// yet) by chaining `build_v7_schema` with the v8 step directly, bypassing `execute_migration` so
/// it doesn't carry the fresh schema straight to v9, leaving nothing at v8 to migrate from.
async fn build_v8_schema(pool: &sqlx::PgPool) {
    build_v7_schema(pool).await;
    let mut tr = pool.begin().await.expect("begin v8");
    upgrade_data_lake_schema_v8(&mut tr)
        .await
        .expect("v7 -> v8");
    tr.commit().await.expect("commit v8");
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

/// The v6 -> v7 step (#1489, AbAC Stage 6a): `execute_migration` against a v6 database creates
/// `audience_grants` with its `PRIMARY KEY (audience, axis, selector)` and its two `CHECK`
/// constraints, both mirroring `AudienceGrants::from_rows`'s Rust-side validation
/// (`rust/auth/src/policy.rs`) on the SQL side.
const SCHEMA_V7: &str = "mm_1489_sql_migration_test_schema";

#[ignore]
#[tokio::test]
async fn v7_creates_audience_grants_table_with_constraints() {
    let base_conn_str = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .expect("MICROMEGAS_SQL_CONNECTION_STRING must point at a live Postgres");

    let setup_pool = sqlx::PgPool::connect(&base_conn_str)
        .await
        .expect("connecting to metadata Postgres");
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {SCHEMA_V7} CASCADE"))
        .execute(&setup_pool)
        .await
        .expect("dropping any stale throwaway schema from a previous failed run");
    sqlx::query(&format!("CREATE SCHEMA {SCHEMA_V7}"))
        .execute(&setup_pool)
        .await
        .expect("creating throwaway schema");

    let opts = sqlx::postgres::PgConnectOptions::from_str(&base_conn_str)
        .expect("valid connection string")
        .options([("search_path", SCHEMA_V7)]);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(opts)
        .await
        .expect("connecting with a throwaway search_path");

    let test_result: Result<(), String> = async {
        build_v6_schema(&pool).await;
        if schema_version(&pool).await != 6 {
            return Err("expected the throwaway schema to start at v6".to_string());
        }

        execute_migration(pool.clone())
            .await
            .map_err(|e| format!("migrating v6 -> v7: {e:#}"))?;

        let version = schema_version(&pool).await;
        if version != LATEST_DATA_LAKE_SCHEMA_VERSION {
            return Err(format!(
                "expected schema version {LATEST_DATA_LAKE_SCHEMA_VERSION} after migrating, got {version} \
                 (a forgotten `UPDATE migration SET version=7;` would surface as a startup panic \
                 in production, not a graceful error -- this is what catches it here instead)"
            ));
        }

        // A well-formed row is accepted.
        sqlx::query(
            "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
             VALUES ('team-alpha', 'read', 'group:eng', now(), 'test')",
        )
        .execute(&pool)
        .await
        .map_err(|e| format!("inserting a well-formed row should succeed: {e:#}"))?;

        // The primary key rejects a duplicate natural-key row.
        let dup = sqlx::query(
            "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
             VALUES ('team-alpha', 'read', 'group:eng', now(), 'test-2')",
        )
        .execute(&pool)
        .await;
        if dup.is_ok() {
            return Err("expected the primary key to reject a duplicate row".to_string());
        }

        // `axis` must be 'read' or 'mint'.
        let bad_axis = sqlx::query(
            "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
             VALUES ('team-alpha', 'write', '*', now(), 'test')",
        )
        .execute(&pool)
        .await;
        if bad_axis.is_ok() {
            return Err("expected the axis CHECK to reject 'write'".to_string());
        }

        // The audience-name CHECK mirrors `is_valid_audience`'s charset -- assert agreement on a
        // representative value, not the full accept/reject table (which
        // `rust/auth/tests/policy_tests.rs`'s `is_valid_audience_*` tests already pin).
        let bad_audience = sqlx::query(
            "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
             VALUES ('group:everyone', 'read', '*', now(), 'test')",
        )
        .execute(&pool)
        .await;
        if bad_audience.is_ok() {
            return Err("expected the audience-name CHECK to reject 'group:everyone'".to_string());
        }

        // The selector-shape CHECK mirrors `valid_selector`.
        let bad_selector = sqlx::query(
            "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
             VALUES ('team-alpha', 'read', 'not-a-selector', now(), 'test')",
        )
        .execute(&pool)
        .await;
        if bad_selector.is_ok() {
            return Err("expected the selector-shape CHECK to reject 'not-a-selector'".to_string());
        }

        Ok(())
    }
    .await;

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {SCHEMA_V7} CASCADE"))
        .execute(&setup_pool)
        .await
        .expect("dropping throwaway schema");

    test_result.expect("test assertions");
}

/// The v7 -> v8 step: per-row `audience` columns on `processes`, `streams`, and `blocks`,
/// nullable with no backfill, each guarded by a `NOT VALID` `CHECK` mirroring
/// `ingestion_api_keys_audience_name`'s charset. `execute_migration` against a v7 database adds
/// the three columns and constraints; a pre-existing row keeps `audience = NULL`, while a fresh
/// insert is subject to the `CHECK` going forward.
const SCHEMA_V8: &str = "mm_1518_sql_migration_test_schema";

#[ignore]
#[tokio::test]
async fn v8_adds_nullable_unbackfilled_audience_columns_with_not_valid_checks() {
    let base_conn_str = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .expect("MICROMEGAS_SQL_CONNECTION_STRING must point at a live Postgres");

    let setup_pool = sqlx::PgPool::connect(&base_conn_str)
        .await
        .expect("connecting to metadata Postgres");
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {SCHEMA_V8} CASCADE"))
        .execute(&setup_pool)
        .await
        .expect("dropping any stale throwaway schema from a previous failed run");
    sqlx::query(&format!("CREATE SCHEMA {SCHEMA_V8}"))
        .execute(&setup_pool)
        .await
        .expect("creating throwaway schema");

    let opts = sqlx::postgres::PgConnectOptions::from_str(&base_conn_str)
        .expect("valid connection string")
        .options([("search_path", SCHEMA_V8)]);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(opts)
        .await
        .expect("connecting with a throwaway search_path");

    let test_result: Result<(), String> = async {
        build_v7_schema(&pool).await;
        if schema_version(&pool).await != 7 {
            return Err("expected the throwaway schema to start at v7".to_string());
        }

        // Pre-v8 rows: no `audience` column exists yet, so there is nothing to set.
        let process_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO processes (process_id, exe, username, realname, computer, distro,
              cpu_brand, tsc_frequency, start_time, start_ticks, insert_time, parent_process_id,
              properties)
             VALUES ($1, 'exe', 'user', 'user', 'computer', 'distro', 'cpu', 0, now(), 0, now(),
              NULL, ARRAY[]::micromegas_property[])",
        )
        .bind(process_id)
        .execute(&pool)
        .await
        .map_err(|e| format!("seeding a v7-era processes row: {e:#}"))?;

        let stream_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO streams (stream_id, process_id, dependencies_metadata, objects_metadata,
              tags, properties, insert_time, format)
             VALUES ($1, $2, '', '', ARRAY[]::TEXT[], ARRAY[]::micromegas_property[], now(),
              'micromegas-transit')",
        )
        .bind(stream_id)
        .bind(process_id)
        .execute(&pool)
        .await
        .map_err(|e| format!("seeding a v7-era streams row: {e:#}"))?;

        let block_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO blocks (block_id, stream_id, process_id, begin_time, begin_ticks,
              end_time, end_ticks, nb_objects, object_offset, payload_size, insert_time)
             VALUES ($1, $2, $3, now(), 0, now(), 0, 0, 0, 0, now())",
        )
        .bind(block_id)
        .bind(stream_id)
        .bind(process_id)
        .execute(&pool)
        .await
        .map_err(|e| format!("seeding a v7-era blocks row: {e:#}"))?;

        execute_migration(pool.clone())
            .await
            .map_err(|e| format!("migrating v7 -> v8: {e:#}"))?;

        let version = schema_version(&pool).await;
        if version != LATEST_DATA_LAKE_SCHEMA_VERSION {
            return Err(format!(
                "expected schema version {LATEST_DATA_LAKE_SCHEMA_VERSION} after migrating, got {version} \
                 (a forgotten `UPDATE migration SET version=8;` would surface as a startup panic \
                 in production, not a graceful error -- this is what catches it here instead)"
            ));
        }

        // No `DEFAULT`, no backfill: every pre-v8 row reads back NULL.
        let process_audience: Option<String> =
            sqlx::query("SELECT audience FROM processes WHERE process_id = $1")
                .bind(process_id)
                .fetch_one(&pool)
                .await
                .map_err(|e| format!("reading back the processes row: {e:#}"))?
                .try_get("audience")
                .map_err(|e| format!("reading processes.audience: {e:#}"))?;
        if process_audience.is_some() {
            return Err(format!(
                "expected the pre-v8 processes row's audience to stay NULL (no backfill), \
                 got {process_audience:?}"
            ));
        }

        let stream_audience: Option<String> =
            sqlx::query("SELECT audience FROM streams WHERE stream_id = $1")
                .bind(stream_id)
                .fetch_one(&pool)
                .await
                .map_err(|e| format!("reading back the streams row: {e:#}"))?
                .try_get("audience")
                .map_err(|e| format!("reading streams.audience: {e:#}"))?;
        if stream_audience.is_some() {
            return Err(format!(
                "expected the pre-v8 streams row's audience to stay NULL (no backfill), \
                 got {stream_audience:?}"
            ));
        }

        let block_audience: Option<String> =
            sqlx::query("SELECT audience FROM blocks WHERE block_id = $1")
                .bind(block_id)
                .fetch_one(&pool)
                .await
                .map_err(|e| format!("reading back the blocks row: {e:#}"))?
                .try_get("audience")
                .map_err(|e| format!("reading blocks.audience: {e:#}"))?;
        if block_audience.is_some() {
            return Err(format!(
                "expected the pre-v8 blocks row's audience to stay NULL (no backfill), \
                 got {block_audience:?}"
            ));
        }

        // The `NOT VALID` CHECK does not block a fresh, well-formed write ...
        sqlx::query(
            "INSERT INTO processes (process_id, exe, username, realname, computer, distro,
              cpu_brand, tsc_frequency, start_time, start_ticks, insert_time, parent_process_id,
              properties, audience)
             VALUES ($1, 'exe', 'user', 'user', 'computer', 'distro', 'cpu', 0, now(), 0, now(),
              NULL, ARRAY[]::micromegas_property[], 'team-alpha')",
        )
        .bind(Uuid::new_v4())
        .execute(&pool)
        .await
        .map_err(|e| format!("a well-formed audience should be accepted on processes: {e:#}"))?;

        // ... but rejects a malformed one, on all three tables (mirroring the v6/v7 tests'
        // own charset case rather than re-enumerating the full accept/reject table).
        let bad_process = sqlx::query(
            "INSERT INTO processes (process_id, exe, username, realname, computer, distro,
              cpu_brand, tsc_frequency, start_time, start_ticks, insert_time, parent_process_id,
              properties, audience)
             VALUES ($1, 'exe', 'user', 'user', 'computer', 'distro', 'cpu', 0, now(), 0, now(),
              NULL, ARRAY[]::micromegas_property[], 'group:everyone')",
        )
        .bind(Uuid::new_v4())
        .execute(&pool)
        .await;
        if bad_process.is_ok() {
            return Err("expected the processes_audience_name CHECK to reject 'group:everyone'".to_string());
        }

        let bad_stream = sqlx::query(
            "INSERT INTO streams (stream_id, process_id, dependencies_metadata, objects_metadata,
              tags, properties, insert_time, format, audience)
             VALUES ($1, $2, '', '', ARRAY[]::TEXT[], ARRAY[]::micromegas_property[], now(),
              'micromegas-transit', 'group:everyone')",
        )
        .bind(Uuid::new_v4())
        .bind(process_id)
        .execute(&pool)
        .await;
        if bad_stream.is_ok() {
            return Err("expected the streams_audience_name CHECK to reject 'group:everyone'".to_string());
        }

        let bad_block = sqlx::query(
            "INSERT INTO blocks (block_id, stream_id, process_id, begin_time, begin_ticks,
              end_time, end_ticks, nb_objects, object_offset, payload_size, insert_time, audience)
             VALUES ($1, $2, $3, now(), 0, now(), 0, 0, 0, 0, now(), 'group:everyone')",
        )
        .bind(Uuid::new_v4())
        .bind(stream_id)
        .bind(process_id)
        .execute(&pool)
        .await;
        if bad_block.is_ok() {
            return Err("expected the blocks_audience_name CHECK to reject 'group:everyone'".to_string());
        }

        Ok(())
    }
    .await;

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {SCHEMA_V8} CASCADE"))
        .execute(&setup_pool)
        .await
        .expect("dropping throwaway schema");

    test_result.expect("test assertions");
}

/// The v8 -> v9 step: seeding `('public', 'read', '*')` and `('public', 'mint', '*')` into
/// `audience_grants`. `execute_migration` against a v8 database ends with exactly those two rows
/// and `LATEST_DATA_LAKE_SCHEMA_VERSION`; running it again is a no-op (`ON CONFLICT DO NOTHING`);
/// and an operator who already created either row by hand before upgrading keeps their own
/// `created_by`, rather than the migration overwriting or duplicating it.
const SCHEMA_V9: &str = "mm_1535_sql_migration_test_schema";

#[ignore]
#[tokio::test]
async fn v9_seeds_public_read_and_mint_grants() {
    let base_conn_str = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .expect("MICROMEGAS_SQL_CONNECTION_STRING must point at a live Postgres");

    let setup_pool = sqlx::PgPool::connect(&base_conn_str)
        .await
        .expect("connecting to metadata Postgres");
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {SCHEMA_V9} CASCADE"))
        .execute(&setup_pool)
        .await
        .expect("dropping any stale throwaway schema from a previous failed run");
    sqlx::query(&format!("CREATE SCHEMA {SCHEMA_V9}"))
        .execute(&setup_pool)
        .await
        .expect("creating throwaway schema");

    let opts = sqlx::postgres::PgConnectOptions::from_str(&base_conn_str)
        .expect("valid connection string")
        .options([("search_path", SCHEMA_V9)]);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(opts)
        .await
        .expect("connecting with a throwaway search_path");

    let test_result: Result<(), String> = async {
        build_v8_schema(&pool).await;
        if schema_version(&pool).await != 8 {
            return Err("expected the throwaway schema to start at v8".to_string());
        }

        // An operator who already created the read row by hand, under their own identity --
        // the migration must not duplicate or overwrite this.
        sqlx::query(
            "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
             VALUES ('public', 'read', '*', now(), 'an-operator')",
        )
        .execute(&pool)
        .await
        .map_err(|e| format!("seeding a pre-existing public/read/* row: {e:#}"))?;

        execute_migration(pool.clone())
            .await
            .map_err(|e| format!("migrating v8 -> v9: {e:#}"))?;

        let version = schema_version(&pool).await;
        if version != LATEST_DATA_LAKE_SCHEMA_VERSION {
            return Err(format!(
                "expected schema version {LATEST_DATA_LAKE_SCHEMA_VERSION} after migrating, got {version} \
                 (a forgotten `UPDATE migration SET version=9;` would surface as a startup panic \
                 in production, not a graceful error -- this is what catches it here instead)"
            ));
        }

        let rows = sqlx::query(
            "SELECT audience, axis, selector, created_by FROM audience_grants ORDER BY axis",
        )
        .fetch_all(&pool)
        .await
        .map_err(|e| format!("reading back audience_grants: {e:#}"))?;
        if rows.len() != 2 {
            return Err(format!(
                "expected exactly the two seeded rows, got {} rows",
                rows.len()
            ));
        }

        let mint_row = &rows[0];
        if mint_row.get::<String, _>("axis") != "mint"
            || mint_row.get::<String, _>("audience") != "public"
            || mint_row.get::<String, _>("selector") != "*"
            || mint_row.get::<String, _>("created_by") != "default"
        {
            return Err("expected the mint row to be ('public', 'mint', '*', 'default')".to_string());
        }

        let read_row = &rows[1];
        if read_row.get::<String, _>("axis") != "read"
            || read_row.get::<String, _>("audience") != "public"
            || read_row.get::<String, _>("selector") != "*"
            || read_row.get::<String, _>("created_by") != "an-operator"
        {
            return Err(
                "expected the pre-existing read row's created_by ('an-operator') to survive \
                 the migration untouched, via ON CONFLICT DO NOTHING"
                    .to_string(),
            );
        }

        // Running the migration again is a no-op: still exactly two rows, unchanged.
        execute_migration(pool.clone())
            .await
            .map_err(|e| format!("re-running execute_migration: {e:#}"))?;
        let count: i64 = sqlx::query("SELECT count(*) AS c FROM audience_grants")
            .fetch_one(&pool)
            .await
            .map_err(|e| format!("counting audience_grants after a second migration run: {e:#}"))?
            .try_get("c")
            .map_err(|e| format!("reading count: {e:#}"))?;
        if count != 2 {
            return Err(format!(
                "expected re-running the migration to be a no-op, got {count} rows"
            ));
        }

        Ok(())
    }
    .await;

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {SCHEMA_V9} CASCADE"))
        .execute(&setup_pool)
        .await
        .expect("dropping throwaway schema");

    test_result.expect("test assertions");
}
