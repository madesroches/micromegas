use micromegas_ingestion::data_lake_config::DataLakeConfig;
use serial_test::serial;

const SQL_VAR: &str = "MICROMEGAS_SQL_CONNECTION_STRING";
const OBJECT_STORE_VAR: &str = "MICROMEGAS_OBJECT_STORE_URI";

/// Clears both env vars on drop so a failing assertion in one test can't leak
/// state into the next (tests are serialized via `#[serial]` since they all
/// mutate process-wide env vars).
struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: tests are serialized with `#[serial]`, so no other thread
        // is reading/writing these vars concurrently.
        unsafe {
            std::env::remove_var(SQL_VAR);
            std::env::remove_var(OBJECT_STORE_VAR);
        }
    }
}

#[test]
#[serial]
fn from_env_both_present() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(SQL_VAR, "postgres://localhost/test");
        std::env::set_var(OBJECT_STORE_VAR, "file:///tmp/lake");
    }

    let cfg = DataLakeConfig::from_env().expect("both vars set");
    assert_eq!(cfg.sql_connection_string, "postgres://localhost/test");
    assert_eq!(cfg.object_store_uri, "file:///tmp/lake");
}

#[test]
#[serial]
fn from_env_missing_sql_connection_string() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(SQL_VAR);
        std::env::set_var(OBJECT_STORE_VAR, "file:///tmp/lake");
    }

    assert!(DataLakeConfig::from_env().is_err());
}

#[test]
#[serial]
fn from_env_missing_object_store_uri() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(SQL_VAR, "postgres://localhost/test");
        std::env::remove_var(OBJECT_STORE_VAR);
    }

    assert!(DataLakeConfig::from_env().is_err());
}
