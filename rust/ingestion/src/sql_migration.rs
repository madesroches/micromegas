use crate::sql_telemetry_db::create_tables;
use anyhow::{Context, Result, anyhow};
use micromegas_tracing::prelude::*;
use sqlx::Executor;
use sqlx::Row;

/// The latest schema version for the data lake.
pub const LATEST_DATA_LAKE_SCHEMA_VERSION: i32 = 10;

/// Reads the current schema version from the database.
pub async fn read_data_lake_schema_version(tr: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> i32 {
    match sqlx::query(
        "SELECT version
         FROM migration;",
    )
    .fetch_one(&mut **tr)
    .await
    {
        Ok(row) => row.get("version"),
        Err(e) => {
            info!(
                "Error reading data lake schema version, assuming version 0: {}",
                e
            );
            0
        }
    }
}

/// Warns when the connected database is behind [`LATEST_DATA_LAKE_SCHEMA_VERSION`].
///
/// `execute_migration` only runs from `telemetry-ingestion-srv`/`micromegas-monolith`; a
/// `flight-sql-srv`/`analytics-web-srv` process reads the same database without ever migrating
/// it. On a schema that already has the `audience_grants` table (v7+) but predates the v9 seed
/// rows, `AudienceReadPolicy` resolves every audience to an empty grant set rather than erroring,
/// so queries silently return zero rows -- this is the only signal an operator gets otherwise.
pub async fn warn_if_data_lake_schema_stale(pool: &sqlx::Pool<sqlx::Postgres>) {
    let version = match pool.begin().await {
        Ok(mut tr) => read_data_lake_schema_version(&mut tr).await,
        Err(e) => {
            warn!("could not check data lake schema version: {e}");
            return;
        }
    };
    if version < LATEST_DATA_LAKE_SCHEMA_VERSION {
        warn!(
            "data lake schema is v{version}, behind the latest v{LATEST_DATA_LAKE_SCHEMA_VERSION}; \
             the seeded 'public' audience_grants rows have not migrated in yet, so every query \
             will resolve to an empty read scope and return zero rows until the schema is upgraded; \
             on a schema older than v10 the `groups`/`group_members` tables do not exist yet, so \
             every request fails with a retryable 503 until the migration runs, because the group \
             store cannot load"
        );
    }
}

/// Upgrades the data lake schema to version 2.
pub async fn upgrade_data_lake_schema_v2(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    tr.execute("ALTER TABLE blocks ADD insert_time TIMESTAMPTZ;")
        .await
        .with_context(|| "adding column insert_time to blocks table")?;
    tr.execute("UPDATE blocks SET insert_time=end_time WHERE insert_time is NULL;")
        .await
        .with_context(|| "use end_time as insert_time to backfill missing data")?;
    tr.execute("CREATE INDEX block_begin_time on blocks(begin_time);")
        .await
        .with_context(|| "adding index block_begin_time")?;
    tr.execute("CREATE INDEX block_end_time on blocks(end_time);")
        .await
        .with_context(|| "adding index block_end_time")?;
    tr.execute("CREATE INDEX block_insert_time on blocks(insert_time);")
        .await
        .with_context(|| "adding index block_insert_time")?;
    tr.execute("CREATE INDEX process_insert_time on processes(insert_time);")
        .await
        .with_context(|| "adding index process_insert_time")?;
    tr.execute("UPDATE migration SET version=2;")
        .await
        .with_context(|| "Updating data lake schema version to 2")?;
    Ok(())
}

/// Upgrades the data lake schema to version 3.
/// Drops old non-unique indexes (superseded by the unique indexes created before this transaction).
pub async fn upgrade_data_lake_schema_v3(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    tr.execute("DROP INDEX IF EXISTS process_id;")
        .await
        .with_context(|| "dropping old non-unique index process_id")?;
    tr.execute("DROP INDEX IF EXISTS stream_id;")
        .await
        .with_context(|| "dropping old non-unique index stream_id")?;
    tr.execute("DROP INDEX IF EXISTS block_id;")
        .await
        .with_context(|| "dropping old non-unique index block_id")?;
    tr.execute("UPDATE migration SET version=3;")
        .await
        .with_context(|| "updating data lake schema version to 3")?;
    Ok(())
}

/// Upgrades the data lake schema to version 4.
/// Adds the `format` column to `streams` so OTLP and native blocks can be distinguished.
pub async fn upgrade_data_lake_schema_v4(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    tr.execute("ALTER TABLE streams ADD COLUMN format TEXT NOT NULL DEFAULT 'micromegas-transit';")
        .await
        .with_context(|| "adding column format to streams table")?;
    tr.execute("UPDATE migration SET version=4;")
        .await
        .with_context(|| "updating data lake schema version to 4")?;
    Ok(())
}

/// Upgrades the data lake schema to version 5.
/// Adds `ingestion_api_keys` and `analytics_api_keys`, the DB-backed API key store:
/// a `key_id` UUID primary key (a non-secret handle for `DELETE`/`GET`,
/// distinct from the `key_hash` lookup value), a SHA-256 `key_hash` with a unique
/// index for O(1) validation lookups, and a `created_at`/`created_by`/`last_used_at`/
/// `revoked_at`/`revoked_by` audit trail. The two tables are identical in shape but
/// deliberately separate (never a shared `scopes` column) so an ingestion (write)
/// key can never double as an analytics (read) credential.
pub async fn upgrade_data_lake_schema_v5(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    tr.execute(
        "CREATE TABLE ingestion_api_keys (
           key_id       UUID PRIMARY KEY,
           key_hash     BYTEA NOT NULL,
           name         VARCHAR(255) NOT NULL,
           created_at   TIMESTAMPTZ NOT NULL,
           created_by   VARCHAR(255) NOT NULL,
           last_used_at TIMESTAMPTZ,
           revoked_at   TIMESTAMPTZ,
           revoked_by   VARCHAR(255)
         );",
    )
    .await
    .with_context(|| "creating table ingestion_api_keys")?;
    tr.execute("CREATE UNIQUE INDEX ingestion_api_keys_key_hash ON ingestion_api_keys(key_hash);")
        .await
        .with_context(|| "creating unique index ingestion_api_keys_key_hash")?;
    tr.execute(
        "CREATE TABLE analytics_api_keys (
           key_id       UUID PRIMARY KEY,
           key_hash     BYTEA NOT NULL,
           name         VARCHAR(255) NOT NULL,
           created_at   TIMESTAMPTZ NOT NULL,
           created_by   VARCHAR(255) NOT NULL,
           last_used_at TIMESTAMPTZ,
           revoked_at   TIMESTAMPTZ,
           revoked_by   VARCHAR(255)
         );",
    )
    .await
    .with_context(|| "creating table analytics_api_keys")?;
    tr.execute("CREATE UNIQUE INDEX analytics_api_keys_key_hash ON analytics_api_keys(key_hash);")
        .await
        .with_context(|| "creating unique index analytics_api_keys_key_hash")?;
    tr.execute("UPDATE migration SET version=5;")
        .await
        .with_context(|| "updating data lake schema version to 5")?;
    Ok(())
}

