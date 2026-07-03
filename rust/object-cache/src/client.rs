use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use bytes::{Buf, Bytes};
use futures::stream::TryStreamExt;
use futures::stream::{self, BoxStream};
use micromegas_tracing::prelude::*;
use object_store::{
    Attributes, CopyOptions, GetOptions, GetResult, GetResultPayload, ListResult, MultipartUpload,
    ObjectMeta, ObjectStore, PutMultipartOptions, PutOptions, PutPayload, PutResult, path::Path,
};
use reqwest::Client;
use serde_json::json;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use crate::prefetch::{ObjectPrefetch, PrefetchItem, PrefetchRequest, PrefetchResponse};

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

    /// Issue a range GET and return the body bytes along with the full object
    /// size parsed from the `Content-Range: bytes {start}-{end}/{size}` response
    /// header. The 206 response already carries the full size, so callers can
    /// avoid a separate HEAD round-trip. When the header is absent or
    /// unparseable, the size is returned as `None` and the caller should fall
    /// back to a `head_size` lookup.
    ///
    /// An open-ended `end` (`None`) requests `bytes={start}-`, i.e. from `start`
    /// to the end of the object, which the cache server resolves against the
    /// true object size.
    async fn get_range_bytes(
        &self,
        location: &Path,
        start: u64,
        end: Option<u64>,
    ) -> Result<(Bytes, Option<u64>)> {
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
        let object_size = parse_content_range_size(resp.headers());
        let data = resp.bytes().await.with_context(|| "reading GET response")?;
        Ok((data, object_size))
    }

    /// Resolve the full object size for a ranged read, preferring the size from
    /// the GET's `Content-Range` header and falling back to a HEAD only when the
    /// header was missing or unparseable.
    async fn resolve_size(&self, location: &Path, from_range: Option<u64>) -> Result<u64> {
        match from_range {
            Some(size) => Ok(size),
            None => self.head_size(location).await,
        }
    }

    /// Issue an unranged GET and build a streaming `GetResult`, mapping the
    /// response body to `GetResultPayload::Stream` so the whole object is never
    /// buffered in memory (matching how the direct store streams the body). The
    /// object size comes from the `Content-Length` header, which is required to
    /// populate `meta.size` and the `0..size` range without reading the body.
    async fn get_full_stream(&self, location: &Path) -> Result<GetResult> {
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
        Ok(stream_get_result(location, resp, size))
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
        let body = serde_json::to_vec(&PrefetchRequest { keys: items })
            .with_context(|| "serializing PrefetchRequest")?;

        let result: Result<PrefetchResponse> = async {
            let resp = self
                .add_auth(
                    self.http
                        .post(&url)
                        .header("Content-Type", "application/json")
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

/// Parse the full object size from a `Content-Range: bytes {start}-{end}/{size}`
/// header (the suffix after `/`). Returns `None` when the header is absent or
/// not in the expected form (e.g. `bytes */size` or an unparseable value).
fn parse_content_range_size(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.rsplit('/').next())
        .and_then(|size| size.trim().parse::<u64>().ok())
}

impl std::fmt::Display for CacheClientStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CacheClientStore({})", self.cache_base_url)
    }
}

/// Build a streaming `GetResult` for an unranged GET, wrapping the reqwest
/// response body in `GetResultPayload::Stream` so the object is delivered in
/// chunks rather than buffered whole. `object_size` (from `Content-Length`)
/// populates `meta.size` and the `0..object_size` range.
fn stream_get_result(location: &Path, resp: reqwest::Response, object_size: u64) -> GetResult {
    let meta = ObjectMeta {
        location: location.clone(),
        last_modified: chrono::Utc::now(),
        size: object_size,
        e_tag: None,
        version: None,
    };
    let body = resp
        .bytes_stream()
        .map_err(|e| object_store::Error::Generic {
            store: "CacheClientStore",
            source: Box::new(e),
        });
    GetResult {
        payload: GetResultPayload::Stream(Box::pin(body)),
        meta,
        range: 0..object_size,
        attributes: Attributes::default(),
    }
}

