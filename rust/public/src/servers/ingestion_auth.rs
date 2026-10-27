//! Authentication layer for the ingestion routes.
//!
//! Wraps [`micromegas_auth::axum::authenticate`] so a rejected `insert_process` request, the
//! first call any client makes, or an OTLP export, can have the identity it claims logged next
//! to the caller's network details. The 401 response is identical to the one `auth_middleware` returns.

use super::otlp::{OTLP_PATHS, content_type_encoding};
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use futures::StreamExt;
use micromegas_auth::axum::{
    AuthError, AuthRejection, QuietAuthError, authenticate, authenticate_quiet,
};
use micromegas_auth::client_ip::resolve_client_ip;
use micromegas_auth::log_sanitize::{RejectionFields, sanitize_for_log};
use micromegas_auth::types::AuthProvider;
use micromegas_otel_ingestion::rejected::claimed_resource_from_prefix;
use micromegas_tracing::prelude::*;
use moka::sync::Cache;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::Read;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

pub(crate) const INSERT_PROCESS_PATH: &str = "/ingestion/insert_process";

/// Caps every body read done for inspection: the `insert_process` body (a real `ProcessInfo` is
/// a few KiB) and the OTLP body prefix, both as sent and after decompression.
const MAX_INSPECTED_BODY_BYTES: usize = 64 * 1024;
const BODY_READ_TIMEOUT: Duration = Duration::from_secs(2);
const CBOR_RECURSION_LIMIT: usize = 16;
const MAX_CLAIM_CHARS: usize = 128;
const MAX_HEADER_CHARS: usize = 64;
/// Bounds the CPU and memory an unauthenticated caller can make us spend on inspection.
const DEFAULT_MAX_CONCURRENT_INSPECTIONS: usize = 8;

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

/// Reads and logs the claims of rejected `insert_process` and OTLP requests, at most once per
/// client IP and claimed identity per minute.
pub struct RejectedRequestLogger {
    recent: Cache<(Option<IpAddr>, Option<String>), ()>,
    inspections: Semaphore,
}

/// Request headers worth logging next to a rejection, already sanitized.
#[derive(Default)]
struct LoggedHeaders {
    content_type: String,
    content_encoding: String,
    content_length: String,
}

impl LoggedHeaders {
    fn from_request(req: &Request) -> Self {
        let clean = |name: axum::http::header::HeaderName| {
            let value = req.headers().get(name).and_then(|v| v.to_str().ok());
            sanitize_for_log(value.unwrap_or_default(), MAX_HEADER_CHARS)
        };
        Self {
            content_type: clean(axum::http::header::CONTENT_TYPE),
            content_encoding: clean(axum::http::header::CONTENT_ENCODING),
            content_length: clean(axum::http::header::CONTENT_LENGTH),
        }
    }
}

#[derive(Debug)]
enum PrefixError {
    Read,
    Timeout,
    Undecodable,
}

impl PrefixError {
    fn reason(&self) -> &'static str {
        match self {
            Self::Read => "read_error",
            Self::Timeout => "read_timeout",
            Self::Undecodable => "undecodable",
        }
    }
}

#[derive(Clone, Copy)]
enum WireEncoding {
    Identity,
    Gzip,
}

/// A deliberate superset of what `RequestDecompressionLayer` accepts (gzip and identity): it
/// tolerates case, `x-gzip` and an empty value, since it only decides whether to inspect a body
/// that has already been rejected. Anything else is not worth guessing at.
fn wire_encoding(req: &Request) -> Option<WireEncoding> {
    let Some(value) = req.headers().get(axum::http::header::CONTENT_ENCODING) else {
        return Some(WireEncoding::Identity);
    };
    match value.to_str().ok()?.trim().to_ascii_lowercase().as_str() {
        "" | "identity" => Some(WireEncoding::Identity),
        "gzip" | "x-gzip" => Some(WireEncoding::Gzip),
        _ => None,
    }
}

