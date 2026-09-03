//! Pins that `FlightSqlServerBuilder::build_and_serve` resolves `IsolationConfig` before it ever
//! touches `LakehouseContext::from_env()` -- a malformed `MICROMEGAS_PUBLIC_VIEW_SETS` must fail
//! fast with its own error, not `LakehouseContext::from_env()`'s "connection string not set" (or
//! a real connection attempt, if one happens to be exported in the shell), with no live Postgres
//! or object store needed to observe it.
//!
//! No `#[serial]`: cargo gives this file its own single-test binary, so there is no other test in
//! this process to race with over process-wide env vars.

use micromegas::servers::flight_sql_server::FlightSqlServer;

const PUBLIC_VIEW_SETS_VAR: &str = "MICROMEGAS_PUBLIC_VIEW_SETS";
const SQL_VAR: &str = "MICROMEGAS_SQL_CONNECTION_STRING";
const OBJECT_STORE_VAR: &str = "MICROMEGAS_OBJECT_STORE_URI";

/// Clears all three env vars on drop.
struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: this file's only test does not run concurrently with any other test in this
        // process (its own single-test binary).
        unsafe {
            std::env::remove_var(PUBLIC_VIEW_SETS_VAR);
            std::env::remove_var(SQL_VAR);
            std::env::remove_var(OBJECT_STORE_VAR);
        }
    }
}

/// Clearing `MICROMEGAS_SQL_CONNECTION_STRING`/`MICROMEGAS_OBJECT_STORE_URI` is what makes
/// reaching this error *without* a lake the assertion that resolution runs ahead of
/// `LakehouseContext::from_env()`: those two are usually exported in a dev shell, and without
/// clearing them the test would still pass after connecting to a real lake, reducing it to a
/// duplicate of the unit test in `flight_sql_server.rs`.
#[tokio::test]
async fn malformed_public_view_sets_fails_before_touching_the_lake() {
    let _guard = EnvGuard;
    // SAFETY: see `EnvGuard`.
    unsafe {
        std::env::set_var(PUBLIC_VIEW_SETS_VAR, r#"["log_stats"]"#);
        std::env::remove_var(SQL_VAR);
        std::env::remove_var(OBJECT_STORE_VAR);
    }

    let err = FlightSqlServer::builder()
        .build_and_serve()
        .await
        .expect_err("a malformed MICROMEGAS_PUBLIC_VIEW_SETS must fail startup");
    let msg = err.to_string();
    assert!(
        msg.contains("comma-separated, not a JSON array"),
        "expected the IsolationConfig parse error, not a lakehouse-connection error, got: {msg}"
    );
}
