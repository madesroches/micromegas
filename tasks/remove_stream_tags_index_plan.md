# Remove the `stream_tags` Index (Schema v13) Plan

## Overview
Per-stream-tag payload retention added data-lake schema v13, whose only step is building a GIN
index `stream_tags` on `streams(tags)`. The index only serves the hourly retention query, which
scans a small table. The migration runs before the ingestion server starts listening, so the
build costs startup downtime (about 5s on a dev database; more on a large `streams` table). v13
has not shipped in a release, so remove it and return the latest schema version to 12.

## Current State
- `rust/ingestion/src/sql_migration.rs:8`: `LATEST_DATA_LAKE_SCHEMA_VERSION = 13`.
- `sql_migration.rs:~648`: the `12 == current_version` step runs
  `CREATE INDEX CONCURRENTLY IF NOT EXISTS stream_tags ...`, validates it with
  `check_index_is_valid`, then calls `upgrade_data_lake_schema_v13` (a bare version bump).
- `sql_migration.rs:~66-70`: `warn_if_data_lake_schema_stale` warns when `version < 13` that the
  index is missing.
- `rust/ingestion/src/remote_data_lake.rs:~23-42` (`migrate_db`): takes an advisory lock, runs
  `execute_migration`, then `assert_eq!(current_version, LATEST_DATA_LAKE_SCHEMA_VERSION)`.
  `web_ingestion_service.rs:282` awaits it before serving.
- The index serves `rust/analytics/src/delete.rs:~98-130` (`s.tags && $3`, `NOT (s.tags && $4)`).
  User-facing tag searches go through DataFusion, not this Postgres table.
- References: `CHANGELOG.md:16` ("Data-lake schema v13 adds a GIN index ..."),
  `tasks/completed/1695_payload_retention_per_tag_plan.md`.
- `check_index_is_valid` stays in use by the v3 step.

## Design
Delete the v13 migration step and function and set the latest version back to 12. Without the
index the retention queries are a sequential scan of `streams` once per pass, which is
acceptable at the size of that table.

### Databases already at v13 (dev only)
`migrate_db` on a v13 database with a v12 binary finds `13 != 12`, `execute_migration` matches no
step, and the final `assert_eq!` panics. That is the right outcome for a real downgrade, so no
downgrade handling is added. The affected dev database is rolled back by hand, before deploying
the new build:

```
DROP INDEX IF EXISTS stream_tags;
UPDATE migration SET version=12;
```

`UPDATE migration SET version=12` is safe because v13 changed nothing else. Tracked in a
separate ticket. No other environment has run v13.

### Interaction with the partition plan
`tasks/backlog/1245_partition_blocks_by_insert_time/plan.md` makes default retention a partition
`DROP`. Per-tag overrides may still need a row-level path, because a partition covers a time
window across all streams. If that path comes back, whether it needs a tag index is decided
there, with a measured cost. No index is kept in reserve here.

## Implementation Steps
1. `rust/ingestion/src/sql_migration.rs`: set `LATEST_DATA_LAKE_SCHEMA_VERSION` to 12; remove the
   `12 == current_version` step and `upgrade_data_lake_schema_v13`; remove the `version < 13`
   warning in `warn_if_data_lake_schema_stale`.
2. `CHANGELOG.md`: delete the "Data-lake schema v13 adds a GIN index on `streams.tags` (built
   concurrently)" sentence from the per-tag retention entry.
3. `tasks/completed/1695_payload_retention_per_tag_plan.md`: add a short note at the index
   sections that the index was removed before release, rather than rewriting history.
4. Add a note to `tasks/backlog/1245_partition_blocks_by_insert_time/plan.md` that per-tag
   overrides may still need a row-level path and that no tag index exists.

## Files to Modify
- `rust/ingestion/src/sql_migration.rs`
- `CHANGELOG.md`
- `tasks/completed/1695_payload_retention_per_tag_plan.md`
- `tasks/backlog/1245_partition_blocks_by_insert_time/plan.md`

## Trade-offs
- **Keep the index, build it later or off the startup path:** more machinery for an index that
  only speeds up an hourly scan of a small table.
- **Keep v13 as a no-op bump:** leaves a dead version in the chain for no benefit, since v13 has
  not shipped.
- **Handle v13-ahead databases in `migrate_db` (tolerate newer versions):** loosens a check that
  catches real downgrades, to spare one manual SQL statement on one dev database.

## Decisions
- v13 is removed rather than neutralised, because it has not shipped in a release.
- No automatic downgrade path; the dev database is fixed by hand.

## Documentation
No `mkdocs/` page mentions the index or v13. `CHANGELOG.md` is the only user-facing change.

## Testing Strategy
- `rust/ingestion/tests/sql_migration_test.rs` already asserts a fresh database migrates to
  `LATEST_DATA_LAKE_SCHEMA_VERSION`; with the constant at 12 it covers the new chain with no
  change. Confirm no test references v13 or `stream_tags`.
- `cargo test -p micromegas-ingestion` and `cargo clippy --workspace -- -D warnings`.

## Manual Verification
1. On a fresh local database, start the services (`python3 local_test_env/ai_scripts/start_services.py`)
   and confirm `/tmp/ingestion.log` reaches v12 and that `\d streams` lists no `stream_tags`
   index. Not automated because the migration test already pins the version, and an absent index
   is obvious in `\d`.
2. On the dev database, apply the two rollback statements above, deploy, and confirm the service
   starts.

## Open Questions
None.