/// Upgrades the data lake schema to version 6.
/// Adds the `audience` column to `ingestion_api_keys`: the write audience a key is
/// immutably bound to. Backfilled to `'public'` before `SET NOT NULL` -- every pre-existing
/// row predates audience stamping, and `public` is the accurate description of its current,
/// unstamped-and-visible-to-everyone state, not a new grant. No `DEFAULT` on the column:
/// that would let a not-yet-upgraded `analytics-web-srv` keep inserting rows that silently
/// take `public`, defeating the column's fail-closed property. `analytics_api_keys` is
/// untouched -- its read-side mirror is `read_audiences`, a set-valued grant in the opposite
/// direction.
pub async fn upgrade_data_lake_schema_v6(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    tr.execute("ALTER TABLE ingestion_api_keys ADD COLUMN audience VARCHAR(255);")
        .await
        .with_context(|| "adding column audience to ingestion_api_keys table")?;
    tr.execute("UPDATE ingestion_api_keys SET audience = 'public' WHERE audience IS NULL;")
        .await
        .with_context(|| "backfilling audience to 'public' on ingestion_api_keys")?;
    tr.execute("ALTER TABLE ingestion_api_keys ALTER COLUMN audience SET NOT NULL;")
        .await
        .with_context(|| "setting audience NOT NULL on ingestion_api_keys")?;
    tr.execute(
        "ALTER TABLE ingestion_api_keys ADD CONSTRAINT ingestion_api_keys_audience_name \
         CHECK (audience ~ '^[A-Za-z0-9_-]+$');",
    )
    .await
    .with_context(|| "adding audience-name CHECK constraint on ingestion_api_keys")?;
    tr.execute("UPDATE migration SET version=6;")
        .await
        .with_context(|| "updating data lake schema version to 6")?;
    Ok(())
}

