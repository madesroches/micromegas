//! Tests for `mutation_audit.rs` -- the control-plane mutation audit record.
//!
//! Three layers, per the module's own design:
//!
//! - **Serialization**: `MutationAuditRecord` constructed directly and round-tripped through
//!   `serde_json`, mirroring `rust/public/tests/query_audit_tests.rs`.
//! - **Outcome classification**: `AuditOutcome::audit_outcome` walked over every variant of the
//!   three error enums that implement it, since a variant misclassified as `"error"` instead of
//!   `"denied"` (or vice versa) is a silent audit hole no HTTP-status assertion would catch.
//! - **Emission, end to end**: routes driven through `tower::ServiceExt::oneshot` with an
//!   in-memory tracing sink capturing the one `control_plane_audit` record each denied/erroring
//!   request emits. Every case here is a gate denial or a `NotConfigured` error, both reachable
//!   with `pool: None`/`pool: Some(lazy_pool())` and no live database -- exactly the seam
//!   `audience_grants_tests.rs`/`groups_tests.rs` already lean on for their own non-`#[ignore]`d
//!   coverage. Tests in this section are `#[serial]`: the tracing dispatch this crate's tests
//!   drive through is process-global.

use analytics_web_srv::audience_grants::{
    AudienceGrantError, AudienceGrantsState, audience_grants_router,
};
use analytics_web_srv::auth::{AuthToken, ValidatedUser};
use analytics_web_srv::groups::{GroupsError, GroupsState, groups_router};
use analytics_web_srv::ingestion_keys::IngestionKeyError;
use analytics_web_srv::mutation_audit::{
    AuditOutcome, CONTROL_PLANE_AUDIT_TARGET, MutationAudit, MutationAuditRecord, action,
};
use axum::{Extension, Router, body::Body, http::Request, http::StatusCode};
use micromegas::auth::types::{AuthContext, AuthType};
use micromegas::tracing::event::in_memory_sink::InMemorySink;
use micromegas::tracing::levels::{LevelFilter, set_max_level};
use micromegas::tracing::logs::LogMsgQueueAny;
use micromegas::tracing::test_utils::init_in_memory_tracing;
use micromegas::transit::HeterogeneousQueue;
use serde_json::Value;
use serial_test::serial;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

fn full_record() -> MutationAuditRecord {
    MutationAuditRecord {
        actor: "alice@example.com".to_string(),
        is_admin: true,
        action: action::CREATE_GRANT,
        outcome: "allowed",
        client_ip: "203.0.113.7".to_string(),
        audience: Some("team-alpha".to_string()),
        axis: Some("read".to_string()),
        selector: Some("group:eng".to_string()),
        group: None,
        member: None,
        created: Some(true),
        reason: None,
    }
}

#[test]
fn full_grant_record_serializes_every_field() {
    let json: Value = serde_json::to_value(full_record()).expect("serialize");
    assert_eq!(json["actor"], "alice@example.com");
    assert_eq!(json["is_admin"], true);
    assert_eq!(json["action"], "create_grant");
    assert_eq!(json["outcome"], "allowed");
    assert_eq!(json["client_ip"], "203.0.113.7");
    assert_eq!(json["audience"], "team-alpha");
    assert_eq!(json["axis"], "read");
    assert_eq!(json["selector"], "group:eng");
    assert_eq!(json["created"], true);
}

#[test]
fn group_record_carries_no_grant_fields() {
    let mut record = full_record();
    record.action = action::CREATE_GROUP;
    record.audience = None;
    record.axis = None;
    record.selector = None;
    record.group = Some("eng".to_string());
    record.created = None;
    let json: Value = serde_json::to_value(record).expect("serialize");
    assert!(json.get("audience").is_none());
    assert!(json.get("axis").is_none());
    assert!(json.get("selector").is_none());
    assert!(json.get("member").is_none());
    assert!(json.get("created").is_none());
    assert_eq!(json["group"], "eng");
}

