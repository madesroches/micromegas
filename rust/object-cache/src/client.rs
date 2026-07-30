use anyhow::{Context, Result, anyhow};
use async_stream::stream as gen_stream;
use async_trait::async_trait;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures::stream::TryStreamExt;
use futures::stream::{self, BoxStream, StreamExt};
use micromegas_tracing::prelude::*;
use object_store::{
    Attributes, CopyOptions, GetOptions, GetRange, GetResult, GetResultPayload, ListResult,
    MultipartUpload, ObjectMeta, ObjectStore, PutMultipartOptions, PutOptions, PutPayload,
    PutResult, path::Path,
};
use reqwest::Client;
use serde_json::json;
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::circuit_breaker::{Admission, CircuitBreaker, CircuitBreakerConfig, Transition};
use crate::prefetch::{ObjectPrefetch, PrefetchItem, PrefetchResponse};

/// Fail fast if the cache server can't be reached, so reads fall back to the
/// direct store instead of stalling on a hung connection. Spans DNS
/// resolution as well as the TCP/TLS handshake (reqwest wraps the whole
/// connector service). Calibrated for the only deployment this client
/// documents -- a private, intra-VPC `object-cache` -- see "Calibrating
/// `abandon_timeout`" in the design doc for the reasoning; raise it via
/// `MICROMEGAS_OBJECT_CACHE_CLIENT_CONNECT_TIMEOUT_MS` for TLS/cross-zone/
/// clustered-DNS deployments.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_millis(50);
/// The direct-path race budget: every phase where the cache can lose (which,
/// with the mid-stream resume fix in place, is all of them) is bounded by
/// this. Calibrated at the production origin-fetch latency distribution from
/// issue #1360 (p99 ~311ms, max ~575ms) -- see "Calibrating
/// `abandon_timeout`".
const DEFAULT_ABANDON_TIMEOUT: Duration = Duration::from_millis(500);
/// Per-chunk bound on body reassembly (`get_ranges`' `pull_exact`, and --
/// indirectly, via `read_timeout` -- the `GET /obj` response body). Looser
/// than `abandon_timeout` because a mid-stream abandon re-reads the
/// remainder not yet delivered: a throughput cost knob, not a correctness
/// bound. Also reused, unmodified, as the breaker's `cooldown`. Not an env
/// var -- see "The constants that are not env vars".
const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(3);
/// Overall per-request deadline (`ClientBuilder::timeout`), unchanged from
/// before this plan. A rare backstop behind the tighter bounds above for a
/// healthy cache; still reports `record_unresponsive` on expiry. Not an env
/// var.
const DEFAULT_TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
/// Consecutive unresponsive requests that trip the circuit breaker. Not an
/// env var by default value, but overridable; `0` disables the breaker.
const DEFAULT_BREAKER_THRESHOLD: u32 = 5;

const ENV_CONNECT_TIMEOUT_MS: &str = "MICROMEGAS_OBJECT_CACHE_CLIENT_CONNECT_TIMEOUT_MS";
const ENV_ABANDON_TIMEOUT_MS: &str = "MICROMEGAS_OBJECT_CACHE_CLIENT_ABANDON_TIMEOUT_MS";
const ENV_BREAKER_THRESHOLD: &str = "MICROMEGAS_OBJECT_CACHE_CLIENT_BREAKER_THRESHOLD";

/// Tunables for `CacheClientStore`. `Default` carries the production values,
/// `from_env` applies operator overrides, and tests construct one directly
/// (short timeouts, near-zero or very long cooldown) instead of sleeping.
#[derive(Debug, Clone)]
pub struct CacheClientConfig {
    pub connect_timeout: Duration,
    /// The direct-path race budget: every phase where the cache can lose,
    /// which after Phase 0 is all of them. Applied at the header (time-to-
    /// headers) phase of the five `send()` sites.
    pub abandon_timeout: Duration,
    /// Per-chunk bound on `get_ranges`' `pull_exact` read. Looser than
    /// `abandon_timeout` because a mid-stream abandon re-reads the ranges
    /// not yet delivered -- a cost knob, not a correctness bound. Also backs
    /// the `GET /obj` response body's per-frame bound, but *not* directly:
    /// the client's `ClientBuilder::read_timeout` is set to `stall_timeout +
    /// abandon_timeout` (looser by a margin), so `/ranges`' own explicit
    /// `pull_exact` wrap -- set to `stall_timeout` exactly -- is always the
    /// one that fires first on a real stall, never `read_timeout`. Reused as
    /// the breaker's `cooldown`.
    pub stall_timeout: Duration,
    pub total_timeout: Duration,
    pub breaker: CircuitBreakerConfig,
}

impl Default for CacheClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            abandon_timeout: DEFAULT_ABANDON_TIMEOUT,
            stall_timeout: DEFAULT_STALL_TIMEOUT,
            total_timeout: DEFAULT_TOTAL_TIMEOUT,
            breaker: CircuitBreakerConfig {
                failure_threshold: DEFAULT_BREAKER_THRESHOLD,
                // Reuse `stall_timeout` rather than adding a second knob --
                // see "Why the cooldown is fixed". Kept in sync by
                // `from_env` too; a test overriding `stall_timeout` sets the
                // cooldown it wants explicitly.
                cooldown: DEFAULT_STALL_TIMEOUT,
            },
        }
    }
}

