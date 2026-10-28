//! Authentication layer for the ingestion routes.
//!
//! Wraps [`micromegas_auth::axum::authenticate_quiet`] so a rejected `insert_process` request, the
//! first call any client makes, or an OTLP export, is logged as one line carrying both the
//! rejection reason and the identity its body claims. The 401 response is identical to the one
//! `auth_middleware` returns.

use super::otlp::{OTLP_PATHS, content_type_encoding};
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use futures::StreamExt;
use micromegas_auth::axum::{
    AuthError, AuthRejection, QuietAuthError, authenticate, authenticate_quiet,
};
use micromegas_auth::log_sanitize::{RejectionFields, sanitize_for_log};
use micromegas_auth::types::AuthProvider;
use micromegas_otel_ingestion::rejected::claimed_resource_from_prefix;
use micromegas_tracing::dispatch;
use micromegas_tracing::logs::{FILTER_LEVEL_UNSET_VALUE, LogMetadata, LogMsgForm};
use micromegas_tracing::prelude::*;
use micromegas_tracing::property_set::PropertySet;
use moka::sync::Cache;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::Read;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
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

/// Build-identity properties worth logging, with their log property names. `command-line` and
/// `exe_args` are left out because they can carry secrets passed on the command line.
const LOGGED_PROPERTIES: [(&str, &str); 7] = [
    ("platform-name", "claimed_platform_name"),
    ("build-version", "claimed_build_version"),
    ("engine-version", "claimed_engine_version"),
    ("build-config", "claimed_build_config"),
    ("build-target", "claimed_build_target"),
    ("branch-name", "claimed_branch_name"),
    ("commit", "claimed_commit"),
];

/// Logs every rejected `insert_process` and OTLP request, with the claims of its body attached at
/// most once per client IP and claimed sender per minute.
pub struct RejectedRequestLogger {
    recent: Cache<(Option<IpAddr>, String), ()>,
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

    /// `sender` is built from truncated claims, so the key size is bounded.
    fn is_fresh(&self, client_ip: Option<IpAddr>, sender: String) -> bool {
        self.recent
            .entry((client_ip, sender))
            .or_insert(())
            .is_fresh()
    }

    /// Consumes a rejected `insert_process` or OTLP request and logs its single rejection line:
    /// the provider reason, plus the identity the body claims or why it could not be read.
    pub async fn log_rejected(&self, req: Request, rejection: &AuthRejection) {
        let headers = LoggedHeaders::from_request(&req);
        let inspected = if req.uri().path() == INSERT_PROCESS_PATH {
            self.inspect_insert_process(req).await
        } else {
            self.inspect_otlp(req).await
        };
        let inspection = match inspected {
            // One set of claims per distinct sender, so several apps behind one NAT address all
            // show up; a repeat still logs the rejection, without the claims.
            Ok(claims) => {
                if self.is_fresh(rejection.parts.client_ip, claims.sender) {
                    Inspection::Claims(claims.properties)
                } else {
                    Inspection::Repeat
                }
            }
            Err(reason) => Inspection::NotInspected(reason),
        };
        let base = RejectionFields::new(&rejection.parts, &rejection.error);
        warn_rejection(&base, &headers, &inspection);
    }

    async fn inspect_insert_process(&self, req: Request) -> Result<Claims, &'static str> {
        let declared = req
            .headers()
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok())
            .ok_or("missing_content_length")?;
        if declared > MAX_INSPECTED_BODY_BYTES {
            return Err("oversize");
        }
        let _permit = self.inspections.try_acquire().map_err(|_| "busy")?;
        let read = to_bytes(req.into_body(), MAX_INSPECTED_BODY_BYTES);
        let bytes = match tokio::time::timeout(BODY_READ_TIMEOUT, read).await {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(_)) => return Err(PrefixError::Read.reason()),
            Err(_) => return Err(PrefixError::Timeout.reason()),
        };
        let claimed: ClaimedProcessInfo =
            ciborium::de::from_reader_with_recursion_limit(&bytes[..], CBOR_RECURSION_LIMIT)
                .map_err(|_| PrefixError::Undecodable.reason())?;
        Ok(claimed.claims())
    }

    async fn inspect_otlp(&self, req: Request) -> Result<Claims, &'static str> {
        let encoding =
            content_type_encoding(req.headers()).map_err(|_| "unsupported_content_type")?;
        let wire = wire_encoding(&req).ok_or("unsupported_encoding")?;
        let _permit = self.inspections.try_acquire().map_err(|_| "busy")?;
        let prefix = read_body_prefix(req.into_body(), wire, MAX_INSPECTED_BODY_BYTES)
            .await
            .map_err(|e| e.reason())?;
        let claimed = claimed_resource_from_prefix(&prefix, encoding).ok_or("no_resource")?;
        let clean = |name: &str| clean_claim(claimed.get(name));
        let sender = format!(
            "otlp\x1f{}\x1f{}\x1f{}",
            clean("claimed_exe"),
            clean("claimed_computer"),
            clean("claimed_service_instance_id")
        );
        let properties = claim_properties(
            claimed
                .properties()
                .iter()
                .map(|(name, value)| (*name, value.as_str())),
        );
        Ok(Claims { sender, properties })
    }
}