/// Reads at most `cap` bytes of the body as sent, then returns them decompressed and capped at
/// `cap` again, so a small gzip stream can't expand into an unbounded buffer.
async fn read_body_prefix(
    body: Body,
    encoding: WireEncoding,
    cap: usize,
) -> Result<Vec<u8>, PrefixError> {
    let mut stream = body.into_data_stream();
    let mut wire: Vec<u8> = vec![];
    let read = async {
        while wire.len() < cap {
            match stream.next().await {
                Some(Ok(chunk)) => wire.extend_from_slice(&chunk),
                Some(Err(_)) => return Err(PrefixError::Read),
                None => break,
            }
        }
        Ok(())
    };
    tokio::time::timeout(BODY_READ_TIMEOUT, read)
        .await
        .map_err(|_| PrefixError::Timeout)??;
    wire.truncate(cap);
    match encoding {
        WireEncoding::Identity => Ok(wire),
        WireEncoding::Gzip => {
            let mut out = vec![];
            let result = flate2::read::GzDecoder::new(&wire[..])
                .take(cap as u64)
                .read_to_end(&mut out);
            // The wire prefix usually ends mid-stream; what decoded before that is still useful.
            if result.is_err() && out.is_empty() {
                return Err(PrefixError::Undecodable);
            }
            Ok(out)
        }
    }
}

impl Default for RejectedRequestLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl RejectedRequestLogger {
    pub fn new() -> Self {
        Self {
            recent: Cache::builder()
                .max_capacity(10_000)
                .time_to_live(Duration::from_secs(60))
                .build(),
            inspections: Semaphore::new(DEFAULT_MAX_CONCURRENT_INSPECTIONS),
        }
    }

    pub fn with_max_concurrent_inspections(max: usize) -> Self {
        Self {
            inspections: Semaphore::new(max),
            ..Self::new()
        }
    }

    fn is_fresh(&self, client_ip: Option<IpAddr>, process_id: Option<String>) -> bool {
        self.recent
            .entry((client_ip, process_id))
            .or_insert(())
            .is_fresh()
    }

    fn log_not_inspected(&self, client_ip: Option<IpAddr>, reason: &str, headers: &LoggedHeaders) {
        if !self.is_fresh(client_ip, None) {
            return;
        }
        let ip = ip_or_empty(client_ip);
        warn!(
            client_ip = %ip,
            reason = %reason,
            content_type = %headers.content_type,
            content_encoding = %headers.content_encoding,
            content_length = %headers.content_length,
            "[auth_failure] insert_process body not inspected"
        );
    }