impl CacheClientConfig {
    /// Apply `MICROMEGAS_OBJECT_CACHE_CLIENT_*` operator overrides on top of
    /// `Default`. `stall_timeout`/`total_timeout` stay fixed named constants
    /// (see "The constants that are not env vars"); only the connect budget,
    /// abandon budget, and breaker threshold are env-overridable.
    pub fn from_env() -> Self {
        let connect_timeout = env_millis(ENV_CONNECT_TIMEOUT_MS, DEFAULT_CONNECT_TIMEOUT);
        let abandon_timeout = env_millis(ENV_ABANDON_TIMEOUT_MS, DEFAULT_ABANDON_TIMEOUT);
        let failure_threshold = env_u32(ENV_BREAKER_THRESHOLD, DEFAULT_BREAKER_THRESHOLD);
        Self {
            connect_timeout,
            abandon_timeout,
            breaker: CircuitBreakerConfig {
                failure_threshold,
                ..Self::default().breaker
            },
            ..Self::default()
        }
    }
}

/// Parse a millisecond duration from the environment, warning and falling
/// back to `default` on an invalid (non-integer) value. Mirrors the
/// warn-and-default pattern in `l1_store.rs`.
fn env_millis(var: &str, default: Duration) -> Duration {
    match std::env::var(var) {
        Ok(s) => match s.parse::<u64>() {
            Ok(ms) => Duration::from_millis(ms),
            Err(_) => {
                warn!("Invalid {var} value '{s}', using default {default:?}");
                default
            }
        },
        Err(_) => default,
    }
}

/// Parse a `u32` from the environment, warning and falling back to `default`
/// on an invalid value. Mirrors the warn-and-default pattern in `l1_store.rs`.
fn env_u32(var: &str, default: u32) -> u32 {
    match std::env::var(var) {
        Ok(s) => match s.parse::<u32>() {
            Ok(v) => v,
            Err(_) => {
                warn!("Invalid {var} value '{s}', using default {default}");
                default
            }
        },
        Err(_) => default,
    }
}

#[derive(Debug)]
pub struct CacheClientStore {
    http: Client,
    cache_base_url: String,
    api_key: Option<String>,
    direct: Arc<dyn ObjectStore>,
    config: CacheClientConfig,
    breaker: Arc<CircuitBreaker>,
}

impl CacheClientStore {
    pub fn new(
        cache_base_url: String,
        api_key: Option<String>,
        direct: Arc<dyn ObjectStore>,
    ) -> Self {
        Self::with_config(
            cache_base_url,
            api_key,
            direct,
            CacheClientConfig::from_env(),
        )
    }

    /// Like `new`, but with an explicit `CacheClientConfig` instead of
    /// reading the environment -- what tests use to install short timeouts
    /// and a controllable cooldown.
    pub fn with_config(
        cache_base_url: String,
        api_key: Option<String>,
        direct: Arc<dyn ObjectStore>,
        config: CacheClientConfig,
    ) -> Self {
        // One client for every endpoint. `read_timeout` gives the `/obj`
        // response body (and `prefetch`'s `resp.bytes()`) its per-frame
        // bound; it's set with a margin above `stall_timeout` so `/ranges`'
        // own explicit `pull_exact` wrap is unambiguously the first of the
        // two to fire on a real stall -- see "`ClientBuilder::read_timeout`
        // on the one client".
        let http = Client::builder()
            .connect_timeout(config.connect_timeout)
            .read_timeout(config.stall_timeout + config.abandon_timeout)
            .timeout(config.total_timeout)
            .build()
            .expect("building reqwest client");
        let breaker = Arc::new(CircuitBreaker::new(config.breaker.clone()));
        Self {
            http,
            cache_base_url,
            api_key,
            direct,
            config,
            breaker,
        }
    }

    fn obj_url(&self, location: &Path) -> String {
        format!(
            "{}/obj/{}",
            self.cache_base_url.trim_end_matches('/'),
            location.as_ref()
        )
    }