impl ClaimedProcessInfo {
    fn claims(&self) -> Claims {
        fn field(value: &Option<String>) -> &str {
            value.as_deref().unwrap_or_default()
        }
        let fields = [
            ("claimed_process_id", field(&self.process_id)),
            ("claimed_exe", field(&self.exe)),
            ("claimed_username", field(&self.username)),
            ("claimed_computer", field(&self.computer)),
            ("claimed_parent_process_id", field(&self.parent_process_id)),
            ("claimed_start_time", field(&self.start_time)),
        ];
        let build = LOGGED_PROPERTIES.iter().map(|(key, name)| {
            let value = self.properties.get(*key).map(String::as_str);
            (*name, value.unwrap_or_default())
        });
        Claims {
            sender: format!("insert_process\x1f{}", clean_claim(field(&self.process_id))),
            properties: claim_properties(fields.into_iter().chain(build)),
        }
    }
}

/// Sanitized identity a rejected request claims.
struct Claims {
    /// Tells apart the senders behind one client IP.
    sender: String,
    /// `claimed_*` log properties; the ones the caller left empty are omitted.
    properties: Vec<(&'static str, String)>,
}

enum Inspection {
    Claims(Vec<(&'static str, String)>),
    /// The sender's claims were already logged within the last minute.
    Repeat,
    NotInspected(&'static str),
}

fn clean_claim(value: &str) -> String {
    sanitize_for_log(value, MAX_CLAIM_CHARS)
}

fn claim_properties<'a>(
    claims: impl IntoIterator<Item = (&'static str, &'a str)>,
) -> Vec<(&'static str, String)> {
    claims
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(name, value)| (name, clean_claim(value)))
        .collect()
}

/// Logged from a runtime property list rather than `warn!`, whose fields are fixed per call
/// site: which claims are present depends on the route and on what the caller sent.
fn warn_rejection(base: &RejectionFields, headers: &LoggedHeaders, inspection: &Inspection) {
    static DESC: LogMetadata = LogMetadata {
        level: Level::Warn,
        level_filter: AtomicU32::new(FILTER_LEVEL_UNSET_VALUE),
        fmt_str: "[auth_failure] {}",
        target: module_path!(),
        module_path: module_path!(),
        file: file!(),
        line: line!(),
    };
    if Level::Warn > STATIC_MAX_LEVEL || Level::Warn > max_level() {
        return;
    }
    let mut properties = vec![
        DynProperty::new("client_ip", &base.client_ip),
        DynProperty::new("method", &base.method),
        DynProperty::new("path", &base.path),
        DynProperty::new("user_agent", &base.user_agent),
        DynProperty::new("content_type", &headers.content_type),
        DynProperty::new("content_encoding", &headers.content_encoding),
        DynProperty::new("content_length", &headers.content_length),
    ];
    match inspection {
        Inspection::Claims(claims) => properties.extend(
            claims
                .iter()
                .map(|(name, value)| DynProperty::new(name, value)),
        ),
        Inspection::Repeat => {}
        Inspection::NotInspected(reason) => {
            properties.push(DynProperty::new("inspection", reason));
        }
    }
    dispatch::log_dyn_tagged(
        &DESC,
        PropertySet::empty(),
        &properties,
        LogMsgForm::Formatted,
        format_args!("[auth_failure] {}", base.reason),
    );
}

/// Like [`micromegas_auth::axum::auth_middleware`], except that a rejected `insert_process` or
/// OTLP request is logged through `logger`, as one line that includes what its body claims.
pub async fn ingestion_auth_middleware(
    provider: Arc<dyn AuthProvider>,
    logger: Arc<RejectedRequestLogger>,
    mut req: Request,
    next: Next,
) -> Result<Response, AuthError> {
    let path = req.uri().path();
    if path != INSERT_PROCESS_PATH && !OTLP_PATHS.contains(&path) {
        authenticate(provider.as_ref(), &mut req).await?;
        return Ok(next.run(req).await);
    }
    match authenticate_quiet(provider.as_ref(), &mut req).await {
        Ok(()) => Ok(next.run(req).await),
        Err(QuietAuthError::Rejected(rejection)) => {
            logger.log_rejected(req, &rejection).await;
            Err(AuthError::InvalidToken)
        }
        Err(QuietAuthError::Unavailable) => Err(AuthError::Unavailable),
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
