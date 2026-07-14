// Integration test for the pg_stats self-observability collector
// (`micromegas::servers::pg_stats`). Requires `MICROMEGAS_SQL_CONNECTION_STRING`
// to point at a live Postgres instance (e.g. the local test stack's metadata
// DB) — marked `#[ignore]` since it needs a live dependency.
//
// Safety note: `collect_pg_stats` issues only `SELECT`s against the
// `pg_stat_*` catalog views. It never calls `pg_stat_reset*` /
// `pg_stat_reset_shared*`, so running this test against a live DB cannot zero
// out its statistics — verified by grepping this module for `pg_stat_reset`.
use micromegas::servers::pg_stats::collect_pg_stats;
use micromegas::sqlx::PgPool;

#[ignore]
#[tokio::test]
async fn collect_pg_stats_against_live_db() {
    let conn_str = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .expect("MICROMEGAS_SQL_CONNECTION_STRING must point at a live Postgres instance");
    let pool = PgPool::connect(&conn_str)
        .await
        .expect("connecting to metadata Postgres");
    collect_pg_stats(&pool)
        .await
        .expect("collect_pg_stats should succeed against a reachable Postgres");
}