    fn add_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(key) = &self.api_key {
            req.bearer_auth(key)
        } else {
            req
        }
    }

    /// Send a request to the cache, bounding time-to-headers with
    /// `config.abandon_timeout` and reporting *failures* to the circuit
    /// breaker. Success is deliberately NOT reported here: a single logical
    /// read can issue two requests (Suffix does `head_size` then
    /// `get_range_stream`), and reporting responsive at each time-to-headers
    /// would zero `consecutive` so a later-phase failure could never trip
    /// the breaker. See "One outcome per logical operation". Every call site
    /// is recoverable -- dropping the future cancels the request and lands
    /// in the existing fallback -- which is what makes one budget correct
    /// for all of them.
    async fn send(&self, req: reqwest::RequestBuilder, what: &str) -> Result<reqwest::Response> {
        let budget = self.config.abandon_timeout;
        match tokio::time::timeout(budget, req.send()).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(e)) => {
                self.report_unresponsive(what);
                Err(e).with_context(|| format!("sending {what} to cache"))
            }
            Err(_) => {
                self.report_abandoned(what);
                Err(anyhow!(
                    "cache {what} did not beat the direct path within {budget:?}"
                ))
            }
        }
    }

    /// The single success report for a logical operation: the resource
    /// answered (any HTTP status), so the circuit is healthy.
    fn report(&self) {
        report_transition(self.breaker.record_responsive());
    }

    /// The resource is not answering at all: connect failure, transport
    /// error, or a `stall_timeout` expiry (see "Abandon vs. unresponsive").
    fn report_unresponsive(&self, what: &str) {
        report_unresponsive(what, &self.breaker);
    }

    /// The cache lost the race against the direct path (an `abandon_timeout`
    /// expiry). Called only from `send`.
    fn report_abandoned(&self, what: &str) {
        imetric!("range_cache_client_abandoned", "count", 1_u64);
        debug!("cache {what} abandoned: did not beat the direct path");
        report_transition(self.breaker.record_unresponsive());
    }

    /// Fall back to the direct store for a whole `get_opts` operation,
    /// bumping the shared `range_cache_client_fallback` counter and timing
    /// `range_cache_client_direct_ms`, mirroring
    /// `L1CacheStore::fallback_get_opts`.
    async fn direct_get_opts(
        &self,
        location: &Path,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        direct_get_opts_with_metrics(&self.direct, location, options).await
    }

    /// Fall back to the direct store for a whole `get_ranges` operation; see
    /// `direct_get_opts`.
    async fn direct_get_ranges(
        &self,
        location: &Path,
        ranges: &[Range<u64>],
    ) -> object_store::Result<Vec<Bytes>> {
        imetric!("range_cache_client_fallback", "count", 1_u64);
        let direct_start = Instant::now();
        let result = self.direct.get_ranges(location, ranges).await;
        fmetric!(
            "range_cache_client_direct_ms",
            "ms",
            direct_start.elapsed().as_secs_f64() * 1000.0
        );
        result
    }

    /// Issue a range GET and build a streaming `GetResult`, mirroring
    /// `get_full_stream` but for ranged reads: the body is streamed rather
    /// than buffered with `.bytes()`, which would otherwise materialize the
    /// whole range (now unbounded, since the server no longer caps total
    /// requested bytes) as one contiguous allocation before any of it is
    /// used. The actual served byte range and the full object size both come
    /// from the 206's `Content-Range: bytes {start}-{end}/{size}` header
    /// rather than a buffered body length, avoiding a separate HEAD
    /// round-trip in the common case.
    ///
    /// An open-ended `end` (`None`) requests `bytes={start}-`, i.e. from `start`
    /// to the end of the object, which the cache server resolves against the
    /// true object size.
    ///
    /// `options` (carrying the original requested range) is threaded through
    /// so a stream error reaching the consumer can fall back to `self.direct`
    /// for the remainder of the same range, via `full_stream_with_fallback`.
    async fn get_range_stream(
        &self,
        location: &Path,
        start: u64,
        end: Option<u64>,
        options: GetOptions,
    ) -> Result<GetResult> {
        let round_trip_start = Instant::now();
        let url = self.obj_url(location);
        let range_header = match end {
            Some(end) => format!("bytes={}-{}", start, end.saturating_sub(1)),
            None => format!("bytes={start}-"),
        };
        let resp = self
            .send(
                self.add_auth(self.http.get(&url))
                    .header("Range", range_header),
                "GET",
            )
            .await?;
        if !resp.status().is_success() {
            // A full HTTP response arrived cheaply -- see "Abandon vs.
            // unresponsive" ("any HTTP response on a demand-read endpoint
            // counts as responsive").
            self.report();
            return Err(anyhow!("cache GET {url} status {}", resp.status()));
        }

        if let Some((served_range, object_size)) = parse_content_range(resp.headers()) {
            let raw = resp
                .bytes_stream()
                .map_err(|e| object_store::Error::Generic {
                    store: "CacheClientStore",
                    source: Box::new(e),
                })
                .boxed();
            let body = full_stream_with_fallback(
                self.direct.clone(),
                self.breaker.clone(),
                location.clone(),
                options,
                served_range.clone(),
                raw,
            );
            // Measured at time-to-usable-stream, i.e. before any body bytes
            // are read (the body streams lazily) — distinct from
            // `range_cache_client_ranges_ms`, which covers the buffered
            // `/ranges` path and is measured after the full body is read.
            fmetric!(
                "range_cache_client_roundtrip_ms",
                "ms",
                round_trip_start.elapsed().as_secs_f64() * 1000.0
            );
            return Ok(stream_get_result(location, body, served_range, object_size));
        }

        // No `Content-Range`: the server serves a zero-length range (an
        // empty/zero-byte object, or an open-ended range starting exactly at
        // EOF) as a plain 200 with an empty body rather than a 206 (see
        // `get_range_handler`), so there's nothing to stream. The full object
        // size still isn't known from this response; resolve it with a HEAD.
        // `head_size` is the tail of this operation -- no stream follows --
        // so this call site owns the single success report.
        let size = self.head_size(location).await?;
        self.report();
        fmetric!(
            "range_cache_client_roundtrip_ms",
            "ms",
            round_trip_start.elapsed().as_secs_f64() * 1000.0
        );
        Ok(build_get_result(location, Bytes::new(), 0..0, size))
    }

    /// Issue an unranged GET and build a streaming `GetResult`, mapping the
    /// response body to `GetResultPayload::Stream` so the whole object is never
    /// buffered in memory (matching how the direct store streams the body). The
    /// object size comes from the `Content-Length` header, which is required to
    /// populate `meta.size` and the `0..size` range without reading the body.
    ///
    /// The body is wrapped with `full_stream_with_fallback` so a stream error
    /// reaching the consumer transparently resumes the remainder from
    /// `self.direct`, at the `GetResult` level instead of the whole-response
    /// level, since this path streams rather than buffers (the ranged path,
    /// `get_range_stream`, uses the same helper for the same reason).
    async fn get_full_stream(&self, location: &Path, options: GetOptions) -> Result<GetResult> {
        let round_trip_start = Instant::now();
        let url = self.obj_url(location);
        let resp = self.send(self.add_auth(self.http.get(&url)), "GET").await?;
        if !resp.status().is_success() {
            self.report();
            return Err(anyhow!("cache GET {url} status {}", resp.status()));
        }
        let size = match resp.content_length() {
            Some(size) => size,
            None => {
                // A malformed response after a cheap 2xx is a protocol
                // violation from our own cache, not a liveness signal -- see
                // "What does not feed the breaker as a failure".
                self.report();
                return Err(anyhow!("missing Content-Length in GET response"));
            }
        };
        let raw = resp
            .bytes_stream()
            .map_err(|e| object_store::Error::Generic {
                store: "CacheClientStore",
                source: Box::new(e),
            })
            .boxed();
        let body = full_stream_with_fallback(
            self.direct.clone(),
            self.breaker.clone(),
            location.clone(),
            options,
            0..size,
            raw,
        );
        // Measured at time-to-usable-stream, before any body bytes are read
        // (see the matching comment in `get_range_stream`).
        fmetric!(
            "range_cache_client_roundtrip_ms",
            "ms",
            round_trip_start.elapsed().as_secs_f64() * 1000.0
        );
        Ok(stream_get_result(location, body, 0..size, size))
    }

    /// Shared by three call sites (the head-only path, the no-`Content-Range`
    /// path in `get_range_stream`, and the Suffix path). Reports
    /// `record_responsive` internally on its own two after-2xx failure arms
    /// (non-2xx status, malformed/missing `Content-Length`) -- both are
    /// terminal for every caller, so those reports fire identically no matter
    /// which site is calling. The *success* report is left to whichever call
    /// site owns the whole logical operation (see "One outcome per logical
    /// operation").
    async fn head_size(&self, location: &Path) -> Result<u64> {
        let url = self.obj_url(location);
        let resp = self
            .send(self.add_auth(self.http.head(&url)), "HEAD")
            .await?;
        if !resp.status().is_success() {
            self.report();
            return Err(anyhow!("cache HEAD {url} status {}", resp.status()));
        }
        match resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
        {
            Some(size) => Ok(size),
            None => {
                self.report();
                Err(anyhow!("missing Content-Length in HEAD response"))
            }
        }
    }

    /// POST a batch of keys to warm at the cache server's prefetch priority.
    /// Best-effort: there is no demand read to fall back to, so callers should
    /// treat an `Err` as "the warm didn't happen" and move on rather than
    /// retrying inline.
    ///
    /// Gated on `admit_bypass_only()`, not `admit()`: prefetch never closes
    /// the circuit and must not be able to consume the single per-cooldown
    /// probe slot a demand read would otherwise receive -- see "Prefetch
    /// does not close the circuit".
    pub async fn prefetch(&self, items: Vec<PrefetchItem>) -> Result<PrefetchResponse> {
        if matches!(self.breaker.admit_bypass_only(), Admission::Bypass) {
            imetric!("range_cache_client_circuit_bypassed", "count", 1_u64);
            debug!(
                "cache circuit open, dropping {} prefetch item(s)",
                items.len()
            );
            return Ok(PrefetchResponse {
                accepted: 0,
                rejected: 0,
                dropped: items.len(),
            });
        }

        let url = format!("{}/prefetch", self.cache_base_url.trim_end_matches('/'));
        let mut body = Vec::new();
        for item in &items {
            serde_json::to_writer(&mut body, item).with_context(|| "serializing PrefetchItem")?;
            body.push(b'\n');
        }

        let result: Result<PrefetchResponse> = async {
            let resp = self
                .send(
                    self.add_auth(
                        self.http
                            .post(&url)
                            .header("Content-Type", "application/x-ndjson")
                            .body(body),
                    ),
                    "prefetch",
                )
                .await?;
            if !resp.status().is_success() {
                // Deliberately not covered by the "any HTTP response counts
                // as responsive" rule, on any admission -- see "Prefetch
                // does not close the circuit": a write-time warm's non-2xx
                // must not hold a 503-ing cache's circuit closed.
                return Err(anyhow!("cache prefetch {url} status {}", resp.status()));
            }
            // Read as two explicit steps rather than via the combined
            // `resp.json()` helper: in reqwest 0.12.28 `json()` re-wraps
            // every error from its internal body read as `Kind::Decode`, so
            // `is_body()`/`is_decode()` can't distinguish a transport
            // failure from a parse failure on that path. See "What does not
            // feed the breaker as a failure".
            let bytes = match resp.bytes().await {
                Ok(bytes) => bytes,
                Err(e) => {
                    // A body/transport/timeout failure: a genuine liveness
                    // signal.
                    self.report_unresponsive("prefetch response body");
                    return Err(e).with_context(|| "reading prefetch response body");
                }
            };
            // The response arrived as a full, cheap 2xx; a parse failure
            // here is a protocol violation from our own cache, not evidence
            // the demand path is unresponsive -- reports nothing, exactly
            // like `get_full_stream`'s/`head_size`'s malformed-response arms.
            serde_json::from_slice::<PrefetchResponse>(&bytes)
                .with_context(|| "parsing prefetch response")
        }
        .await;

        if let Err(e) = &result {
            imetric!("range_cache_client_prefetch_error", "count", 1_u64);
            debug!("prefetch request to {url} failed: {e}");
        }
        result
    }

    /// Issue `POST /ranges` and reassemble the framed response body, keeping
    /// four failure kinds distinct instead of folding them into one:
    /// header-phase failure (`Send`, reusing `send`'s own `abandon_timeout`
    /// wrap and reporting), a non-2xx `Status`, and each `RangesReadError`
    /// body-failure kind (`Transport`/`Stalled`/`Truncated`). Only a `Send`
    /// failure or a body `Transport`/`Stalled` counts as a breaker failure; a
    /// non-2xx `Status` or a `Truncated` body means a full HTTP response
    /// arrived cheaply, so both report `record_responsive` per "Abandon vs.
    /// unresponsive". Success -- the framed body fully resolving -- is the
    /// single success report for the whole `get_ranges` operation, made here
    /// because `read_framed_ranges` exposes nothing to the caller until it
    /// fully resolves.
    pub async fn send_ranges(
        &self,
        location: &Path,
        ranges: &[Range<u64>],
    ) -> Result<Vec<Bytes>, RangesSendError> {
        let url = format!(
            "{}/ranges/{}",
            self.cache_base_url.trim_end_matches('/'),
            location.as_ref()
        );
        let ranges_json: Vec<[u64; 2]> = ranges.iter().map(|r| [r.start, r.end]).collect();
        let body = json!({ "ranges": ranges_json }).to_string();

        let resp = self
            .send(
                self.add_auth(
                    self.http
                        .post(&url)
                        .header("Content-Type", "application/json")
                        .body(body),
                ),
                "ranges request",
            )
            .await
            .map_err(RangesSendError::Send)?;

        if !resp.status().is_success() {
            let status = resp.status();
            self.report();
            return Err(RangesSendError::Status(status));
        }

        // Stream the length-prefixed multi-range body (see the server's
        // `frame_ranges_stream`) and reassemble each range's `Bytes` as its
        // chunks arrive, instead of buffering the whole response with
        // `.bytes()` into one contiguous allocation before any of it is
        // used — the response can now be arbitrarily large since the server
        // no longer caps total requested bytes. `pull_exact`'s per-chunk
        // read is wrapped in `stall_timeout`, deliberately tighter than the
        // client's `read_timeout` so this explicit wrap is the one that
        // fires on a real stall -- see "`ClientBuilder::read_timeout` on the
        // one client". `read_framed_ranges` is a plain `Future` (not a
        // `Stream`) that only resolves once every range has been read, so
        // nothing is ever exposed to the caller before completion.
        match read_framed_ranges(
            resp.bytes_stream().boxed(),
            ranges.len(),
            self.config.stall_timeout,
        )
        .await
        {
            Ok(results) => {
                self.report();
                Ok(results)
            }
            Err(RangesReadError::Transport(e)) => {
                self.report_unresponsive("ranges body");
                Err(RangesSendError::Body(RangesReadError::Transport(e)))
            }
            Err(RangesReadError::Stalled) => {
                self.report_unresponsive("ranges body");
                Err(RangesSendError::Body(RangesReadError::Stalled))
            }
            Err(RangesReadError::Truncated) => {
                // A truncated/garbled framing from our own cache is a
                // protocol violation (unexpected), not a liveness signal --
                // see "What does not feed the breaker as a failure".
                self.report();
                Err(RangesSendError::Body(RangesReadError::Truncated))
            }
        }
    }
}