    /// Consumes the rejected request and logs what its body claims about the caller.
    pub async fn log_rejected(&self, req: Request) {
        let client_ip = resolve_client_ip(req.headers(), req.extensions());
        let headers = LoggedHeaders::from_request(&req);
        let declared = req
            .headers()
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok());
        let Some(declared) = declared else {
            self.log_not_inspected(client_ip, "missing_content_length", &headers);
            return;
        };
        if declared > MAX_INSPECTED_BODY_BYTES {
            self.log_not_inspected(client_ip, "oversize", &headers);
            return;
        }
        let body = req.into_body();
        let bytes =
            match tokio::time::timeout(BODY_READ_TIMEOUT, to_bytes(body, MAX_INSPECTED_BODY_BYTES))
                .await
            {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(_)) => {
                    self.log_not_inspected(client_ip, "read_error", &headers);
                    return;
                }
                Err(_) => {
                    self.log_not_inspected(client_ip, "read_timeout", &headers);
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

    /// Consumes a rejected OTLP request and logs its single rejection line: the provider reason,
    /// plus the resource identity the body claims or why it could not be read.
    pub async fn log_rejected_otlp(&self, req: Request, rejection: &AuthRejection) {
        let client_ip = resolve_client_ip(req.headers(), req.extensions());
        let headers = LoggedHeaders::from_request(&req);
        let base = RejectionFields::new(&rejection.parts, &rejection.error);
        match self.inspect_otlp(req).await {
            Ok((claims, identity_key)) => {
                // One set of claims per distinct sender, so several apps behind one NAT address
                // all show up; a repeat still logs the rejection, without the claims.
                if self.is_fresh(client_ip, Some(identity_key)) {
                    warn_otlp_with_claims(&base, &headers, &claims);
                } else {
                    warn_otlp_base(&base, &headers);
                }
            }
            Err(reason) => warn_otlp_not_inspected(&base, &headers, reason),
        }
    }

    async fn inspect_otlp(&self, req: Request) -> Result<(OtlpClaims, String), &'static str> {
        let encoding =
            content_type_encoding(req.headers()).map_err(|_| "unsupported_content_type")?;
        let wire = wire_encoding(&req).ok_or("unsupported_encoding")?;
        let _permit = self.inspections.try_acquire().map_err(|_| "busy")?;
        let prefix = read_body_prefix(req.into_body(), wire, MAX_INSPECTED_BODY_BYTES)
            .await
            .map_err(|e| e.reason())?;
        let claimed = claimed_resource_from_prefix(&prefix, encoding).ok_or("no_resource")?;
        let clean = |name: &str| sanitize_for_log(claimed.get(name), MAX_CLAIM_CHARS);
        let claims = OtlpClaims {
            exe: clean("claimed_exe"),
            computer: clean("claimed_computer"),
            instance_id: clean("claimed_service_instance_id"),
            sdk_language: clean("claimed_sdk_language"),
            sdk_name: clean("claimed_sdk_name"),
            sdk_version: clean("claimed_sdk_version"),
            service_version: clean("claimed_service_version"),
            deployment_environment: clean("claimed_deployment_environment"),
            k8s_namespace: clean("claimed_k8s_namespace"),
            k8s_pod: clean("claimed_k8s_pod"),
            k8s_deployment: clean("claimed_k8s_deployment"),
            k8s_cluster: clean("claimed_k8s_cluster"),
            container: clean("claimed_container"),
            cloud_region: clean("claimed_cloud_region"),
            pid: clean("claimed_pid"),
            runtime: clean("claimed_runtime"),
            username: clean("claimed_username"),
        };
        // Built from the truncated values so the key size is bounded.
        let identity_key = format!(
            "otlp\x1f{}\x1f{}\x1f{}",
            claims.exe, claims.computer, claims.instance_id
        );
        Ok((claims, identity_key))
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

/// Sanitized resource identity claimed by a rejected OTLP request.
struct OtlpClaims {
    exe: String,
    computer: String,
    instance_id: String,
    sdk_language: String,
    sdk_name: String,
    sdk_version: String,
    service_version: String,
    deployment_environment: String,
    k8s_namespace: String,
    k8s_pod: String,
    k8s_deployment: String,
    k8s_cluster: String,
    container: String,
    cloud_region: String,
    pid: String,
    runtime: String,
    username: String,
}

// Tracing macros need a fixed field list per call site, hence one function per variant.
fn warn_otlp_base(base: &RejectionFields, headers: &LoggedHeaders) {
    warn!(
        client_ip = %base.client_ip,
        method = %base.method,
        path = %base.path,
        user_agent = %base.user_agent,
        content_type = %headers.content_type,
        content_encoding = %headers.content_encoding,
        content_length = %headers.content_length,
        "[auth_failure] {}",
        base.reason
    );
}

fn warn_otlp_not_inspected(base: &RejectionFields, headers: &LoggedHeaders, inspection: &str) {
    warn!(
        client_ip = %base.client_ip,
        method = %base.method,
        path = %base.path,
        user_agent = %base.user_agent,
        content_type = %headers.content_type,
        content_encoding = %headers.content_encoding,
        content_length = %headers.content_length,
        inspection = %inspection,
        "[auth_failure] {}",
        base.reason
    );
}

fn warn_otlp_with_claims(base: &RejectionFields, headers: &LoggedHeaders, c: &OtlpClaims) {
    warn!(
        client_ip = %base.client_ip,
        method = %base.method,
        path = %base.path,
        user_agent = %base.user_agent,
        content_type = %headers.content_type,
        content_encoding = %headers.content_encoding,
        content_length = %headers.content_length,
        claimed_exe = %c.exe,
        claimed_computer = %c.computer,
        claimed_sdk_language = %c.sdk_language,
        claimed_sdk_name = %c.sdk_name,
        claimed_sdk_version = %c.sdk_version,
        claimed_service_version = %c.service_version,
        claimed_service_instance_id = %c.instance_id,
        claimed_deployment_environment = %c.deployment_environment,
        claimed_k8s_namespace = %c.k8s_namespace,
        claimed_k8s_pod = %c.k8s_pod,
        claimed_k8s_deployment = %c.k8s_deployment,
        claimed_k8s_cluster = %c.k8s_cluster,
        claimed_container = %c.container,
        claimed_cloud_region = %c.cloud_region,
        claimed_pid = %c.pid,
        claimed_runtime = %c.runtime,
        claimed_username = %c.username,
        "[auth_failure] {}",
        base.reason
    );
}

fn ip_or_empty(ip: Option<IpAddr>) -> String {
    ip.map(|ip| ip.to_string()).unwrap_or_default()
}

/// Like [`micromegas_auth::axum::auth_middleware`], and additionally logs the claims of a
/// rejected `insert_process` or OTLP request through `logger`.
pub async fn ingestion_auth_middleware(
    provider: Arc<dyn AuthProvider>,
    logger: Arc<RejectedRequestLogger>,
    mut req: Request,
    next: Next,
) -> Result<Response, AuthError> {
    let path = req.uri().path().to_owned();
    if OTLP_PATHS.contains(&path.as_str()) {
        return match authenticate_quiet(provider.as_ref(), &mut req).await {
            Ok(()) => Ok(next.run(req).await),
            Err(QuietAuthError::Rejected(rejection)) => {
                logger.log_rejected_otlp(req, &rejection).await;
                Err(AuthError::InvalidToken)
            }
            Err(QuietAuthError::Unavailable) => Err(AuthError::Unavailable),
        };
    }
    match authenticate(provider.as_ref(), &mut req).await {
        Ok(()) => Ok(next.run(req).await),
        Err(AuthError::InvalidToken) => {
            if path == INSERT_PROCESS_PATH {
                logger.log_rejected(req).await;
            }
            Err(AuthError::InvalidToken)
        }
        Err(e) => Err(e),
    }
}

// Lives here because `read_body_prefix` is private and the bound on its output length is the
// property under test.
#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut enc = GzEncoder::new(vec![], Compression::best());
        enc.write_all(bytes).expect("gzip write");
        enc.finish().expect("gzip finish")
    }

    #[tokio::test]
    async fn gzip_bomb_is_stopped_at_the_cap() {
        let bomb = gzip(&vec![0u8; 64 * 1024 * 1024]);
        assert!(bomb.len() < MAX_INSPECTED_BODY_BYTES);
        let out = read_body_prefix(
            Body::from(bomb),
            WireEncoding::Gzip,
            MAX_INSPECTED_BODY_BYTES,
        )
        .await
        .expect("prefix");
        assert_eq!(out.len(), MAX_INSPECTED_BODY_BYTES);
    }

    #[tokio::test]
    async fn plain_body_is_cut_at_the_cap() {
        let out = read_body_prefix(
            Body::from(vec![1u8; MAX_INSPECTED_BODY_BYTES * 3]),
            WireEncoding::Identity,
            MAX_INSPECTED_BODY_BYTES,
        )
        .await
        .expect("prefix");
        assert_eq!(out.len(), MAX_INSPECTED_BODY_BYTES);
    }

    #[tokio::test]
    async fn invalid_gzip_is_undecodable() {
        let err = read_body_prefix(Body::from("nope"), WireEncoding::Gzip, 1024)
            .await
            .expect_err("undecodable");
        assert!(matches!(err, PrefixError::Undecodable));
    }
}