/// Upgrades the data lake schema to version 7.
/// Adds `audience_grants`: the DB-backed audience grant store, a 1:1
/// stand-in for the long-term model's `group_read_grants`/`group_mint_grants` tables. One table
/// with an `axis` column (`'read'`/`'mint'`) rather than two, mirroring today's single env-var
/// grant map (`{prefix}_AUDIENCE_GRANTS`); `(audience, axis, selector)` is the row's own natural
/// key, so there is no surrogate `grant_id` the way `ingestion_api_keys.key_id` has one (a grant
/// carries no secret to keep unlinkable from its own identity). No `revoked_at`/`revoked_by`
/// either -- grants are hard-`DELETE`d, since a removed grant has no ongoing artifact whose
/// provenance a caller might later need. The two `CHECK` constraints mirror the same validation
/// `AudienceGrants::from_rows` (`rust/auth/src/policy.rs`) re-runs independently in Rust, so a row
/// inserted by any means other than the admin API (a manual `psql` fix, a future migration) still
/// can't produce an unparseable or unreadable grant silently.
pub async fn upgrade_data_lake_schema_v7(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    tr.execute(
        "CREATE TABLE audience_grants (
           audience   VARCHAR(255) NOT NULL,
           axis       VARCHAR(4) NOT NULL CHECK (axis IN ('read', 'mint')),
           selector   VARCHAR(255) NOT NULL,
           created_at TIMESTAMPTZ NOT NULL,
           created_by VARCHAR(255) NOT NULL,
           PRIMARY KEY (audience, axis, selector),
           CONSTRAINT audience_grants_audience_name CHECK (audience ~ '^[A-Za-z0-9_-]+$'),
           CONSTRAINT audience_grants_selector_shape
               CHECK (selector = '*' OR selector ~ '^(user|group):.+$')
         );",
    )
    .await
    .with_context(|| "creating table audience_grants")?;
    tr.execute("UPDATE migration SET version=7;")
        .await
        .with_context(|| "updating data lake schema version to 7")?;
    Ok(())
}