#[async_trait]
impl ObjectPrefetch for CacheClientStore {
    async fn prefetch(&self, items: Vec<PrefetchItem>) -> Result<PrefetchResponse> {
        CacheClientStore::prefetch(self, items).await
    }
}

/// A state change worth reporting: emits its metric/log exactly once per
/// transition (not once per request), so an outage doesn't flood.
fn report_transition(t: Transition) {
    match t {
        Transition::None => {}
        Transition::Opened { cooldown } => {
            imetric!("range_cache_client_circuit_opened", "count", 1_u64);
            warn!("cache circuit opened, cooling down for {cooldown:?}");
        }
        Transition::Closed => {
            imetric!("range_cache_client_circuit_closed", "count", 1_u64);
            info!("cache circuit closed");
        }
    }
}

/// Report an unresponsive cache to `breaker` and emit the resulting
/// transition. Free-standing (rather than a `CacheClientStore` method) so
/// `full_stream_with_fallback` -- itself a free function, since its `'static`
/// stream can't borrow `&self` -- can report from its resume path, the one
/// place a free function needs to report unresponsive at all.
/// `CacheClientStore::report_unresponsive` is a thin wrapper over this.
fn report_unresponsive(what: &str, breaker: &CircuitBreaker) {
    imetric!("range_cache_client_unresponsive", "count", 1_u64);
    debug!("cache {what} unresponsive");
    report_transition(breaker.record_unresponsive());
}