#[test]
fn gate_denial_record_carries_no_target_fields() {
    let record = MutationAuditRecord {
        actor: "unauthenticated".to_string(),
        is_admin: false,
        action: action::CREATE_GRANT,
        outcome: "denied",
        client_ip: "203.0.113.7".to_string(),
        audience: None,
        axis: None,
        selector: None,
        group: None,
        member: None,
        created: None,
        reason: Some("authentication required".to_string()),
    };
    let json: Value = serde_json::to_value(record).expect("serialize");
    for field in ["audience", "axis", "selector", "group", "member", "created"] {
        assert!(json.get(field).is_none(), "unexpected {field} in {json}");
    }
    assert_eq!(json["reason"], "authentication required");
}

#[test]
fn reason_is_omitted_when_outcome_is_allowed() {
    let record = full_record();
    let json: Value = serde_json::to_value(record).expect("serialize");
    assert!(json.get("reason").is_none());
}

/// A hierarchical `group:` selector can carry `{`/`}`/quotes -- these must round-trip through
/// JSON rather than corrupting the record.
#[test]
fn a_selector_with_json_metacharacters_round_trips() {
    let mut record = full_record();
    record.selector = Some(r#"group:{"nested":true}"#.to_string());
    let json_str = serde_json::to_string(&record).expect("serialize");
    let parsed: Value = serde_json::from_str(&json_str).expect("valid json");
    assert_eq!(parsed["selector"], r#"group:{"nested":true}"#);
}

// ---------------------------------------------------------------------------
// Outcome classification
// ---------------------------------------------------------------------------

#[test]
fn audience_grant_error_outcome_classification() {
    let denied: Vec<AudienceGrantError> = vec![
        AudienceGrantError::Forbidden("nope".to_string()),
        AudienceGrantError::Unauthenticated("nope".to_string()),
        AudienceGrantError::NotFound,
        AudienceGrantError::GroupNotFound("nope".to_string()),
        AudienceGrantError::BadRequest("nope".to_string()),
    ];
    for err in denied {
        assert_eq!(err.audit_outcome().0, "denied", "{err:?}");
    }

    let errors: Vec<AudienceGrantError> = vec![
        AudienceGrantError::Database(sqlx::Error::RowNotFound),
        AudienceGrantError::NotConfigured,
        AudienceGrantError::Internal("boom".to_string()),
    ];
    for err in errors {
        assert_eq!(err.audit_outcome().0, "error", "{err:?}");
    }

    let (_, reason) = AudienceGrantError::Database(sqlx::Error::RowNotFound).audit_outcome();
    assert_eq!(reason, Some("internal database error".to_string()));
}

#[test]
fn groups_error_outcome_classification() {
    let denied: Vec<GroupsError> = vec![
        GroupsError::BadRequest("nope".to_string()),
        GroupsError::NotFound,
        GroupsError::NestedGroupNotFound("nope".to_string()),
        GroupsError::Conflict("nope".to_string()),
    ];
    for err in denied {
        assert_eq!(err.audit_outcome().0, "denied");
    }

    let errors: Vec<GroupsError> = vec![
        GroupsError::Database(sqlx::Error::RowNotFound),
        GroupsError::NotConfigured,
        GroupsError::Internal("boom".to_string()),
    ];
    for err in errors {
        assert_eq!(err.audit_outcome().0, "error");
    }

    let (_, reason) = GroupsError::Database(sqlx::Error::RowNotFound).audit_outcome();
    assert_eq!(reason, Some("internal database error".to_string()));
}

/// `Conflict` on the claim path (advisory-lock contention) classifies as `"error"`, not
/// `"denied"` -- `try_claim_and_mint`'s own doc comment says the caller should retry, which is
/// not a denial.
#[test]
fn ingestion_key_error_outcome_classification() {
    let denied: Vec<IngestionKeyError> = vec![
        IngestionKeyError::Forbidden("nope".to_string()),
        IngestionKeyError::Unauthenticated("nope".to_string()),
        IngestionKeyError::NotFound,
        IngestionKeyError::BadRequest("nope".to_string()),
    ];
    for err in denied {
        assert_eq!(err.audit_outcome().0, "denied");
    }

    let errors: Vec<IngestionKeyError> = vec![
        IngestionKeyError::Conflict("retry".to_string()),
        IngestionKeyError::Database(sqlx::Error::RowNotFound),
        IngestionKeyError::NotConfigured,
        IngestionKeyError::Unavailable("down".to_string()),
    ];
    for err in errors {
        assert_eq!(err.audit_outcome().0, "error", "{err:?}");
    }

    let (_, reason) = IngestionKeyError::Database(sqlx::Error::RowNotFound).audit_outcome();
    assert_eq!(reason, Some("internal database error".to_string()));
}

// ---------------------------------------------------------------------------
// Emission, end to end -- in-process, no live DB.
// ---------------------------------------------------------------------------

/// `init_in_memory_tracing()` wires up the dispatch but doesn't raise the process-global max log
/// level -- without this, `info!`'s own level guard silently drops every call, since the default
/// global level is `LevelFilter::Off` (`auth_observability_tests.rs` documents the same gotcha).
fn enable_info_logging() {
    set_max_level(LevelFilter::Trace);
}

/// Every `control_plane_audit` record collected, parsed as JSON.
fn collect_audit_records(sink: &InMemorySink) -> Vec<Value> {
    let state = sink.state.lock().expect("sink lock");
    let mut records = Vec::new();
    for block in &state.log_blocks {
        for event in block.events.iter() {
            if let LogMsgQueueAny::LogStringEvent(evt) = event
                && evt.desc.target == CONTROL_PLANE_AUDIT_TARGET
            {
                records.push(serde_json::from_str(&evt.msg.0).expect("valid json audit record"));
            }
        }
    }
    records
}

fn lazy_pool() -> sqlx::PgPool {
    sqlx::PgPool::connect_lazy("postgres://localhost/unused")
        .expect("lazy pool creation is infallible")
}

fn admin_user() -> ValidatedUser {
    ValidatedUser {
        subject: "admin".to_string(),
        email: Some("admin@example.com".to_string()),
        issuer: "local".to_string(),
        is_admin: true,
    }
}

fn non_admin_user() -> ValidatedUser {
    ValidatedUser {
        subject: "reader".to_string(),
        email: Some("reader@example.com".to_string()),
        issuer: "local".to_string(),
        is_admin: false,
    }
}

/// Mirrors `audience_grants_tests.rs::auth_context_for` -- duplicated per this crate's existing
/// convention of not sharing helpers across `tests/*.rs` files (each is a separate crate).
/// `AuthContext::is_admin()` derives from group membership, not a plain field (unlike
/// `ValidatedUser::is_admin`), so an admin fixture must carry `ADMINS_GROUP`.
fn auth_context_for(user: &ValidatedUser) -> AuthContext {
    let mut memberships = Vec::new();
    if user.is_admin {
        memberships.push(micromegas::auth::groups::ADMINS_GROUP.to_string());
    }
    AuthContext {
        subject: user.subject.clone(),
        email: user.email.clone(),
        issuer: user.issuer.clone(),
        audience: None,
        expires_at: None,
        auth_type: AuthType::Oidc,
        allow_delegation: false,
        bound_audience: None,
        read_audiences: vec![],
        memberships: memberships.into(),
    }
}

fn grant_router_with_user(state: AudienceGrantsState, user: ValidatedUser) -> Router {
    let auth_context = auth_context_for(&user);
    audience_grants_router("")
        .layer(Extension(state))
        .layer(Extension(AuthToken(String::new())))
        .layer(Extension(user))
        .layer(Extension(auth_context))
}

/// No `AuthContext`/`ValidatedUser` extension at all -- the fail-closed "routing misconfigured"
/// case `AuthenticatedUser`'s own doc comment describes.
fn grant_router_with_no_identity(state: AudienceGrantsState) -> Router {
    audience_grants_router("").layer(Extension(state))
}

fn groups_router_with_user(state: GroupsState, user: ValidatedUser) -> Router {
    groups_router("")
        .layer(Extension(state))
        .layer(Extension(AuthToken(String::new())))
        .layer(Extension(user))
}

fn post_request(uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("build request")
}

fn delete_request(uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .expect("build request")
}

/// One of the four group-mutation requests, keyed by `(method, uri, body)` -- shared by the two
/// table-driven tests below.
fn group_mutation_request(method: &str, uri: &str, body: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header(axum::http::header::CONTENT_TYPE, "application/json");
    }
    builder
        .body(match body {
            Some(b) => Body::from(b.to_string()),
            None => Body::empty(),
        })
        .expect("build request")
}

/// The four group-mutation `(method, uri, body, expected_action)` cases, shared by the two
/// table-driven tests below.
fn group_mutation_cases() -> [(
    &'static str,
    &'static str,
    Option<&'static str>,
    &'static str,
); 4] {
    [
        (
            "POST",
            "/api/groups",
            Some(r#"{"name":"eng"}"#),
            "create_group",
        ),
        ("DELETE", "/api/groups/eng", None, "delete_group"),
        (
            "POST",
            "/api/groups/eng/members",
            Some(r#"{"member":"user:x@example.com"}"#),
            "add_member",
        ),
        (
            "DELETE",
            "/api/groups/eng/members?member=user:x@example.com",
            None,
            "remove_member",
        ),
    ]
}

#[tokio::test]
#[serial]
async fn create_grant_gate_denial_emits_a_denied_record_with_no_target_fields() {
    let guard = init_in_memory_tracing();
    enable_info_logging();

    let app = grant_router_with_user(
        AudienceGrantsState {
            pool: Some(lazy_pool()),
            self_service_mint_enabled: false,
            max_grants_per_caller: 50,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(post_request(
            "/api/audience-grants",
            r#"{"audience": "team-alpha", "axis": "read", "selector": "user:reader@example.com"}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    micromegas::tracing::dispatch::flush_log_buffer();
    let records = collect_audit_records(&guard.sink);
    assert_eq!(records.len(), 1, "{records:?}");
    let record = &records[0];
    assert_eq!(record["action"], "create_grant");
    assert_eq!(record["outcome"], "denied");
    assert_eq!(record["actor"], "reader@example.com");
    for field in ["audience", "axis", "selector"] {
        assert!(
            record.get(field).is_none(),
            "unexpected {field} in {record}"
        );
    }
}

#[tokio::test]
#[serial]
async fn delete_grant_gate_denial_emits_the_delete_grant_action() {
    let guard = init_in_memory_tracing();
    enable_info_logging();

    let app = grant_router_with_user(
        AudienceGrantsState {
            pool: Some(lazy_pool()),
            self_service_mint_enabled: false,
            max_grants_per_caller: 50,
        },
        non_admin_user(),
    );
    let response = app
        .oneshot(delete_request(
            "/api/audience-grants?audience=team-alpha&axis=read&selector=%2A",
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    micromegas::tracing::dispatch::flush_log_buffer();
    let records = collect_audit_records(&guard.sink);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["action"], "delete_grant");
    assert_eq!(records[0]["outcome"], "denied");
}

#[tokio::test]
#[serial]
async fn grant_gate_with_no_identity_extension_records_unauthenticated_actor() {
    let guard = init_in_memory_tracing();
    enable_info_logging();

    let app = grant_router_with_no_identity(AudienceGrantsState {
        pool: Some(lazy_pool()),
        self_service_mint_enabled: true,
        max_grants_per_caller: 50,
    });
    let response = app
        .oneshot(post_request(
            "/api/audience-grants",
            r#"{"audience": "team-alpha", "axis": "read", "selector": "*"}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    micromegas::tracing::dispatch::flush_log_buffer();
    let records = collect_audit_records(&guard.sink);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["actor"], "unauthenticated");
    assert_eq!(records[0]["outcome"], "denied");
}

/// Pins the record to `get_client_ip`'s rightmost-`X-Forwarded-For`-entry rule, not the socket
/// address.
#[tokio::test]
#[serial]
async fn client_ip_is_the_rightmost_x_forwarded_for_entry() {
    let guard = init_in_memory_tracing();
    enable_info_logging();

    let app = groups_router_with_user(
        GroupsState {
            pool: Some(lazy_pool()),
        },
        non_admin_user(),
    );
    let request = Request::builder()
        .method("POST")
        .uri("/api/groups")
        .header("X-Forwarded-For", "203.0.113.7, 198.51.100.1")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"name": "eng"}"#.to_string()))
        .expect("build request");
    let response = app.oneshot(request).await.expect("call service");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    micromegas::tracing::dispatch::flush_log_buffer();
    let records = collect_audit_records(&guard.sink);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["client_ip"], "198.51.100.1");
}

/// An admin request with `pool: None` reaches `create_grant_inner`'s `require_pool` and emits an
/// `"error"` record -- the only automated coverage of a handler-wrapper (not gate) emission for
/// the grant routes.
#[tokio::test]
#[serial]
async fn create_grant_pool_unconfigured_emits_an_error_record() {
    let guard = init_in_memory_tracing();
    enable_info_logging();

    let app = grant_router_with_user(
        AudienceGrantsState {
            pool: None,
            self_service_mint_enabled: false,
            max_grants_per_caller: 50,
        },
        admin_user(),
    );
    let response = app
        .oneshot(post_request(
            "/api/audience-grants",
            r#"{"audience": "team-alpha", "axis": "read", "selector": "*"}"#,
        ))
        .await
        .expect("call service");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    micromegas::tracing::dispatch::flush_log_buffer();
    let records = collect_audit_records(&guard.sink);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["outcome"], "error");
    assert_eq!(records[0]["actor"], "admin@example.com");
    assert!(
        records[0]["reason"]
            .as_str()
            .expect("reason")
            .starts_with("audience grant store not configured")
    );
}

/// One record per non-admin denial, on each of the four group mutation routes -- covers the
/// `GroupAdminGate` action-derivation table.
#[tokio::test]
#[serial]
async fn group_mutation_routes_denied_for_non_admin_with_the_right_action() {
    for (method, uri, body, expected_action) in group_mutation_cases() {
        let guard = init_in_memory_tracing();
        enable_info_logging();

        let app = groups_router_with_user(
            GroupsState {
                pool: Some(lazy_pool()),
            },
            non_admin_user(),
        );
        let request = group_mutation_request(method, uri, body);
        let response = app.oneshot(request).await.expect("call service");
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {uri}");

        micromegas::tracing::dispatch::flush_log_buffer();
        let records = collect_audit_records(&guard.sink);
        assert_eq!(records.len(), 1, "{method} {uri}: {records:?}");
        assert_eq!(records[0]["action"], expected_action, "{method} {uri}");
        assert_eq!(records[0]["outcome"], "denied");
        assert_eq!(records[0]["actor"], "reader@example.com");
    }
}

/// An admin request on each of the four group mutation routes with `pool: None` reaches the
/// handler (`require_pool` is its first statement) and emits one `"error"` record with the
/// group/member target fields -- the only automated coverage of these four wrappers, since every
/// case above is a gate denial that never reaches them.
#[tokio::test]
#[serial]
async fn group_mutation_routes_pool_unconfigured_emit_error_records_with_targets() {
    for (method, uri, body, expected_action) in group_mutation_cases() {
        let guard = init_in_memory_tracing();
        enable_info_logging();

        let app = groups_router_with_user(GroupsState { pool: None }, admin_user());
        let request = group_mutation_request(method, uri, body);
        let response = app.oneshot(request).await.expect("call service");
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{method} {uri}"
        );

        micromegas::tracing::dispatch::flush_log_buffer();
        let records = collect_audit_records(&guard.sink);
        assert_eq!(records.len(), 1, "{method} {uri}: {records:?}");
        let record = &records[0];
        assert_eq!(record["action"], expected_action, "{method} {uri}");
        assert_eq!(record["outcome"], "error");
        assert_eq!(record["actor"], "admin@example.com");
        assert_eq!(record["group"], "eng", "{method} {uri}");
        if uri.contains("members") {
            assert_eq!(record["member"], "user:x@example.com", "{method} {uri}");
        }
    }
}

/// A field over the truncation budget (255 bytes, `MAX_SELECTOR_BYTES`'s precedent) is cut down
/// and marked with a trailing `"..."`, so a truncated value in the log is never mistaken for a
/// complete one -- this is what stands in for the validation the audit is built before.
#[tokio::test]
#[serial]
async fn a_long_audience_value_is_truncated_with_a_marker() {
    let guard = init_in_memory_tracing();
    enable_info_logging();

    let long_audience = "a".repeat(1000);
    let audit = MutationAudit::new(
        action::CREATE_GRANT,
        "alice@example.com".to_string(),
        false,
        "203.0.113.7".to_string(),
    )
    .audience(&long_audience);
    audit.emit_gate_outcome("denied", "too long");

    micromegas::tracing::dispatch::flush_log_buffer();
    let records = collect_audit_records(&guard.sink);
    assert_eq!(records.len(), 1, "{records:?}");
    let audience = records[0]["audience"].as_str().expect("audience");
    assert_eq!(audience, format!("{}...", "a".repeat(255)));
}

/// A multi-byte character straddling the truncation budget must not be split mid-codepoint: `"é"`
/// (2 bytes) placed so it spans bytes 254-255 forces the truncation point back to the character
/// boundary at 254 rather than slicing through it.
#[tokio::test]
#[serial]
async fn a_value_that_would_split_a_codepoint_truncates_before_the_boundary() {
    let guard = init_in_memory_tracing();
    enable_info_logging();

    let value = format!("{}{}{}", "a".repeat(254), "é", "b".repeat(10));
    let audit = MutationAudit::new(
        action::CREATE_GROUP,
        "alice@example.com".to_string(),
        false,
        "203.0.113.7".to_string(),
    )
    .group(&value);
    audit.emit_gate_outcome("denied", "too long");

    micromegas::tracing::dispatch::flush_log_buffer();
    let records = collect_audit_records(&guard.sink);
    assert_eq!(records.len(), 1, "{records:?}");
    let group = records[0]["group"].as_str().expect("group");
    assert_eq!(group, format!("{}...", "a".repeat(254)));
}

/// The `reason` field is bounded the same way, whether it comes from a gate denial's caller-chosen
/// string (here) or `AuditOutcome::audit_outcome`'s formatted message -- both funnel through the
/// same truncation so an oversized value can't land in the log via either path.
#[tokio::test]
#[serial]
async fn a_long_gate_denial_reason_is_truncated_with_a_marker() {
    let guard = init_in_memory_tracing();
    enable_info_logging();

    let long_reason = "x".repeat(1000);
    let audit = MutationAudit::new(
        action::CREATE_GRANT,
        "alice@example.com".to_string(),
        false,
        "203.0.113.7".to_string(),
    );
    audit.emit_gate_outcome("denied", long_reason);

    micromegas::tracing::dispatch::flush_log_buffer();
    let records = collect_audit_records(&guard.sink);
    assert_eq!(records.len(), 1, "{records:?}");
    let reason = records[0]["reason"].as_str().expect("reason");
    assert_eq!(reason, format!("{}...", "x".repeat(255)));
}

/// The only automated coverage of the `Ok` -> `"allowed"` classification: `emit`'s `Err` arm is
/// covered by every case above, walking `E`'s variants; this is the one test of its `Ok` arm.
#[tokio::test]
#[serial]
async fn emit_ok_result_records_allowed_with_no_reason() {
    let guard = init_in_memory_tracing();
    enable_info_logging();

    let audit = MutationAudit::new(
        action::CREATE_GRANT,
        "alice@example.com".to_string(),
        true,
        "203.0.113.7".to_string(),
    )
    .created(true);
    audit.emit(&Ok::<(), AudienceGrantError>(()));

    micromegas::tracing::dispatch::flush_log_buffer();
    let records = collect_audit_records(&guard.sink);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["outcome"], "allowed");
    assert!(records[0].get("reason").is_none());
    assert_eq!(records[0]["created"], true);
}