/// Upgrades the data lake schema to version 8.
///
/// Adds a nullable `audience` column to `processes`, `streams`, and `blocks`: the write audience
/// the row was stamped with at insert time. No `DEFAULT` and no backfill -- a NULL column means
/// the row predates this stage and resolves to the deployment's `MICROMEGAS_DEFAULT_AUDIENCE` at
/// read time, exactly as an unstamped row does today.
///
/// The `CHECK` constraints are added `NOT VALID` so `ALTER TABLE` does not scan and lock every
/// existing row on `blocks`, the largest table in the lake; the constraint only applies to rows
/// written from this point on (`WriteAudience::new` already validates the charset in Rust).
///
/// No index: nothing queries Postgres by audience, so one would be pure write cost.
///
/// `sql_telemetry_db.rs`'s `create_tables` (the v1 shape) is not touched -- a fresh database
/// walks every upgrade in turn, so adding the column there too would double-apply it.
pub async fn upgrade_data_lake_schema_v8(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    tr.execute("ALTER TABLE processes ADD COLUMN audience VARCHAR(255);")
        .await
        .with_context(|| "adding column audience to processes table")?;
    tr.execute("ALTER TABLE streams ADD COLUMN audience VARCHAR(255);")
        .await
        .with_context(|| "adding column audience to streams table")?;
    tr.execute("ALTER TABLE blocks ADD COLUMN audience VARCHAR(255);")
        .await
        .with_context(|| "adding column audience to blocks table")?;
    tr.execute(
        "ALTER TABLE processes ADD CONSTRAINT processes_audience_name \
         CHECK (audience ~ '^[A-Za-z0-9_-]+$') NOT VALID;",
    )
    .await
    .with_context(|| "adding audience-name CHECK constraint on processes")?;
    tr.execute(
        "ALTER TABLE streams ADD CONSTRAINT streams_audience_name \
         CHECK (audience ~ '^[A-Za-z0-9_-]+$') NOT VALID;",
    )
    .await
    .with_context(|| "adding audience-name CHECK constraint on streams")?;
    tr.execute(
        "ALTER TABLE blocks ADD CONSTRAINT blocks_audience_name \
         CHECK (audience ~ '^[A-Za-z0-9_-]+$') NOT VALID;",
    )
    .await
    .with_context(|| "adding audience-name CHECK constraint on blocks")?;
    tr.execute("UPDATE migration SET version=8;")
        .await
        .with_context(|| "updating data lake schema version to 8")?;
    Ok(())
}

/// Upgrades the data lake schema to version 9.
///
/// Seeds `('public', 'read', '*')` and `('public', 'mint', '*')` into `audience_grants`. The
/// read row replaces the built-in `PUBLIC_AUDIENCE` arm removed from `AudienceReadPolicy::resolve`;
/// the mint row is what lets a non-admin mint a key bound to `public` without an operator having
/// to discover and create the row by hand. `ON CONFLICT DO NOTHING` because an operator may
/// already have created either row before upgrading -- this must not fail the migration.
pub async fn upgrade_data_lake_schema_v9(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    tr.execute(
        "INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
         VALUES ('public', 'read', '*', now(), 'default'),
                ('public', 'mint', '*', now(), 'default')
         ON CONFLICT DO NOTHING;",
    )
    .await
    .with_context(|| "seeding public read/mint grants")?;
    tr.execute("UPDATE migration SET version=9;")
        .await
        .with_context(|| "updating data lake schema version to 9")?;
    Ok(())
}

/// The resolved seeding mode for the `admins` group's members on a v10 upgrade, decoded from the
/// analytics admin var by [`AdminSeed::parse`]/[`admin_seed_from_env`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminSeed {
    /// The admin var was set and held a non-empty JSON array: one `user:<entry>` row per entry,
    /// no wildcard. Who is an admin does not change from today.
    Users(Vec<String>),
    /// The admin var was unset, or held an empty JSON array (`[]`): one `('admins', '*')` row --
    /// a fresh install, or an upgrade that never set the var.
    Everyone,
}

impl AdminSeed {
    /// Decodes a raw env-var value into an `AdminSeed`. `Err` on malformed JSON or a JSON value
    /// that is not an array -- deliberately not `load_admin_users`'s old `unwrap_or_default()`
    /// pattern, which would let a JSON typo silently seed the wildcard instead of the operator's
    /// intended admins. A valid empty array (`[]`) parses to `Everyone`, preserving today's
    /// effective state (an unset or empty admin list already means every caller can be admin).
    pub fn parse(raw: &str) -> Result<Self> {
        let value: serde_json::Value =
            serde_json::from_str(raw).map_err(|e| anyhow!("invalid admin seed JSON: {e}"))?;
        let arr = value
            .as_array()
            .ok_or_else(|| anyhow!("admin seed value must be a JSON array"))?;
        if arr.is_empty() {
            return Ok(AdminSeed::Everyone);
        }
        let mut users = Vec::with_capacity(arr.len());
        for entry in arr {
            let s = entry
                .as_str()
                .ok_or_else(|| anyhow!("admin seed array must contain only strings"))?;
            users.push(s.to_string());
        }
        Ok(AdminSeed::Users(users))
    }
}