/// Fall back to `direct` for a whole `get_opts` operation, bumping the
/// shared `range_cache_client_fallback` counter and timing
/// `range_cache_client_direct_ms`. Free-standing so `full_stream_with_fallback`
/// (a free function) can share it for its resume read; `CacheClientStore::
/// direct_get_opts` is a thin wrapper over this for every other call site.
async fn direct_get_opts_with_metrics(
    direct: &Arc<dyn ObjectStore>,
    location: &Path,
    options: GetOptions,
) -> object_store::Result<GetResult> {
    imetric!("range_cache_client_fallback", "count", 1_u64);
    let direct_start = Instant::now();
    let result = direct.get_opts(location, options).await;
    fmetric!(
        "range_cache_client_direct_ms",
        "ms",
        direct_start.elapsed().as_secs_f64() * 1000.0
    );
    result
}

/// Parse a `Content-Range: bytes {start}-{end}/{size}` response header,
/// returning the actual byte range served (`start..end+1`) and the full
/// object size. Returns `None` when the header is absent or not in the
/// expected form (e.g. the unsatisfiable `bytes */size` form, or an
/// unparseable value).
fn parse_content_range(headers: &reqwest::header::HeaderMap) -> Option<(Range<u64>, u64)> {
    let value = headers.get(reqwest::header::CONTENT_RANGE)?.to_str().ok()?;
    let value = value.strip_prefix("bytes ")?;
    let (span, size) = value.split_once('/')?;
    let size: u64 = size.trim().parse().ok()?;
    let (start, end) = span.split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    let end: u64 = end.trim().parse().ok()?;
    Some((start..end.saturating_add(1), size))
}

