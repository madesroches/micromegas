use anyhow::{Context, Result, anyhow};
use async_stream::stream as gen_stream;
use async_trait::async_trait;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures::stream::TryStreamExt;
use futures::stream::{self, BoxStream, StreamExt};
use micromegas_tracing::prelude::*;
use object_store::{
    Attributes, CopyOptions, GetOptions, GetResult, GetResultPayload, ListResult, MultipartUpload,
    ObjectMeta, ObjectStore, PutMultipartOptions, PutOptions, PutPayload, PutResult, path::Path,
};
use reqwest::Client;
use serde_json::json;
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::prefetch::{ObjectPrefetch, PrefetchItem, PrefetchResponse};

/// Fail fast if the cache server can't be reached, so reads fall back to the
/// direct store instead of stalling on a hung connection.
const CACHE_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Overall per-request timeout; a slow cache surfaces as an error and triggers
/// the existing fallback-to-direct path.
const CACHE_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub struct CacheClientStore {
    http: Client,
    cache_base_url: String,
    api_key: Option<String>,
    direct: Arc<dyn ObjectStore>,
}

impl CacheClientStore {
    pub fn new(
        cache_base_url: String,
        api_key: Option<String>,
        direct: Arc<dyn ObjectStore>,
    ) -> Self {
        let http = Client::builder()
            .connect_timeout(CACHE_CONNECT_TIMEOUT)
            .timeout(CACHE_REQUEST_TIMEOUT)
            .build()
            .expect("building reqwest client");
        Self {
            http,
            cache_base_url,
            api_key,
            direct,
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
    /// so a stream error before the first chunk reaches the consumer can fall
    /// back to `self.direct` for the same range, via `full_stream_with_fallback`.
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
            .add_auth(self.http.get(&url))
            .header("Range", range_header)
            .send()
            .await
            .with_context(|| "sending GET to cache")?;
        if !resp.status().is_success() {
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
            let body =
                full_stream_with_fallback(self.direct.clone(), location.clone(), options, raw);
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
        let size = self.head_size(location).await?;
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
    /// before the first chunk reaches the consumer transparently falls back
    /// to `self.direct`, at the `GetResult` level instead of the
    /// whole-response level, since this path streams rather than buffers
    /// (the ranged path, `get_range_stream`, uses the same helper for the
    /// same reason).
    async fn get_full_stream(&self, location: &Path, options: GetOptions) -> Result<GetResult> {
        let round_trip_start = Instant::now();
        let url = self.obj_url(location);
        let resp = self
            .add_auth(self.http.get(&url))
            .send()
            .await
            .with_context(|| "sending GET to cache")?;
        if !resp.status().is_success() {
            return Err(anyhow!("cache GET {url} status {}", resp.status()));
        }
        let size = resp
            .content_length()
            .ok_or_else(|| anyhow!("missing Content-Length in GET response"))?;
        let raw = resp
            .bytes_stream()
            .map_err(|e| object_store::Error::Generic {
                store: "CacheClientStore",
                source: Box::new(e),
            })
            .boxed();
        let body = full_stream_with_fallback(self.direct.clone(), location.clone(), options, raw);
        // Measured at time-to-usable-stream, before any body bytes are read
        // (see the matching comment in `get_range_stream`).
        fmetric!(
            "range_cache_client_roundtrip_ms",
            "ms",
            round_trip_start.elapsed().as_secs_f64() * 1000.0
        );
        Ok(stream_get_result(location, body, 0..size, size))
    }

    async fn head_size(&self, location: &Path) -> Result<u64> {
        let url = self.obj_url(location);
        let resp = self
            .add_auth(self.http.head(&url))
            .send()
            .await
            .with_context(|| "sending HEAD to cache")?;
        if !resp.status().is_success() {
            return Err(anyhow!("cache HEAD {url} status {}", resp.status()));
        }
        resp.headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| anyhow!("missing Content-Length in HEAD response"))
    }

    /// POST a batch of keys to warm at the cache server's prefetch priority.
    /// Best-effort: there is no demand read to fall back to, so callers should
    /// treat an `Err` as "the warm didn't happen" and move on rather than
    /// retrying inline.
    pub async fn prefetch(&self, items: Vec<PrefetchItem>) -> Result<PrefetchResponse> {
        let url = format!("{}/prefetch", self.cache_base_url.trim_end_matches('/'));
        let mut body = Vec::new();
        for item in &items {
            serde_json::to_writer(&mut body, item).with_context(|| "serializing PrefetchItem")?;
            body.push(b'\n');
        }

        let result: Result<PrefetchResponse> = async {
            let resp = self
                .add_auth(
                    self.http
                        .post(&url)
                        .header("Content-Type", "application/x-ndjson")
                        .body(body),
                )
                .send()
                .await
                .with_context(|| "sending POST to cache prefetch")?;
            if !resp.status().is_success() {
                return Err(anyhow!("cache prefetch {url} status {}", resp.status()));
            }
            resp.json::<PrefetchResponse>()
                .await
                .with_context(|| "reading prefetch response")
        }
        .await;

        if let Err(e) = &result {
            imetric!("range_cache_client_prefetch_error", "count", 1_u64);
            debug!("prefetch request to {url} failed: {e}");
        }
        result
    }
}

#[async_trait]
impl ObjectPrefetch for CacheClientStore {
    async fn prefetch(&self, items: Vec<PrefetchItem>) -> Result<PrefetchResponse> {
        CacheClientStore::prefetch(self, items).await
    }
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

/// Wrap the raw byte stream for a GET (full or ranged) so a stream error
/// *before* any bytes reach the consumer transparently falls back to
/// `direct`: zero bytes have been yielded downstream yet, so retrying
/// against the origin can't re-deliver a duplicate prefix. Once the first
/// chunk has been yielded downstream, a later error simply ends the stream —
/// retrying at that point would re-emit already-delivered bytes from the
/// start, which is unsound.
fn full_stream_with_fallback(
    direct: Arc<dyn ObjectStore>,
    location: Path,
    options: GetOptions,
    mut first: BoxStream<'static, object_store::Result<Bytes>>,
) -> BoxStream<'static, object_store::Result<Bytes>> {
    gen_stream! {
        match first.next().await {
            None => {}
            Some(Ok(chunk)) => {
                yield Ok(chunk);
                while let Some(item) = first.next().await {
                    yield item;
                }
            }
            Some(Err(e)) => {
                imetric!("range_cache_client_fallback", "count", 1_u64);
                debug!(
                    "cache GET stream for {location} failed before the first chunk, \
                     falling back to direct: {e}"
                );
                let direct_start = Instant::now();
                let direct_result = direct.get_opts(&location, options).await;
                fmetric!(
                    "range_cache_client_direct_ms",
                    "ms",
                    direct_start.elapsed().as_secs_f64() * 1000.0
                );
                match direct_result {
                    Ok(result) => {
                        let mut body = result.into_stream();
                        while let Some(item) = body.next().await {
                            yield item;
                        }
                    }
                    Err(direct_err) => yield Err(direct_err),
                }
            }
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

/// Why reading a streamed `/ranges` response body failed: distinguishing the
/// two cases lets `get_ranges` keep logging them the way the previous
/// buffered implementation did (a transport error at `debug`, a
/// protocol-level truncation at `warn`).
enum RangesReadError {
    Transport(reqwest::Error),
    Truncated,
}

impl From<reqwest::Error> for RangesReadError {
    fn from(e: reqwest::Error) -> Self {
        RangesReadError::Transport(e)
    }
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
) -> Result<Vec<Bytes>, RangesReadError> {
    let mut pending: Option<Bytes> = None;
    let mut results = Vec::with_capacity(count);
    for _ in 0..count {
        let mut prefix = pull_exact(&mut stream, &mut pending, 8).await?;
        let len = prefix.get_u64_le() as usize;
        let data = pull_exact(&mut stream, &mut pending, len).await?;
        results.push(data);
    }
    Ok(results)
}

/// Pull exactly `need` bytes out of `stream`, using `pending` as a one-chunk
/// lookahead so a frame that straddles a network chunk boundary is
/// reassembled correctly (mirrors `RangeCache::get_ranges`'s reassembly loop
/// in `range_cache.rs`).
async fn pull_exact(
    stream: &mut BoxStream<'static, reqwest::Result<Bytes>>,
    pending: &mut Option<Bytes>,
    need: usize,
) -> Result<Bytes, RangesReadError> {
    let mut collected = BytesMut::with_capacity(need);
    while collected.len() < need {
        let chunk = match pending.take() {
            Some(c) => c,
            None => match stream.next().await {
                Some(Ok(c)) => c,
                Some(Err(e)) => return Err(e.into()),
                None => return Err(RangesReadError::Truncated),
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
        use object_store::GetRange;

        // The cache HTTP protocol can't convey conditional/version preconditions,
        // so any such request must go straight to the direct store to preserve
        // the expected 412/304 semantics.
        if options.if_match.is_some()
            || options.if_none_match.is_some()
            || options.if_modified_since.is_some()
            || options.if_unmodified_since.is_some()
            || options.version.is_some()
        {
            return self.direct.get_opts(location, options).await;
        }

        // A head-only request needs metadata, not the body: return an empty
        // payload with the true object size instead of streaming the object.
        if options.head {
            let result: Result<GetResult> = match self.head_size(location).await {
                Ok(size) => Ok(build_get_result(location, Bytes::new(), 0..0, size)),
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
                    imetric!("range_cache_client_fallback", "count", 1_u64);
                    debug!("cache miss for {location} (head), falling back to direct: {e}");
                    let direct_start = Instant::now();
                    let direct_result = self.direct.get_opts(location, options).await;
                    fmetric!(
                        "range_cache_client_direct_ms",
                        "ms",
                        direct_start.elapsed().as_secs_f64() * 1000.0
                    );
                    direct_result
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
            // `bytes=-N` suffix form. A HEAD is unavoidable here.
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
                imetric!("range_cache_client_fallback", "count", 1_u64);
                debug!("cache miss for {location}, falling back to direct: {e}");
                let direct_start = Instant::now();
                let direct_result = self.direct.get_opts(location, options).await;
                fmetric!(
                    "range_cache_client_direct_ms",
                    "ms",
                    direct_start.elapsed().as_secs_f64() * 1000.0
                );
                direct_result
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

        let round_trip_start = Instant::now();
        let url = format!(
            "{}/ranges/{}",
            self.cache_base_url.trim_end_matches('/'),
            location.as_ref()
        );
        let ranges_json: Vec<[u64; 2]> = ranges.iter().map(|r| [r.start, r.end]).collect();
        let body = json!({ "ranges": ranges_json }).to_string();

        let resp = match self
            .add_auth(
                self.http
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .body(body),
            )
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                imetric!("range_cache_client_fallback", "count", 1_u64);
                debug!("cache ranges {url} status {}, falling back", r.status());
                let direct_start = Instant::now();
                let result = self.direct.get_ranges(location, ranges).await;
                fmetric!(
                    "range_cache_client_direct_ms",
                    "ms",
                    direct_start.elapsed().as_secs_f64() * 1000.0
                );
                return result;
            }
            Err(e) => {
                imetric!("range_cache_client_fallback", "count", 1_u64);
                debug!("cache ranges request failed: {e}, falling back to direct");
                let direct_start = Instant::now();
                let result = self.direct.get_ranges(location, ranges).await;
                fmetric!(
                    "range_cache_client_direct_ms",
                    "ms",
                    direct_start.elapsed().as_secs_f64() * 1000.0
                );
                return result;
            }
        };

        // Stream the length-prefixed multi-range body (see the server's
        // `frame_ranges_stream`) and reassemble each range's `Bytes` as its
        // chunks arrive, instead of buffering the whole response with
        // `.bytes()` into one contiguous allocation before any of it is
        // used — the response can now be arbitrarily large since the server
        // no longer caps total requested bytes. `read_framed_ranges` is a
        // plain `Future` (not a `Stream`) that only resolves once every
        // range has been read, so nothing is ever exposed to the caller
        // before completion, and any read failure can still safely fall back
        // to `self.direct` here, exactly like the previous buffered
        // implementation.
        match read_framed_ranges(resp.bytes_stream().boxed(), ranges.len()).await {
            Ok(results) => {
                // Distinct from `range_cache_client_roundtrip_ms`: that metric
                // covers the streaming GET paths and is measured at
                // time-to-headers (before any body bytes are read), while this
                // path buffers the full framed response body via
                // `read_framed_ranges` before emitting, so it measures
                // time-to-full-body. Keeping them under separate names avoids
                // conflating two different quantities in one distribution.
                fmetric!(
                    "range_cache_client_ranges_ms",
                    "ms",
                    round_trip_start.elapsed().as_secs_f64() * 1000.0
                );
                Ok(results)
            }
            Err(RangesReadError::Transport(e)) => {
                imetric!("range_cache_client_fallback", "count", 1_u64);
                debug!("reading ranges response failed: {e}, falling back to direct");
                let direct_start = Instant::now();
                let result = self.direct.get_ranges(location, ranges).await;
                fmetric!(
                    "range_cache_client_direct_ms",
                    "ms",
                    direct_start.elapsed().as_secs_f64() * 1000.0
                );
                result
            }
            Err(RangesReadError::Truncated) => {
                // A truncated/garbled framing from our own cache is a protocol
                // violation (unexpected), unlike the clean miss/outage paths
                // above — keep this at warn.
                imetric!("range_cache_client_fallback", "count", 1_u64);
                warn!("truncated ranges response, falling back to direct");
                let direct_start = Instant::now();
                let result = self.direct.get_ranges(location, ranges).await;
                fmetric!(
                    "range_cache_client_direct_ms",
                    "ms",
                    direct_start.elapsed().as_secs_f64() * 1000.0
                );
                result
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