/// Resolves the v10 seeding mode from `MICROMEGAS_ANALYTICS_ADMINS`, falling back to
/// `MICROMEGAS_ADMINS` -- the **analytics** var only, since a principal listed only in
/// `MICROMEGAS_INGESTION_ADMINS` has no admin capability today. If `MICROMEGAS_INGESTION_ADMINS`
/// holds entries the analytics var lacks, `warn!` naming the dropped entries. Called from
/// `execute_migration`, not from [`upgrade_data_lake_schema_v10`] itself, which keeps that
/// function free of env access.
pub fn admin_seed_from_env() -> Result<AdminSeed> {
    let raw = std::env::var("MICROMEGAS_ANALYTICS_ADMINS")
        .or_else(|_| std::env::var("MICROMEGAS_ADMINS"));
    let seed = match raw {
        Ok(raw) => AdminSeed::parse(&raw)?,
        Err(_) => AdminSeed::Everyone,
    };

    if let Ok(ingestion_raw) = std::env::var("MICROMEGAS_INGESTION_ADMINS")
        && let Ok(AdminSeed::Users(ingestion_users)) = AdminSeed::parse(&ingestion_raw)
    {
        let analytics_users: &[String] = match &seed {
            AdminSeed::Users(users) => users,
            AdminSeed::Everyone => &[],
        };
        let dropped: Vec<&String> = ingestion_users
            .iter()
            .filter(|u| !analytics_users.contains(u))
            .collect();
        if !dropped.is_empty() {
            warn!(
                "MICROMEGAS_INGESTION_ADMINS holds entries not present in the analytics admin \
                 var; dropped (no admin capability today, so not seeded): {dropped:?}"
            );
        }
    }

    Ok(seed)
}