impl std::fmt::Display for CacheClientStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CacheClientStore({})", self.cache_base_url)
    }
}

/// Build a streaming `GetResult` from an already-built byte stream (see
/// `full_stream_with_fallback`), so the object/range is delivered in chunks
/// rather than buffered whole. `range` is the slice actually being streamed
/// (`0..object_size` for an unranged GET) and `object_size` is the full
/// object size, per the `ObjectMeta` contract.
fn stream_get_result(
    location: &Path,
    body: BoxStream<'static, object_store::Result<Bytes>>,
    range: Range<u64>,
    object_size: u64,
) -> GetResult {
    let meta = ObjectMeta {
        location: location.clone(),
        last_modified: chrono::Utc::now(),
        size: object_size,
        e_tag: None,
        version: None,
    };
    GetResult {
        payload: GetResultPayload::Stream(body),
        meta,
        range,
        attributes: Attributes::default(),
    }
}

/// Wrap the raw byte stream for a GET (full or ranged) so any stream error --
/// or the stream ending cleanly short of the requested range -- transparently
/// resumes the remainder from `direct` at the byte offset already delivered,
/// instead of surfacing the cache's own error to the consumer or silently
/// truncating. See "The fix: resume from the delivered offset".
///
/// `resolved_range` is the absolute byte range this stream is expected to
/// deliver: `served_range` from `parse_content_range` on the ranged path, or
/// `0..size` from `Content-Length` on the full path -- resolved by the caller
/// before this helper is ever invoked.
///
/// This is also the single place the whole `get_opts` main-path operation
/// reports its outcome to the breaker (see "One outcome per logical
/// operation"): `record_responsive` when the stream ends -- with or without
/// error -- with every requested byte already delivered (no resume occurs),
/// or `record_unresponsive` (via the resume path only) when a resume occurs.
/// Never both.
fn full_stream_with_fallback(
    direct: Arc<dyn ObjectStore>,
    breaker: Arc<CircuitBreaker>,
    location: Path,
    options: GetOptions,
    resolved_range: Range<u64>,
    mut first: BoxStream<'static, object_store::Result<Bytes>>,
) -> BoxStream<'static, object_store::Result<Bytes>> {
    gen_stream! {
        let mut bytes_yielded: u64 = 0;
        let mut last_err: Option<String> = None;
        loop {
            match first.next().await {
                Some(Ok(chunk)) => {
                    bytes_yielded += chunk.len() as u64;
                    yield Ok(chunk);
                }
                Some(Err(e)) => {
                    last_err = Some(e.to_string());
                    break;
                }
                None => break,
            }
        }

        let resume_start = resolved_range.start.saturating_add(bytes_yielded);
        if resume_start >= resolved_range.end {
            // Every requested byte was already delivered (or the cache
            // over-delivered past its own declared range): the operation
            // succeeded even if the terminal event was an `Err`. Never issue
            // an empty or inverted `direct.get_opts(Bounded(x..x))` call --
            // `object_store` rejects that as a hard error. This is the
            // single success report for this operation.
            report_transition(breaker.record_responsive());
            return;
        }

        // Bytes still owed: resume the remainder from `direct`, treating a
        // clean end (`None`) with bytes owed identically to an `Err`.
        let remainder = resume_start..resolved_range.end;
        imetric!("range_cache_client_stream_resumed", "count", 1_u64);
        match &last_err {
            Some(e) => debug!(
                "cache GET stream for {location} failed with {} bytes still owed, \
                 resuming {}..{} from direct: {e}",
                remainder.end - remainder.start,
                remainder.start,
                remainder.end
            ),
            None => debug!(
                "cache GET stream for {location} ended short by {} bytes, \
                 resuming {}..{} from direct",
                remainder.end - remainder.start,
                remainder.start,
                remainder.end
            ),
        }
        // The only report a resumed operation makes -- see "One outcome per
        // logical operation".
        report_unresponsive("stream", &breaker);

        let mut resumed_options = options;
        resumed_options.range = Some(GetRange::Bounded(remainder));
        match direct_get_opts_with_metrics(&direct, &location, resumed_options).await {
            Ok(result) => {
                let mut body = result.into_stream();
                while let Some(item) = body.next().await {
                    yield item;
                }
            }
            // A direct-store failure is a failure the invariant permits --
            // it constrains cache failure modes, not the direct store's own.
            Err(direct_err) => yield Err(direct_err),
        }
    }
    .boxed()
}

