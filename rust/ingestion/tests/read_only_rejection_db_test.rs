//! DB-backed regression test pinning the read-only-phase bug this crate's `WritablePolicy`
//! guards against: a real Postgres connection with `default_transaction_read_only=on` must never
//! be handed out by a `read_write_pool_options()` pool, not even once. Requires a live
//! `MICROMEGAS_SQL_CONNECTION_STRING` (see `rust/ingestion/tests/insert_block_dedup_db_test.rs`
//! for the same harness pattern); does not run under a plain `cargo test`.

use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use micromegas_ingestion::data_lake_connection::read_write_pool_options;
use sqlx::postgres::PgConnectOptions;

#[ignore]
#[tokio::test]
async fn new_read_only_connection_is_rejected() -> Result<()> {
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let connect_options = PgConnectOptions::from_str(&connection_string)
        .with_context(|| "parsing MICROMEGAS_SQL_CONNECTION_STRING")?
        .options([("default_transaction_read_only", "on")]);

    let pool = read_write_pool_options()
        .acquire_timeout(Duration::from_millis(500))
        .connect_lazy_with(connect_options);

    let result = pool.acquire().await;
    match result {
        Err(sqlx::Error::PoolTimedOut) => Ok(()),
        Err(other) => Err(anyhow::anyhow!(
            "expected sqlx::Error::PoolTimedOut, got: {other}"
        )),
        Ok(_) => Err(anyhow::anyhow!(
            "a read-only connection must never be handed out by a read-write pool"
        )),
    }
}