/// Build a `GetResult` for a ranged GET. `range` is the slice actually returned
/// while `object_size` is the full object size (per the `ObjectMeta` contract).
///
/// When no bytes are returned (e.g. a 0-byte object, or a range starting at or
/// beyond EOF), the requested `range` may lie outside the object; report an
/// empty `0..0` range so it always matches `data.len()` and stays within
/// `object_size`, satisfying the `GetResult` invariant.
fn ranged_get_result(
    location: &Path,
    data: Bytes,
    range: Range<u64>,
    object_size: u64,
) -> GetResult {
    let range = if data.is_empty() { 0..0 } else { range };
    build_get_result(location, data, range, object_size)
}

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
                Ok(size) => Ok(ranged_get_result(location, Bytes::new(), 0..0, size)),
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
                    self.direct.get_opts(location, options).await
                }
            };
        }

        let result: Result<GetResult> = match &options.range {
            None => self.get_full_stream(location).await,
            // Issue the range GET first and read the full object size from the
            // 206 `Content-Range` header, avoiding a preceding HEAD round-trip.
            // `resolve_size` only falls back to a HEAD if that header was absent
            // or unparseable.
            Some(GetRange::Bounded(r)) => {
                match self.get_range_bytes(location, r.start, Some(r.end)).await {
                    Ok((data, size_hint)) => match self.resolve_size(location, size_hint).await {
                        Ok(size) => {
                            let len = data.len() as u64;
                            Ok(ranged_get_result(
                                location,
                                data,
                                r.start..r.start + len,
                                size,
                            ))
                        }
                        Err(e) => Err(e),
                    },
                    Err(e) => Err(e),
                }
            }
            Some(GetRange::Offset(offset)) => {
                let start = *offset;
                // Open-ended range: the server resolves `-` against the true
                // object size, returned to us via `Content-Range`.
                match self.get_range_bytes(location, start, None).await {
                    Ok((data, size_hint)) => match self.resolve_size(location, size_hint).await {
                        Ok(size) => {
                            let len = data.len() as u64;
                            Ok(ranged_get_result(location, data, start..start + len, size))
                        }
                        Err(e) => Err(e),
                    },
                    Err(e) => Err(e),
                }
            }
            // Suffix reads need the object size up front to compute the start
            // offset, since the cache server's Range parser does not accept the
            // `bytes=-N` suffix form. A HEAD is unavoidable here.
            Some(GetRange::Suffix(suffix)) => match self.head_size(location).await {
                Ok(size) => {
                    let start = size.saturating_sub(*suffix);
                    self.get_range_bytes(location, start, Some(size))
                        .await
                        .map(|(data, _)| {
                            let len = data.len() as u64;
                            ranged_get_result(location, data, start..start + len, size)
                        })
                }
                Err(e) => Err(e),
            },
        };

        match result {
            Ok(r) => Ok(r),
            Err(e) => {
                imetric!("range_cache_client_fallback", "count", 1_u64);
                debug!("cache miss for {location}, falling back to direct: {e}");
                self.direct.get_opts(location, options).await
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
                return self.direct.get_ranges(location, ranges).await;
            }
            Err(e) => {
                imetric!("range_cache_client_fallback", "count", 1_u64);
                debug!("cache ranges request failed: {e}, falling back to direct");
                return self.direct.get_ranges(location, ranges).await;
            }
        };

        let mut data = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => {
                imetric!("range_cache_client_fallback", "count", 1_u64);
                debug!("reading ranges response failed: {e}, falling back to direct");
                return self.direct.get_ranges(location, ranges).await;
            }
        };

        let mut results = Vec::with_capacity(ranges.len());
        for _ in 0..ranges.len() {
            if data.remaining() < 8 {
                // A truncated/garbled framing from our own cache is a protocol
                // violation (unexpected), unlike the clean miss/outage paths
                // above — keep this at warn.
                imetric!("range_cache_client_fallback", "count", 1_u64);
                warn!("truncated ranges response, falling back to direct");
                return self.direct.get_ranges(location, ranges).await;
            }
            let len = data.get_u64_le() as usize;
            if data.remaining() < len {
                imetric!("range_cache_client_fallback", "count", 1_u64);
                warn!("truncated ranges response body, falling back to direct");
                return self.direct.get_ranges(location, ranges).await;
            }
            results.push(data.copy_to_bytes(len));
        }

        Ok(results)
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
