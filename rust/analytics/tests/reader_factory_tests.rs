use micromegas_analytics::lakehouse::reader_factory::read_disable_metadata_psql_cache;
use serial_test::serial;

const ENV_VAR: &str = "MICROMEGAS_DISABLE_METADATA_PSQL_CACHE";

/// Sets/unsets `ENV_VAR` and reads back `read_disable_metadata_psql_cache()`.
/// Runs `#[serial]` since it mutates process-wide environment state.
fn with_env_var(value: Option<&str>) -> bool {
    match value {
        Some(v) => unsafe { std::env::set_var(ENV_VAR, v) },
        None => unsafe { std::env::remove_var(ENV_VAR) },
    }
    let result = read_disable_metadata_psql_cache();
    unsafe { std::env::remove_var(ENV_VAR) };
    result
}

#[test]
#[serial]
fn test_disable_metadata_psql_cache_unset_defaults_to_false() {
    assert!(!with_env_var(None));
}

#[test]
#[serial]
fn test_disable_metadata_psql_cache_truthy_values() {
    for value in ["1", "true", "TRUE", "True", "on", "ON", "yes", "YES"] {
        assert!(
            with_env_var(Some(value)),
            "expected '{value}' to enable the bypass"
        );
    }
}

#[test]
#[serial]
fn test_disable_metadata_psql_cache_falsy_values() {
    for value in ["0", "false", "FALSE", "no", "garbage", ""] {
        assert!(
            !with_env_var(Some(value)),
            "expected '{value}' to keep the postgres path"
        );
    }
}
