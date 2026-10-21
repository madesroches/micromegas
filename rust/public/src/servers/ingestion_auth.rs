//! Authentication layer for the ingestion routes.
//!
//! Wraps [`micromegas_auth::axum::authenticate`] so a rejected `insert_process` request, the
//! first call any client makes, can have the process identity it claims logged next to the
//! caller's network details. The 401 response is identical to the one `auth_middleware` returns.

use axum::body::to_bytes;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use micromegas_auth::axum::{AuthError, authenticate};
use micromegas_auth::client_ip::resolve_client_ip;
use micromegas_auth::log_sanitize::sanitize_for_log;
use micromegas_auth::types::AuthProvider;
use micromegas_tracing::prelude::*;
use moka::sync::Cache;
use serde::Deserialize;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

pub(crate) const INSERT_PROCESS_PATH: &str = "/ingestion/insert_process";

/// A real `ProcessInfo` is a few KiB.
const MAX_INSPECTED_BODY_BYTES: usize = 64 * 1024;
const BODY_READ_TIMEOUT: Duration = Duration::from_secs(2);
const CBOR_RECURSION_LIMIT: usize = 16;
const MAX_CLAIM_CHARS: usize = 128;

/// Caller-claimed process identity. Separate from `ProcessInfo` because its strict UUID and
/// timestamp deserializers would reject a malformed id outright and lose every other field.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ClaimedProcessInfo {
    process_id: Option<String>,
    exe: Option<String>,
    username: Option<String>,
    computer: Option<String>,
    parent_process_id: Option<String>,
    start_time: Option<String>,
    properties: HashMap<String, String>,
}

/// Build-identity properties worth logging. `command-line` and `exe_args` are left out because
/// they can carry secrets passed on the command line.
const LOGGED_PROPERTY_KEYS: [&str; 7] = [
    "platform-name",
    "build-version",
    "engine-version",
    "build-config",
    "build-target",
    "branch-name",
    "commit",
];

/// Reads and logs the claims of rejected `insert_process` requests, at most once per client IP
/// and process id per minute.
pub struct RejectedProcessLogger {
    recent: Cache<(Option<IpAddr>, Option<String>), ()>,
}

impl Default for RejectedProcessLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl RejectedProcessLogger {
    pub fn new() -> Self {
        Self {
            recent: Cache::builder()
                .max_capacity(10_000)
                .time_to_live(Duration::from_secs(60))
                .build(),
        }
    }

    fn is_fresh(&self, client_ip: Option<IpAddr>, process_id: Option<String>) -> bool {
        self.recent
            .entry((client_ip, process_id))
            .or_insert(())
            .is_fresh()
    }

    fn log_not_inspected(
        &self,
        client_ip: Option<IpAddr>,
        reason: &str,
        content_length: Option<&str>,
    ) {
        if !self.is_fresh(client_ip, None) {
            return;
        }
        let ip = ip_or_empty(client_ip);
        let content_length = sanitize_for_log(content_length.unwrap_or_default(), 32);
        warn!(
            client_ip = %ip,
            reason = %reason,
            content_length = %content_length,
            "[auth_failure] insert_process body not inspected"
        );
    }

    /// Consumes the rejected request and logs what its body claims about the caller.
    pub async fn log_rejected(&self, req: Request) {
        let client_ip = resolve_client_ip(req.headers(), req.extensions());
        let content_length = req
            .headers()
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let declared = content_length
            .as_deref()
            .and_then(|v| v.parse::<usize>().ok());
        let Some(declared) = declared else {
            self.log_not_inspected(
                client_ip,
                "missing_content_length",
                content_length.as_deref(),
            );
            return;
        };
        if declared > MAX_INSPECTED_BODY_BYTES {
            self.log_not_inspected(client_ip, "oversize", content_length.as_deref());
            return;
        }
        let body = req.into_body();
        let bytes =
            match tokio::time::timeout(BODY_READ_TIMEOUT, to_bytes(body, MAX_INSPECTED_BODY_BYTES))
                .await
            {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(_)) => {
                    self.log_not_inspected(client_ip, "read_error", content_length.as_deref());
                    return;
                }
                Err(_) => {
                    self.log_not_inspected(client_ip, "read_timeout", content_length.as_deref());
                    return;
                }
            };
        let claimed: ClaimedProcessInfo = match ciborium::de::from_reader_with_recursion_limit(
            &bytes[..],
            CBOR_RECURSION_LIMIT,
        ) {
            Ok(claimed) => claimed,
            Err(_) => {
                if self.is_fresh(client_ip, None) {
                    let ip = ip_or_empty(client_ip);
                    warn!(client_ip = %ip, "[auth_failure] insert_process body unparseable");
                }
                return;
            }
        };
        self.log_claims(client_ip, &claimed);
    }

    fn log_claims(&self, client_ip: Option<IpAddr>, claimed: &ClaimedProcessInfo) {
        let clean = |v: Option<&str>| sanitize_for_log(v.unwrap_or_default(), MAX_CLAIM_CHARS);
        let process_id = clean(claimed.process_id.as_deref());
        // `Some` even when absent so this key never collides with the `None` key of the
        // "not inspected" lines.
        if !self.is_fresh(client_ip, Some(process_id.clone())) {
            return;
        }
        let property = |key: &str| clean(claimed.properties.get(key).map(String::as_str));
        let ip = ip_or_empty(client_ip);
        let exe = clean(claimed.exe.as_deref());
        let username = clean(claimed.username.as_deref());
        let computer = clean(claimed.computer.as_deref());
        let parent_process_id = clean(claimed.parent_process_id.as_deref());
        let start_time = clean(claimed.start_time.as_deref());
        let platform_name = property(LOGGED_PROPERTY_KEYS[0]);
        let build_version = property(LOGGED_PROPERTY_KEYS[1]);
        let engine_version = property(LOGGED_PROPERTY_KEYS[2]);
        let build_config = property(LOGGED_PROPERTY_KEYS[3]);
        let build_target = property(LOGGED_PROPERTY_KEYS[4]);
        let branch_name = property(LOGGED_PROPERTY_KEYS[5]);
        let commit = property(LOGGED_PROPERTY_KEYS[6]);
        warn!(
            client_ip = %ip,
            claimed_process_id = %process_id,
            claimed_exe = %exe,
            claimed_username = %username,
            claimed_computer = %computer,
            claimed_parent_process_id = %parent_process_id,
            claimed_start_time = %start_time,
            claimed_platform_name = %platform_name,
            claimed_build_version = %build_version,
            claimed_engine_version = %engine_version,
            claimed_build_config = %build_config,
            claimed_build_target = %build_target,
            claimed_branch_name = %branch_name,
            claimed_commit = %commit,
            "[auth_failure] rejected insert_process caller claims"
        );
    }
}

fn ip_or_empty(ip: Option<IpAddr>) -> String {
    ip.map(|ip| ip.to_string()).unwrap_or_default()
}

/// Like [`micromegas_auth::axum::auth_middleware`], and additionally logs the claims of a
/// rejected `insert_process` request through `logger`.
pub async fn ingestion_auth_middleware(
    provider: Arc<dyn AuthProvider>,
    logger: Arc<RejectedProcessLogger>,
    mut req: Request,
    next: Next,
) -> Result<Response, AuthError> {
    match authenticate(provider.as_ref(), &mut req).await {
        Ok(()) => Ok(next.run(req).await),
        Err(AuthError::InvalidToken) if req.uri().path() == INSERT_PROCESS_PATH => {
            logger.log_rejected(req).await;
            Err(AuthError::InvalidToken)
        }
        Err(e) => Err(e),
    }
}
