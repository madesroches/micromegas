//! Coverage for `WebServerConfig::from_cli_and_env`: the base-path validation
//! rule and the required-env-var errors. Tests are serialized with
//! `#[serial]` since they all mutate process-wide env vars.

use analytics_web_srv::web_server::{WebCliArgs, WebServerConfig};
use serial_test::serial;

const CORS_VAR: &str = "MICROMEGAS_WEB_CORS_ORIGIN";
const BASE_PATH_VAR: &str = "MICROMEGAS_BASE_PATH";
const APP_DB_VAR: &str = "MICROMEGAS_APP_SQL_CONNECTION_STRING";
const MAPS_URI_VAR: &str = "MICROMEGAS_MAPS_OBJECT_STORE_URI";
const MAPS_MAX_UPLOAD_VAR: &str = "MICROMEGAS_MAPS_MAX_UPLOAD_BYTES";

/// Clears every var this config touches on drop, so a failing assertion in
/// one test can't leak state into the next.
struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: tests are serialized with `#[serial]`.
        unsafe {
            std::env::remove_var(CORS_VAR);
            std::env::remove_var(BASE_PATH_VAR);
            std::env::remove_var(APP_DB_VAR);
            std::env::remove_var(MAPS_URI_VAR);
            std::env::remove_var(MAPS_MAX_UPLOAD_VAR);
        }
    }
}

fn set_required_vars(base_path: &str) {
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(CORS_VAR, "http://localhost:3000");
        std::env::set_var(BASE_PATH_VAR, base_path);
        std::env::set_var(APP_DB_VAR, "postgres://localhost/app");
    }
}

fn cli_args() -> WebCliArgs {
    WebCliArgs {
        port: 3000,
        frontend_dir: "dist".to_string(),
        disable_auth: true,
        admin_var_name: "MICROMEGAS_ADMINS".to_string(),
    }
}

#[test]
#[serial]
fn base_path_empty_is_accepted_as_is() {
    let _guard = EnvGuard;
    set_required_vars("");

    let config = WebServerConfig::from_cli_and_env(cli_args()).expect("valid config");
    assert_eq!(config.base_path, "");
}

#[test]
#[serial]
fn base_path_root_slash_is_trimmed_to_empty() {
    let _guard = EnvGuard;
    set_required_vars("/");

    let config = WebServerConfig::from_cli_and_env(cli_args()).expect("valid config");
    assert_eq!(config.base_path, "");
}

#[test]
#[serial]
fn base_path_with_trailing_slash_is_trimmed() {
    let _guard = EnvGuard;
    set_required_vars("/micromegas/");

    let config = WebServerConfig::from_cli_and_env(cli_args()).expect("valid config");
    assert_eq!(config.base_path, "/micromegas");
}

#[test]
#[serial]
fn base_path_non_root_is_accepted() {
    let _guard = EnvGuard;
    set_required_vars("/micromegas");

    let config = WebServerConfig::from_cli_and_env(cli_args()).expect("valid config");
    assert_eq!(config.base_path, "/micromegas");
}

#[test]
#[serial]
fn base_path_missing_leading_slash_is_rejected() {
    let _guard = EnvGuard;
    set_required_vars("micromegas");

    let err = WebServerConfig::from_cli_and_env(cli_args())
        .err()
        .expect("base path without a leading slash must be rejected");
    assert!(err.to_string().contains("must start with '/'"));
}

#[test]
#[serial]
fn missing_cors_origin_is_an_error() {
    let _guard = EnvGuard;
    set_required_vars("/");
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(CORS_VAR);
    }

    assert!(WebServerConfig::from_cli_and_env(cli_args()).is_err());
}

#[test]
#[serial]
fn missing_base_path_is_an_error() {
    let _guard = EnvGuard;
    set_required_vars("/");
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(BASE_PATH_VAR);
    }

    assert!(WebServerConfig::from_cli_and_env(cli_args()).is_err());
}

#[test]
#[serial]
fn missing_app_db_string_is_an_error() {
    let _guard = EnvGuard;
    set_required_vars("/");
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(APP_DB_VAR);
    }

    assert!(WebServerConfig::from_cli_and_env(cli_args()).is_err());
}

#[test]
#[serial]
fn maps_vars_are_optional() {
    let _guard = EnvGuard;
    set_required_vars("/");

    let config = WebServerConfig::from_cli_and_env(cli_args()).expect("valid config");
    assert_eq!(config.maps_uri, None);
    assert_eq!(config.max_upload_bytes, None);
}
