//! Tests for `settings.rs` -- the admin settings routes. No database: the gate and the
//! validation checks run before any query, and the GET response shape is a pure function.

use analytics_web_srv::auth::{AuthToken, ValidatedUser};
use analytics_web_srv::mutation_audit::CONTROL_PLANE_AUDIT_TARGET;
use analytics_web_srv::settings::{SettingsState, StoredSetting, build_entries, settings_router};
use axum::{Extension, Router, body::Body, http::Request, http::StatusCode};
use chrono::{TimeZone, Utc};
use micromegas::settings::SETTINGS;
use micromegas::tracing::event::in_memory_sink::InMemorySink;
use micromegas::tracing::levels::{LevelFilter, set_max_level};
use micromegas::tracing::logs::LogMsgQueueAny;
use micromegas::tracing::test_utils::init_in_memory_tracing;
use micromegas::transit::HeterogeneousQueue;
use serde_json::{Value, json};
use serial_test::serial;
use tower::ServiceExt;

fn user(is_admin: bool) -> ValidatedUser {
    ValidatedUser {
        subject: "u".to_string(),
        email: Some("u@example.com".to_string()),
        issuer: "local".to_string(),
        is_admin,
    }
}

/// A lazy pool that is never queried: every request in this file is rejected before the handler
/// touches the database.
fn lazy_pool() -> sqlx::PgPool {
    sqlx::PgPool::connect_lazy("postgres://localhost/unused")
        .expect("lazy pool creation is infallible")
}

fn app(is_admin: bool) -> Router {
    settings_router("")
        .layer(Extension(SettingsState {
            pool: Some(lazy_pool()),
        }))
        .layer(Extension(AuthToken(String::new())))
        .layer(Extension(user(is_admin)))
}

fn put(uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(uri)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("build request")
}

fn audit_records(sink: &InMemorySink) -> Vec<Value> {
    let state = sink.state.lock().expect("sink lock");
    let mut records = Vec::new();
    for block in &state.log_blocks {
        for event in block.events.iter() {
            if let LogMsgQueueAny::LogStringEvent(evt) = event
                && evt.desc.target == CONTROL_PLANE_AUDIT_TARGET
            {
                records.push(serde_json::from_str(&evt.msg.0).expect("audit json"));
            }
        }
    }
    records
}

#[tokio::test]
#[serial]
async fn non_admin_put_is_403_with_a_denied_record() {
    let guard = init_in_memory_tracing();
    set_max_level(LevelFilter::Trace);

    let response = app(false)
        .oneshot(put("/api/settings/self_service_mint", r#"{"value":true}"#))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    micromegas::tracing::dispatch::flush_log_buffer();
    let records = audit_records(&guard.sink);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["action"], "set_setting");
    assert_eq!(records[0]["outcome"], "denied");
}

#[tokio::test]
#[serial]
async fn unknown_key_is_404() {
    let response = app(true)
        .oneshot(put("/api/settings/no_such_key", r#"{"value":true}"#))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[serial]
async fn invalid_value_is_400_before_any_db_access() {
    let response = app(true)
        .oneshot(put(
            "/api/settings/self_service_mint",
            r#"{"value":"true"}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app(true)
        .oneshot(put(
            "/api/settings/self_service_max_keys_per_caller",
            r#"{"value":-1}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app(true)
        .oneshot(put(
            "/api/settings/retention_by_stream_tag",
            r#"{"value":{"audit":0}}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[test]
fn entries_cover_every_registered_setting_with_stored_values_merged() {
    let updated_at = Utc.with_ymd_and_hms(2026, 10, 21, 9, 14, 0).unwrap();
    let rows = vec![
        StoredSetting {
            key: "self_service_mint".to_string(),
            value: json!(true),
            updated_at,
            updated_by: "admin@example.com".to_string(),
        },
        // Written by a newer replica; not listed.
        StoredSetting {
            key: "unknown_future_key".to_string(),
            value: json!(1),
            updated_at,
            updated_by: "admin@example.com".to_string(),
        },
    ];
    let entries = build_entries(SETTINGS, &rows);
    assert_eq!(entries.len(), SETTINGS.len());

    let json: Value = serde_json::to_value(&entries).expect("serialize");
    let mint = json
        .as_array()
        .expect("array")
        .iter()
        .find(|e| e["key"] == "self_service_mint")
        .expect("mint entry");
    assert_eq!(mint["kind"], "bool");
    assert_eq!(mint["env_var"], "MICROMEGAS_SELF_SERVICE_MINT");
    assert_eq!(mint["default"], false);
    assert_eq!(mint["value"], true);
    assert_eq!(mint["updated_by"], "admin@example.com");
    assert!(mint["updated_at"].is_string());

    let views = json
        .as_array()
        .expect("array")
        .iter()
        .find(|e| e["key"] == "public_view_sets")
        .expect("public_view_sets entry");
    assert_eq!(views["default"], json!([]));

    let retention = json
        .as_array()
        .expect("array")
        .iter()
        .find(|e| e["key"] == "retention_by_stream_tag")
        .expect("retention_by_stream_tag entry");
    assert_eq!(retention["kind"], "tag_horizons");
    assert!(retention["env_var"].is_null());
    assert_eq!(retention["default"], json!({}));
    assert!(views["value"].is_null());
    assert!(views["updated_at"].is_null());
    assert!(views["updated_by"].is_null());
    assert!(
        json.as_array()
            .unwrap()
            .iter()
            .all(|e| e["key"] != "unknown_future_key")
    );
}