/// `true` if `name` matches the `groups.name`/`group_members.group_name` charset:
/// `[A-Za-z0-9_-]{1,255}`. A local copy rather than a dependency on `micromegas-auth` (which this
/// crate does not otherwise depend on).
fn is_valid_group_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Upgrades the data lake schema to version 10.
///
/// Adds `groups` and `group_members`, the local-group-membership store: a `group_members.member`
/// row is a selector in exactly the vocabulary `audience_grants.selector` uses (`*`,
/// `user:<email>`, `group:<name>`), so nesting is the `group:` arm of the same predicate rather
/// than a special case. Seeds the reserved `admins` group from `seed` (resolved by
/// `admin_seed_from_env`, outside this function, so this step stays free of env access), and
/// backfills an empty group for every distinct `group:X` selector already present in
/// `audience_grants` so a legacy claim-derived grant is not left dangling at an unknown name.
pub async fn upgrade_data_lake_schema_v10(
    tr: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    seed: &AdminSeed,
) -> Result<()> {
    tr.execute(
        "CREATE TABLE groups (
           name        VARCHAR(255) PRIMARY KEY,
           description TEXT,
           created_at  TIMESTAMPTZ NOT NULL,
           created_by  VARCHAR(255) NOT NULL,
           CONSTRAINT groups_name CHECK (name ~ '^[A-Za-z0-9_-]+$')
         );",
    )
    .await
    .with_context(|| "creating table groups")?;
    tr.execute(
        "CREATE TABLE group_members (
           group_name  VARCHAR(255) NOT NULL REFERENCES groups(name) ON DELETE CASCADE,
           member      VARCHAR(255) NOT NULL,
           created_at  TIMESTAMPTZ NOT NULL,
           created_by  VARCHAR(255) NOT NULL,
           PRIMARY KEY (group_name, member),
           CONSTRAINT group_members_selector_shape CHECK (member = '*' OR member ~ '^(user|group):.+$')
         );",
    )
    .await
    .with_context(|| "creating table group_members")?;

    tr.execute(
        "INSERT INTO groups (name, description, created_at, created_by)
         VALUES ('admins', 'Deployment administrators', now(), 'default')
         ON CONFLICT (name) DO NOTHING;",
    )
    .await
    .with_context(|| "seeding the admins group")?;

    match seed {
        AdminSeed::Users(users) => {
            for user in users {
                if !user.contains('@') {
                    warn!(
                        "admin seed entry {user:?} does not look like an email; seeded verbatim \
                         as a `user:` member, which matches AuthContext.email only -- it will \
                         never match a subject-shaped identity"
                    );
                }
                sqlx::query(
                    "INSERT INTO group_members (group_name, member, created_at, created_by)
                     VALUES ('admins', $1, now(), 'default')
                     ON CONFLICT (group_name, member) DO NOTHING;",
                )
                .bind(format!("user:{user}"))
                .execute(&mut **tr)
                .await
                .with_context(|| format!("seeding admins member user:{user}"))?;
            }
        }
        AdminSeed::Everyone => {
            tr.execute(
                "INSERT INTO group_members (group_name, member, created_at, created_by)
                 VALUES ('admins', '*', now(), 'default')
                 ON CONFLICT (group_name, member) DO NOTHING;",
            )
            .await
            .with_context(|| "seeding the admins group with the wildcard member")?;
        }
    }

    let rows =
        sqlx::query("SELECT DISTINCT selector FROM audience_grants WHERE selector LIKE 'group:%';")
            .fetch_all(&mut **tr)
            .await
            .with_context(|| "querying distinct group: selectors from audience_grants")?;
    for row in rows {
        let selector: String = row.try_get("selector").context("reading selector")?;
        let name = selector
            .strip_prefix("group:")
            .expect("selector matched the LIKE 'group:%' filter");
        if !is_valid_group_name(name) {
            warn!(
                "legacy audience_grants selector group:{name:?} does not match the group name \
                 charset [A-Za-z0-9_-]{{1,255}}; left as an inert grant row, not backfilled into \
                 a group"
            );
            continue;
        }
        sqlx::query(
            "INSERT INTO groups (name, description, created_at, created_by)
             VALUES ($1, NULL, now(), 'migration')
             ON CONFLICT (name) DO NOTHING;",
        )
        .bind(name)
        .execute(&mut **tr)
        .await
        .with_context(|| format!("backfilling empty group {name:?} from a legacy grant"))?;
        info!("backfilled empty group {name:?} from a legacy audience_grants selector");
    }

    tr.execute("UPDATE migration SET version=10;")
        .await
        .with_context(|| "updating data lake schema version to 10")?;
    Ok(())
}

/// Checks whether a specific index is valid in `pg_index`.
/// If the index is invalid, drops it and returns `Ok(false)`.
/// If valid, returns `Ok(true)`.
/// Returns an error if the index does not exist.
async fn check_index_is_valid(pool: &sqlx::Pool<sqlx::Postgres>, index_name: &str) -> Result<bool> {
    let row = sqlx::query(
        "SELECT i.indisvalid
         FROM pg_class c
         JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = $1;",
    )
    .bind(index_name)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("querying pg_index for {index_name}"))?;

    let row = row.with_context(|| format!("index {index_name} not found in pg_class"))?;
    let is_valid: bool = row.get("indisvalid");

    if !is_valid {
        info!("index {index_name} is INVALID, dropping it");
        sqlx::query(&format!("DROP INDEX IF EXISTS {index_name}"))
            .execute(pool)
            .await
            .with_context(|| format!("dropping invalid index {index_name}"))?;
        return Ok(false);
    }

    Ok(true)
}

/// Validates that all three unique indexes created during the v2→v3 migration are valid.
/// Drops any invalid indexes and returns an error so the migration can be retried.
async fn validate_unique_indexes(pool: &sqlx::Pool<sqlx::Postgres>) -> Result<()> {
    let index_names = [
        "processes_process_id_unique",
        "streams_stream_id_unique",
        "blocks_block_id_unique",
    ];

    let mut invalid_indexes = Vec::new();
    for name in &index_names {
        if !check_index_is_valid(pool, name).await? {
            invalid_indexes.push(*name);
        }
    }

    if !invalid_indexes.is_empty() {
        anyhow::bail!(
            "invalid indexes detected and dropped: {}. The migration will be retried on next startup.",
            invalid_indexes.join(", ")
        );
    }

    Ok(())
}