/// Build a `GetResult` for an already-buffered, small ranged payload (used
/// only for the zero-length-range edge cases in `get_range_stream` and the
/// HEAD-only path in `get_opts`, where there is nothing worth streaming).
/// `range` is the slice actually returned while `object_size` is the full
/// object size, per the `ObjectMeta` contract.
fn build_get_result(
    location: &Path,
    data: Bytes,
    range: Range<u64>,
    object_size: u64,
) -> GetResult {
    let meta = ObjectMeta {
        location: location.clone(),
        last_modified: chrono::Utc::now(),
        size: object_size,
        e_tag: None,
        version: None,
    };
    let payload = GetResultPayload::Stream(Box::pin(stream::once(async move { Ok(data) })));
    GetResult {
        payload,
        meta,
        range,
        attributes: Attributes::default(),
    }
}

/// Why reading a streamed `/ranges` response body failed. Public API surface
/// (re-exported from `lib.rs`) so the cross-crate integration test can
/// assert on the exact failure kind directly (via `send_ranges`), rather than
/// inspecting logs or metrics.
#[derive(Debug, thiserror::Error)]
pub enum RangesReadError {
    #[error("transport error reading ranges response: {0}")]
    Transport(#[from] reqwest::Error),
    /// No data arrived on the response body for a full `stall_timeout`
    /// window -- the per-chunk bound `pull_exact` wraps around each
    /// `stream.next()`, deliberately tighter than the client's `read_timeout`
    /// margin so this is the classification that actually fires on a real
    /// stall. See "`ClientBuilder::read_timeout` on the one client".
    #[error("ranges response body stalled (no data within the stall budget)")]
    Stalled,
    /// The framed body ended before every declared frame was fully read: a
    /// protocol violation from our own cache, not a liveness signal.
    #[error("truncated ranges response")]
    Truncated,
}

/// Why `send_ranges` failed, keeping the header-phase failure, a non-2xx
/// status, and each `RangesReadError` body-failure kind distinct rather than
/// folding them into one. Public API surface (re-exported from `lib.rs`) for
/// the same reason as `RangesReadError`.
#[derive(Debug, thiserror::Error)]
pub enum RangesSendError {
    #[error("sending ranges request to cache: {0}")]
    Send(anyhow::Error),
    #[error("cache ranges request status {0}")]
    Status(reqwest::StatusCode),
    #[error(transparent)]
    Body(#[from] RangesReadError),
}

/// Reassemble `count` length-prefixed frames (an 8-byte little-endian length
/// followed by that many bytes, repeated once per requested range — see the
/// server's `frame_ranges_stream`) from a streaming multi-range response
/// body, mirroring `RangeCache::get_ranges`'s pending-chunk reassembly on the
/// server side (`range_cache.rs`) instead of buffering the whole response
/// with `resp.bytes().await` before parsing it.
async fn read_framed_ranges(
    mut stream: BoxStream<'static, reqwest::Result<Bytes>>,
    count: usize,
    stall_timeout: Duration,
) -> Result<Vec<Bytes>, RangesReadError> {
    let mut pending: Option<Bytes> = None;
    let mut results = Vec::with_capacity(count);
    for _ in 0..count {
        let mut prefix = pull_exact(&mut stream, &mut pending, 8, stall_timeout).await?;
        let len = prefix.get_u64_le() as usize;
        let data = pull_exact(&mut stream, &mut pending, len, stall_timeout).await?;
        results.push(data);
    }
    Ok(results)
}

/// Pull exactly `need` bytes out of `stream`, using `pending` as a one-chunk
/// lookahead so a frame that straddles a network chunk boundary is
/// reassembled correctly (mirrors `RangeCache::get_ranges`'s reassembly loop
/// in `range_cache.rs`). Each `stream.next()` await is bounded by
/// `stall_timeout` -- a per-chunk bound, since the whole body has no size cap
/// (constraint (b) in the design doc).
async fn pull_exact(
    stream: &mut BoxStream<'static, reqwest::Result<Bytes>>,
    pending: &mut Option<Bytes>,
    need: usize,
    stall_timeout: Duration,
) -> Result<Bytes, RangesReadError> {
    let mut collected = BytesMut::with_capacity(need);
    while collected.len() < need {
        let chunk = match pending.take() {
            Some(c) => c,
            None => match tokio::time::timeout(stall_timeout, stream.next()).await {
                Ok(Some(Ok(c))) => c,
                Ok(Some(Err(e))) => return Err(e.into()),
                Ok(None) => return Err(RangesReadError::Truncated),
                Err(_) => return Err(RangesReadError::Stalled),
            },
        };
        let remaining = need - collected.len();
        if chunk.len() > remaining {
            collected.put_slice(&chunk[..remaining]);
            *pending = Some(chunk.slice(remaining..));
        } else {
            collected.put_slice(&chunk);
        }
    }
    Ok(collected.freeze())
}

#[async_trait]
impl ObjectStore for CacheClientStore {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> object_store::Result<PutResult> {
        self.direct.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        self.direct.put_multipart_opts(location, opts).await
    }