/// Executes the database migration.
pub async fn execute_migration(pool: sqlx::Pool<sqlx::Postgres>) -> Result<()> {
    let mut current_version = read_data_lake_schema_version(&mut pool.begin().await?).await;
    if 0 == current_version {
        info!("creating v1 data_lake_schema");
        let mut tr = pool.begin().await?;
        create_tables(&mut tr).await?;
        current_version = read_data_lake_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    if 1 == current_version {
        info!("upgrading data_lake_schema to v2");
        let mut tr = pool.begin().await?;
        upgrade_data_lake_schema_v2(&mut tr).await?;
        current_version = read_data_lake_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    if 2 == current_version {
        info!("upgrading data_lake_schema to v3");
        // CREATE UNIQUE INDEX CONCURRENTLY cannot run inside a transaction.
        // Run these outside any transaction, then do the rest in a transaction.
        // IF NOT EXISTS makes this idempotent and safe for retries.
        sqlx::query("CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS processes_process_id_unique ON processes(process_id);")
            .execute(&pool)
            .await
            .with_context(|| "creating unique index on processes(process_id)")?;
        sqlx::query("CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS streams_stream_id_unique ON streams(stream_id);")
            .execute(&pool)
            .await
            .with_context(|| "creating unique index on streams(stream_id)")?;
        sqlx::query("CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS blocks_block_id_unique ON blocks(block_id);")
            .execute(&pool)
            .await
            .with_context(|| "creating unique index on blocks(block_id)")?;

        validate_unique_indexes(&pool).await?;

        let mut tr = pool.begin().await?;
        upgrade_data_lake_schema_v3(&mut tr).await?;
        current_version = read_data_lake_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    if 3 == current_version {
        info!("upgrading data_lake_schema to v4");
        let mut tr = pool.begin().await?;
        upgrade_data_lake_schema_v4(&mut tr).await?;
        current_version = read_data_lake_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    if 4 == current_version {
        info!("upgrading data_lake_schema to v5");
        let mut tr = pool.begin().await?;
        upgrade_data_lake_schema_v5(&mut tr).await?;
        current_version = read_data_lake_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    if 5 == current_version {
        info!("upgrading data_lake_schema to v6");
        let mut tr = pool.begin().await?;
        upgrade_data_lake_schema_v6(&mut tr).await?;
        current_version = read_data_lake_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    if 6 == current_version {
        info!("upgrading data_lake_schema to v7");
        let mut tr = pool.begin().await?;
        upgrade_data_lake_schema_v7(&mut tr).await?;
        current_version = read_data_lake_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    if 7 == current_version {
        info!("upgrading data_lake_schema to v8");
        let mut tr = pool.begin().await?;
        upgrade_data_lake_schema_v8(&mut tr).await?;
        current_version = read_data_lake_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    if 8 == current_version {
        info!("upgrading data_lake_schema to v9");
        let mut tr = pool.begin().await?;
        upgrade_data_lake_schema_v9(&mut tr).await?;
        current_version = read_data_lake_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    if 9 == current_version {
        info!("upgrading data_lake_schema to v10");
        let seed = admin_seed_from_env()?;
        info!(
            "v10 admins seeding mode: {}",
            match &seed {
                AdminSeed::Users(users) => format!("Users({} entries)", users.len()),
                AdminSeed::Everyone => "Everyone".to_string(),
            }
        );
        let mut tr = pool.begin().await?;
        upgrade_data_lake_schema_v10(&mut tr, &seed).await?;
        current_version = read_data_lake_schema_version(&mut tr).await;
        tr.commit().await?;
    }
    assert_eq!(current_version, LATEST_DATA_LAKE_SCHEMA_VERSION);
    Ok(())
}