    async fn get_opts(
        &self,
        location: &Path,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        // The cache HTTP protocol can't convey conditional/version preconditions,
        // so any such request must go straight to the direct store to preserve
        // the expected 412/304 semantics. Never touches the cache, so no
        // fallback bookkeeping.
        if options.if_match.is_some()
            || options.if_none_match.is_some()
            || options.if_modified_since.is_some()
            || options.if_unmodified_since.is_some()
            || options.version.is_some()
        {
            return self.direct.get_opts(location, options).await;
        }

        // One admission gate per public entry point (see "The breaker"): a
        // `Probe` admission behaves exactly like `Allow` below, so only
        // `Bypass` needs handling here.
        if matches!(self.breaker.admit(), Admission::Bypass) {
            imetric!("range_cache_client_circuit_bypassed", "count", 1_u64);
            debug!("cache circuit open, reading {location} direct");
            return self.direct_get_opts(location, options).await;
        }

        // A head-only request needs metadata, not the body: return an empty
        // payload with the true object size instead of streaming the object.
        if options.head {
            let result: Result<GetResult> = match self.head_size(location).await {
                Ok(size) => {
                    // `head_size`'s completion *is* the whole operation
                    // here: the single success report.
                    self.report();
                    Ok(build_get_result(location, Bytes::new(), 0..0, size))
                }
                Err(e) => Err(e),
            };
            return match result {
                Ok(r) => Ok(r),
                Err(e) => {
                    // Falling back to the direct store is a by-design graceful
                    // degradation path (cache restarting/unreachable), not an
                    // error: keep it at debug and let the fallback metric (which
                    // is what dashboards alert on) carry the signal, so a cache
                    // outage doesn't flood logs with one warning per read.
                    debug!("cache miss for {location} (head), falling back to direct: {e}");
                    self.direct_get_opts(location, options).await
                }
            };
        }

        let result: Result<GetResult> = match &options.range {
            None => self.get_full_stream(location, options.clone()).await,
            // Issue the range GET and stream the body; the actual served
            // range and the full object size come from the 206's
            // `Content-Range` header (see `get_range_stream`), avoiding a
            // preceding HEAD round-trip in the common case.
            Some(GetRange::Bounded(r)) => {
                self.get_range_stream(location, r.start, Some(r.end), options.clone())
                    .await
            }
            // Open-ended range: the server resolves `-` against the true
            // object size, returned to us via `Content-Range`.
            Some(GetRange::Offset(offset)) => {
                self.get_range_stream(location, *offset, None, options.clone())
                    .await
            }
            // Suffix reads need the object size up front to compute the start
            // offset, since the cache server's Range parser does not accept the
            // `bytes=-N` suffix form. A HEAD is unavoidable here. This is a
            // genuine intermediate step -- `get_range_stream`'s eventual
            // stream is what completes the operation -- so no success report
            // is made at this call site; `head_size`'s own failure-arm
            // reports still fire as usual.
            Some(GetRange::Suffix(suffix)) => match self.head_size(location).await {
                Ok(size) => {
                    let start = size.saturating_sub(*suffix);
                    self.get_range_stream(location, start, Some(size), options.clone())
                        .await
                }
                Err(e) => Err(e),
            },
        };

        match result {
            Ok(r) => Ok(r),
            Err(e) => {
                debug!("cache miss for {location}, falling back to direct: {e}");
                self.direct_get_opts(location, options).await
            }
        }
    }

    async fn get_ranges(
        &self,
        location: &Path,
        ranges: &[Range<u64>],
    ) -> object_store::Result<Vec<Bytes>> {
        if ranges.is_empty() {
            return Ok(vec![]);
        }

        if matches!(self.breaker.admit(), Admission::Bypass) {
            imetric!("range_cache_client_circuit_bypassed", "count", 1_u64);
            debug!("cache circuit open, reading ranges for {location} direct");
            return self.direct_get_ranges(location, ranges).await;
        }

        let round_trip_start = Instant::now();
        match self.send_ranges(location, ranges).await {
            Ok(results) => {
                // Distinct from `range_cache_client_roundtrip_ms`: that metric
                // covers the streaming GET paths and is measured at
                // time-to-headers (before any body bytes are read), while this
                // path buffers the full framed response body before emitting,
                // so it measures time-to-full-body. Keeping them under
                // separate names avoids conflating two different quantities
                // in one distribution.
                fmetric!(
                    "range_cache_client_ranges_ms",
                    "ms",
                    round_trip_start.elapsed().as_secs_f64() * 1000.0
                );
                Ok(results)
            }
            Err(RangesSendError::Send(e)) => {
                debug!("cache ranges request for {location} failed: {e}, falling back to direct");
                self.direct_get_ranges(location, ranges).await
            }
            Err(RangesSendError::Status(status)) => {
                debug!("cache ranges for {location} status {status}, falling back to direct");
                self.direct_get_ranges(location, ranges).await
            }
            Err(RangesSendError::Body(RangesReadError::Transport(e))) => {
                debug!(
                    "reading ranges response for {location} failed: {e}, falling back to direct"
                );
                self.direct_get_ranges(location, ranges).await
            }
            Err(RangesSendError::Body(RangesReadError::Stalled)) => {
                debug!("ranges response for {location} stalled, falling back to direct");
                self.direct_get_ranges(location, ranges).await
            }
            Err(RangesSendError::Body(RangesReadError::Truncated)) => {
                // A truncated/garbled framing from our own cache is a protocol
                // violation (unexpected), unlike the clean miss/outage paths
                // above — keep this at warn.
                warn!("truncated ranges response for {location}, falling back to direct");
                self.direct_get_ranges(location, ranges).await
            }
        }
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, object_store::Result<Path>>,
    ) -> BoxStream<'static, object_store::Result<Path>> {
        self.direct.delete_stream(locations)
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        self.direct.list(prefix)
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> object_store::Result<ListResult> {
        self.direct.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &Path,
        to: &Path,
        options: CopyOptions,
    ) -> object_store::Result<()> {
        self.direct.copy_opts(from, to, options).await
    }
}
