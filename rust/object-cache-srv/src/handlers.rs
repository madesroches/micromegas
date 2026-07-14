use crate::app_state::AppState;
use crate::validation::{is_not_found, parse_range_header};
use async_stream::{stream as gen_stream, try_stream};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use bytes::{BufMut, Bytes, BytesMut};
use futures::stream::BoxStream;
use futures::{Stream, StreamExt, stream};
use micromegas_object_cache::blocks::blocks_for_range;
use micromegas_object_cache::prefetch::{PrefetchItem, PrefetchResponse};
use micromegas_object_cache::range_cache::{DEMAND_WINDOW_BLOCKS, RangeError, StreamRangesCaller};
use micromegas_object_cache::validation::validate_key;
use micromegas_tracing::prelude::*;
use micromegas_tracing::property_set::{Property, PropertySet};
use serde::Deserialize;
use std::collections::HashSet;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use std::time::Instant;
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::mpsc::error::TrySendError;

/// Maximum number of ranges accepted in a single multi-range request. A
/// parquet/block reader fetches at most a few thousand column chunks per file,
/// so this is comfortably above legitimate use while bounding per-request work.
const MAX_RANGES_PER_REQUEST: usize = 4096;

/// Maximum size of a single NDJSON line in a `POST /prefetch` body. An item at
/// the `MAX_RANGES_PER_REQUEST` cap serializes to roughly 100 KiB, so this
/// leaves about 10x headroom while still bounding per-line memory; unlike a
/// whole-body cap, this doesn't limit how many items a client can batch.
const MAX_PREFETCH_LINE_BYTES: usize = 1024 * 1024; // 1 MiB

pub const BYTES_PER_MEM_PERMIT: u64 = 1024 * 1024;

/// Number of `mem_permits` (1 MiB each) needed to cover `bytes`.
pub fn permits_for_bytes(bytes: u64) -> u32 {
    bytes.div_ceil(BYTES_PER_MEM_PERMIT) as u32
}

/// Byte size of the fixed window a streaming request's memory charge is
/// capped at: `permits_for_bytes(min(response size, this))`. Shared by the
/// handlers' proportional per-stream charge below and by the startup guard
/// in `object_cache_srv.rs`, which floors `--memory-budget-mb` at this value
/// so a large read's charge can never exceed the whole budget — which would
/// otherwise make `acquire_many_owned` hang forever instead of failing fast.
pub fn stream_window_bytes(block_size: u64) -> u64 {
    2 * DEMAND_WINDOW_BLOCKS * block_size
}

/// A response body wrapping a byte stream plus a memory-budget permit held
/// for the body's entire lifetime: the permit is released whenever this value
/// is dropped, whether that's after the body was fully sent or because the
/// connection was aborted mid-stream. This is what makes the memory-budget
/// guard cover the response's full lifetime rather than just the fetch
/// window.
struct PermitBody {
    stream: BoxStream<'static, Result<Bytes, anyhow::Error>>,
    _permit: OwnedSemaphorePermit,
}

impl Stream for PermitBody {
    type Item = Result<Bytes, anyhow::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.get_mut().stream).poll_next(cx)
    }
}

fn permit_body(
    stream: BoxStream<'static, Result<Bytes, anyhow::Error>>,
    permit: OwnedSemaphorePermit,
) -> Body {
    Body::from_stream(PermitBody {
        stream,
        _permit: permit,
    })
}

/// Wrap `inner` so that `on_complete` is called exactly once with the total
/// bytes yielded, as soon as the payload is fully produced. When
/// `expected_total` is known, the callback fires immediately BEFORE yielding
/// the chunk that completes it — a `Content-Length`-framed HTTP body is
/// considered complete by the transport once the declared byte count is
/// written and is never polled again for a terminal `None`, so firing after
/// the final `yield` (or on the terminal `None`) would never run in practice.
/// A mid-stream `Err`, or a stream that ends before reaching `expected_total`,
/// skips the callback — preserving the accepted under-reporting on truncation.
/// When `expected_total` is `None` (length genuinely unknown up front), there
/// is no "before completion" point to detect, so the callback instead fires
/// on the terminal `None`, with whichever total was accumulated by then.
fn count_bytes_served<F>(
    mut inner: BoxStream<'static, Result<Bytes, anyhow::Error>>,
    expected_total: Option<u64>,
    on_complete: F,
) -> BoxStream<'static, Result<Bytes, anyhow::Error>>
where
    F: FnOnce(u64) + Send + 'static,
{
    gen_stream! {
        let mut total = 0u64;
        let mut on_complete = Some(on_complete);
        while let Some(item) = inner.next().await {
            match &item {
                Ok(chunk) => {
                    total += chunk.len() as u64;
                    // Fire BEFORE the final yield: the transport may never
                    // poll us again once Content-Length is satisfied.
                    if let Some(expected) = expected_total
                        && total >= expected
                        && let Some(f) = on_complete.take()
                    {
                        f(total);
                    }
                    yield item;
                }
                Err(_) => {
                    yield item;
                    return; // mid-stream error: skip the callback
                }
            }
        }
        // Fallback for streams with no known expected length: fire on the
        // terminal `None` with whatever total was accumulated. When
        // `expected_total` is `Some`, a stream that ends early (without an
        // error) must NOT fire here — that's the accepted under-reporting
        // case documented above, not a second chance to fire.
        if expected_total.is_none()
            && let Some(f) = on_complete.take()
        {
            f(total);
        }
    }
    .boxed()
}

/// Interleave each range's 8-byte little-endian length prefix with its data
/// chunks pulled from `inner`, matching the on-wire framing the client's
/// length-prefixed reader expects. `range_lens` must have exactly one entry
/// per non-degenerate range passed to the `stream_ranges` call that produced
/// `inner`, in the same order, and `inner` must yield each range's bytes
/// contiguously (see `RangeCache::stream_ranges`'s ordering guarantee).
fn frame_ranges_stream(
    mut inner: BoxStream<'static, Result<Bytes, anyhow::Error>>,
    range_lens: Vec<u64>,
) -> BoxStream<'static, Result<Bytes, anyhow::Error>> {
    try_stream! {
        for len in range_lens {
            let mut prefix = BytesMut::with_capacity(8);
            prefix.put_u64_le(len);
            yield prefix.freeze();

            let mut remaining = len;
            while remaining > 0 {
                let chunk = inner
                    .next()
                    .await
                    .expect("stream_ranges under-yielded for a non-degenerate range")?;
                remaining = remaining.saturating_sub(chunk.len() as u64);
                yield chunk;
            }
        }
    }
    .boxed()
}

/// Map a response status to the `status` metric-tag value. Deliberately a
/// small closed set (`"other"` covers everything else) so the `status`
/// dimension stays bounded per the tagged-metric cardinality contract.
fn status_label(status: StatusCode) -> &'static str {
    match status {
        StatusCode::OK => "200",
        StatusCode::PARTIAL_CONTENT => "206",
        StatusCode::BAD_REQUEST => "400",
        StatusCode::NOT_FOUND => "404",
        StatusCode::RANGE_NOT_SATISFIABLE => "416",
        StatusCode::INTERNAL_SERVER_ERROR => "500",
        StatusCode::SERVICE_UNAVAILABLE => "503",
        _ => "other",
    }
}

/// Thin wrapper around `head_handler_inner`, mirroring
/// `get_range_handler`/`post_ranges_handler`: counts every outcome (not just
/// the success path) with a `status`/`prefix`-tagged `object_cache_head_requests`.
/// Before this, HEAD traffic had no direct counter and could only be inferred
/// as a residual of the size/HEAD-tier metrics (#1280).
pub async fn head_handler(
    Path(key): Path<String>,
    State(state): State<AppState>,
) -> Result<Response, StatusCode> {
    let prefix = state.cache.classify(&key);
    let result = head_handler_inner(key, state).await;
    let status = match &result {
        Ok(resp) => resp.status(),
        Err(code) => *code,
    };
    imetric!(
        "object_cache_head_requests",
        "count",
        PropertySet::find_or_create(vec![
            Property::new("status", status_label(status)),
            Property::new("prefix", prefix),
        ]),
        1_u64
    );
    result
}

#[span_fn]
async fn head_handler_inner(key: String, state: AppState) -> Result<Response, StatusCode> {
    if let Err(e) = validate_key(&key, &state.allowed_prefixes) {
        warn!("rejected key {key}: {e}");
        return Err(StatusCode::BAD_REQUEST);
    }
    match state.cache.size(&key).await {
        Ok(size) => {
            let response = Response::builder()
                .status(StatusCode::OK)
                .header("Content-Length", size.to_string())
                .body(Body::empty())
                .expect("build HEAD response");
            Ok(response)
        }
        Err(e) => {
            if is_not_found(&e) {
                Err(StatusCode::NOT_FOUND)
            } else {
                error!("HEAD {key}: {e:?}");
                Err(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    }
}

/// Thin wrapper around `get_range_handler_inner` that counts every outcome
/// the handler body can produce (not just the success path): runs the inner
/// handler, derives the final `status` from its `Ok`/`Err` result, and emits
/// `object_cache_get_requests` exactly once per call, tagged with `status`
/// and `prefix`. This is what fixes the success-only undercounting bug (see
/// the plan's "Correctness fixes") -- a `400`/`404`/`416`/`500` GET used to
/// go uncounted entirely.
pub async fn get_range_handler(
    Path(key): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let prefix = state.cache.classify(&key);
    let result = get_range_handler_inner(key, state, headers).await;
    let status = match &result {
        Ok(resp) => resp.status(),
        Err(code) => *code,
    };
    imetric!(
        "object_cache_get_requests",
        "count",
        PropertySet::find_or_create(vec![
            Property::new("status", status_label(status)),
            Property::new("prefix", prefix),
        ]),
        1_u64
    );
    result
}

#[span_fn]
async fn get_range_handler_inner(
    key: String,
    state: AppState,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let request_start = Instant::now();
    if let Err(e) = validate_key(&key, &state.allowed_prefixes) {
        warn!("rejected key {key}: {e}");
        return Err(StatusCode::BAD_REQUEST);
    }

    let file_size = match state.cache.size(&key).await {
        Ok(s) => s,
        Err(e) => {
            if is_not_found(&e) {
                return Err(StatusCode::NOT_FOUND);
            }
            error!("size {key}: {e:?}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // A zero-byte object cannot be expressed as a satisfiable byte range, and
    // `Content-Range: bytes 0-0/0` is not RFC 7233-valid for an empty entity.
    // Serve it as a plain 200 with an empty body instead of a 206.
    if file_size == 0 {
        let response = Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", "0")
            .body(Body::empty())
            .expect("build empty GET response");
        return Ok(response);
    }

    let range_header = match headers.get("range").or_else(|| headers.get("Range")) {
        Some(h) => h.to_str().unwrap_or("").to_string(),
        None => format!("bytes=0-{}", file_size.saturating_sub(1)),
    };

    let byte_range = match parse_range_header(&range_header, file_size) {
        Ok(r) => r,
        Err(e) => {
            warn!("bad Range header {range_header}: {e}");
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    // An explicit range whose end exceeds the object size (or an open-ended
    // range whose start is past EOF) is unsatisfiable per RFC 7233.
    if byte_range.end > file_size || byte_range.start > file_size {
        warn!("range {byte_range:?} out of bounds for {key} (file_size {file_size})");
        return Err(StatusCode::RANGE_NOT_SATISFIABLE);
    }

    // An open-ended range at exactly EOF (`bytes=<file_size>-`) is a valid
    // zero-length read. A zero-length entity has no satisfiable byte position to
    // express in a 206 `Content-Range`, so serve it as a plain 200 with an empty
    // body, mirroring the zero-byte-object case above, rather than falling
    // through to `get_range` (which would build a malformed 206 header).
    if byte_range.start == byte_range.end {
        let response = Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", "0")
            .body(Body::empty())
            .expect("build empty range response");
        return Ok(response);
    }

    // Proportional memory charge: a small read charges close to its actual
    // size, a large one clamps to the fixed streaming window, so per-request
    // in-flight memory stays bounded regardless of how large the range is.
    // The client falls back to the direct store on any non-2xx, so an
    // oversized read still succeeds (just uncached).
    let requested_bytes = byte_range.end - byte_range.start;
    let window = stream_window_bytes(state.cache.block_size());
    let permits_needed = permits_for_bytes(requested_bytes.min(window));
    let mem_permit_wait_start = Instant::now();
    let mem_permit = state
        .mem_permits
        .clone()
        .acquire_many_owned(permits_needed)
        .await
        .expect("mem_permits semaphore is never closed");
    fmetric!(
        "object_cache_mem_permit_wait_ms",
        "ms",
        mem_permit_wait_start.elapsed().as_secs_f64() * 1000.0
    );

    // `_with_size` reuses the `file_size` already resolved above instead of
    // resolving it again inside `stream_ranges`, so `range_cache_size_backend_hit`
    // fires exactly once per ranged GET (see the plan's "Correctness fixes").
    let mut inner = match state
        .cache
        .stream_ranges_with_size(
            &key,
            vec![byte_range.clone()],
            file_size,
            StreamRangesCaller::Range,
        )
        .await
    {
        Ok(s) => s.boxed(),
        Err(e) => {
            if let Some(RangeError::OutOfBounds { .. }) = e.downcast_ref::<RangeError>() {
                warn!("range {byte_range:?} out of bounds for {key}: {e}");
                return Err(StatusCode::RANGE_NOT_SATISFIABLE);
            }
            if is_not_found(&e) {
                return Err(StatusCode::NOT_FOUND);
            }
            error!("get_range {key} {byte_range:?}: {e:?}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Commit-before-stream: await the first chunk before building the
    // response, so a dead origin still surfaces as 500 rather than an
    // aborted 200/206.
    let first = match inner.next().await {
        Some(Ok(chunk)) => chunk,
        Some(Err(e)) => {
            error!("get_range {key} {byte_range:?}: {e:?}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        None => unreachable!("stream_ranges yielded no chunks for a non-degenerate range"),
    };

    // Time to first byte, now that streaming (#1189/#1222) has landed --
    // measured from handler entry to the point the first chunk is in hand.
    fmetric!(
        "object_cache_ttfb_ms",
        "ms",
        PropertySet::find_or_create(vec![Property::new("prefix", state.cache.classify(&key))]),
        request_start.elapsed().as_secs_f64() * 1000.0
    );

    let key_for_log = key.clone();
    let range_for_log = byte_range.clone();
    let full = stream::once(async move { Ok::<Bytes, anyhow::Error>(first) })
        .chain(inner)
        .boxed();
    let counted = count_bytes_served(full, Some(requested_bytes), move |bytes| {
        imetric!("object_cache_get_bytes_served", "bytes", bytes);
        debug!(
            "GET {key_for_log} {}-{} served {bytes} bytes",
            range_for_log.start,
            range_for_log.end.saturating_sub(1)
        );
    });

    let response = Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header("Content-Type", "application/octet-stream")
        .header("Content-Length", requested_bytes.to_string())
        .header(
            "Content-Range",
            format!(
                "bytes {}-{}/{}",
                byte_range.start,
                byte_range.end.saturating_sub(1),
                file_size
            ),
        )
        .body(permit_body(counted, mem_permit))
        .expect("build GET response");
    Ok(response)
}

#[derive(Deserialize)]
struct RangesRequest {
    ranges: Vec<[u64; 2]>,
}

/// Thin wrapper around `post_ranges_handler_inner`, mirroring
/// `get_range_handler`'s wrapper: counts every outcome (not just success)
/// with a `status`/`prefix`-tagged `object_cache_ranges_requests`. The
/// empty-ranges short-circuit inside the inner handler emits
/// `object_cache_ranges_count` / `_ranges_bytes_served` itself (they're
/// meaningful only on that success path) but no longer emits
/// `object_cache_ranges_requests` -- this wrapper is now its sole emitter,
/// so a `{"ranges":[]}` request isn't double-counted.
pub async fn post_ranges_handler(
    Path(key): Path<String>,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Response, StatusCode> {
    let prefix = state.cache.classify(&key);
    let result = post_ranges_handler_inner(key, state, body).await;
    let status = match &result {
        Ok(resp) => resp.status(),
        Err(code) => *code,
    };
    imetric!(
        "object_cache_ranges_requests",
        "count",
        PropertySet::find_or_create(vec![
            Property::new("status", status_label(status)),
            Property::new("prefix", prefix),
        ]),
        1_u64
    );
    result
}

#[span_fn]
async fn post_ranges_handler_inner(
    key: String,
    state: AppState,
    body: Bytes,
) -> Result<Response, StatusCode> {
    let request_start = Instant::now();
    if let Err(e) = validate_key(&key, &state.allowed_prefixes) {
        warn!("rejected key {key}: {e}");
        return Err(StatusCode::BAD_REQUEST);
    }

    let req: RangesRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            warn!("bad ranges JSON: {e}");
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    // Bound the number of ranges to cap per-request work on this public
    // authenticated endpoint.
    if req.ranges.len() > MAX_RANGES_PER_REQUEST {
        warn!(
            "rejected {n} ranges for {key}: exceeds max {MAX_RANGES_PER_REQUEST}",
            n = req.ranges.len()
        );
        return Err(StatusCode::BAD_REQUEST);
    }

    // Reject inverted/degenerate ranges (e.g. `[100, 50]` or `[50, 50]`),
    // matching the single-range path's `parse_range_header` validation. An
    // empty or backwards range would otherwise silently yield 0-length data.
    // While iterating, sum the requested bytes for the proportional memory
    // charge below, and track which blocks each range touches: a request
    // with many scattered small ranges can transiently retain a whole block
    // per range while charging only for the (much smaller) requested bytes,
    // so the charge must also account for distinct blocks touched.
    let block_size = state.cache.block_size();
    let mut total_requested: u64 = 0;
    let mut touched_blocks: HashSet<u64> = HashSet::new();
    for &[s, e] in &req.ranges {
        if s >= e {
            warn!("rejected inverted range [{s}, {e}] for {key}");
            return Err(StatusCode::BAD_REQUEST);
        }
        total_requested = total_requested.saturating_add(e - s);
        touched_blocks.extend(blocks_for_range(s, e, block_size));
    }

    // Empty-ranges short-circuit, mirroring `get_ranges`'s own guard:
    // `stream_ranges` always does a `size()` lookup up front regardless of
    // `ranges`, so without this a `{"ranges":[]}` request against a missing
    // key would flip from today's 200 (empty body) to 404. `_ranges_count` /
    // `_ranges_bytes_served` are still emitted here to match the Ok-arm
    // behavior below; `object_cache_ranges_requests` is emitted once by the
    // `post_ranges_handler` wrapper for every outcome, so it is not repeated
    // here.
    if req.ranges.is_empty() {
        imetric!("object_cache_ranges_count", "count", 0_u64);
        imetric!("object_cache_ranges_bytes_served", "bytes", 0_u64);
        let response = Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/octet-stream")
            .body(Body::empty())
            .expect("build empty ranges response");
        return Ok(response);
    }

    // Each range is written into the response body behind an 8-byte
    // little-endian length prefix (see `frame_ranges_stream`); account for
    // that overhead so the permit charge matches the actual framed size.
    let framed_response_bytes = total_requested.saturating_add(8 * req.ranges.len() as u64);

    // Proportional memory charge, capped at the fixed streaming window (see
    // `get_range_handler` for the same pattern and its rationale). Charge for
    // whichever is larger of the framed response size and the bytes backing
    // the distinct blocks this request touches, so scattered small ranges
    // spread across many blocks can't under-charge the shared memory budget.
    let block_bytes = touched_blocks.len() as u64 * block_size;
    let charge_bytes = framed_response_bytes.max(block_bytes);
    let window = stream_window_bytes(block_size);
    let permits_needed = permits_for_bytes(charge_bytes.min(window));
    let mem_permit_wait_start = Instant::now();
    let mem_permit = state
        .mem_permits
        .clone()
        .acquire_many_owned(permits_needed)
        .await
        .expect("mem_permits semaphore is never closed");
    fmetric!(
        "object_cache_mem_permit_wait_ms",
        "ms",
        mem_permit_wait_start.elapsed().as_secs_f64() * 1000.0
    );

    let ranges: Vec<std::ops::Range<u64>> = req.ranges.iter().map(|&[s, e]| s..e).collect();
    let range_lens: Vec<u64> = ranges.iter().map(|r| r.end - r.start).collect();
    let range_count = ranges.len() as u64;

    let inner = match state
        .cache
        .stream_ranges(&key, ranges.clone(), StreamRangesCaller::Ranges)
        .await
    {
        Ok(s) => s.boxed(),
        Err(e) => {
            if let Some(RangeError::OutOfBounds { .. }) = e.downcast_ref::<RangeError>() {
                warn!("ranges {ranges:?} out of bounds for {key}: {e}");
                return Err(StatusCode::RANGE_NOT_SATISFIABLE);
            }
            if is_not_found(&e) {
                return Err(StatusCode::NOT_FOUND);
            }
            error!("get_ranges {key}: {e:?}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let mut framed = frame_ranges_stream(inner, range_lens);

    // Commit-before-stream: await the first frame (always the first range's
    // length prefix, since `req.ranges` is non-empty here) before building
    // the response, so a dead origin still surfaces as 500 rather than an
    // aborted 200.
    let first = match framed.next().await {
        Some(Ok(chunk)) => chunk,
        Some(Err(e)) => {
            error!("get_ranges {key}: {e:?}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        None => unreachable!("frame_ranges_stream always yields at least one prefix"),
    };

    fmetric!(
        "object_cache_ttfb_ms",
        "ms",
        PropertySet::find_or_create(vec![Property::new("prefix", state.cache.classify(&key))]),
        request_start.elapsed().as_secs_f64() * 1000.0
    );

    imetric!("object_cache_ranges_count", "count", range_count);

    let key_for_log = key.clone();
    let full = stream::once(async move { Ok::<Bytes, anyhow::Error>(first) })
        .chain(framed)
        .boxed();
    let counted = count_bytes_served(full, Some(framed_response_bytes), move |bytes| {
        imetric!("object_cache_ranges_bytes_served", "bytes", bytes);
        debug!("POST ranges {key_for_log}: {range_count} ranges served {bytes} bytes");
    });

    let response = Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/octet-stream")
        .body(permit_body(counted, mem_permit))
        .expect("build ranges response");
    Ok(response)
}

/// Parse and process a single NDJSON line from a `POST /prefetch` body,
/// updating the running counters. Returns `Err` only when the prefetch queue
/// worker is gone, which aborts the whole request with `503`.
fn process_prefetch_line(
    line: &[u8],
    state: &AppState,
    accepted: &mut usize,
    rejected: &mut usize,
    dropped: &mut usize,
) -> Result<(), StatusCode> {
    if line.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(());
    }

    let item: PrefetchItem = match serde_json::from_slice(line) {
        Ok(item) => item,
        Err(e) => {
            warn!("bad prefetch line JSON: {e}");
            *rejected += 1;
            return Ok(());
        }
    };

    if let Err(e) = validate_key(&item.key, &state.allowed_prefixes) {
        warn!("rejected prefetch key {}: {e}", item.key);
        *rejected += 1;
        return Ok(());
    }
    let range_count = item.ranges.as_ref().map_or(0, |r| r.len());
    if range_count > MAX_RANGES_PER_REQUEST {
        warn!(
            "rejected prefetch key {}: {range_count} ranges exceeds max {MAX_RANGES_PER_REQUEST}",
            item.key
        );
        *rejected += 1;
        return Ok(());
    }
    // `item.size` is intentionally NOT capped: the worker streams the
    // block-index space in bounded windows (`prefetch_queue.rs`) rather
    // than materializing it, so per-item work is bounded regardless of
    // `size`, and an over-claimed `size` is bounded by stop-on-first-error
    // once fills start. This lets legitimate multi-GB partitions warm.
    // Absent/empty ranges = whole-object warm of [0, item.size), per the
    // shared-type contract; only present ranges need bounds validation.
    let has_invalid_range = item
        .ranges
        .iter()
        .flatten()
        .any(|&[s, e]| s >= e || e > item.size);
    if has_invalid_range {
        warn!(
            "rejected prefetch key {}: inverted or out-of-bounds range for size {}",
            item.key, item.size
        );
        *rejected += 1;
        return Ok(());
    }

    match state.prefetch_tx.try_send(item) {
        Ok(()) => *accepted += 1,
        Err(TrySendError::Full(_)) => {
            *dropped += 1;
            imetric!("object_cache_prefetch_dropped", "count", 1_u64);
        }
        Err(TrySendError::Closed(_)) => {
            error!("prefetch queue worker is gone");
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
    }
    Ok(())
}

/// Accept a batch of keys to warm at prefetch priority and return
/// immediately: fills are handed to the bounded queue in `AppState` and run
/// asynchronously by the consumer task, so this handler never blocks on an
/// origin fetch and never acquires a `mem_permit` (the response carries no
/// object bytes; the fill's memory is already bounded by the scheduler).
///
/// The body is `application/x-ndjson`: one `PrefetchItem` JSON object per
/// `\n`-terminated line, consumed incrementally as it arrives so the whole
/// batch is never buffered. `MAX_PREFETCH_LINE_BYTES` bounds a single line;
/// there is no whole-body size limit.
#[span_fn]
pub async fn prefetch_handler(
    State(state): State<AppState>,
    body: Body,
) -> Result<Response, StatusCode> {
    let mut stream = body.into_data_stream();
    let mut buf = BytesMut::new();
    let mut accepted = 0usize;
    let mut rejected = 0usize;
    let mut dropped = 0usize;

    loop {
        let chunk = match stream.next().await {
            Some(Ok(chunk)) => chunk,
            Some(Err(e)) => {
                warn!("error reading prefetch body: {e}");
                break;
            }
            None => break,
        };
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let mut line = buf.split_to(pos + 1);
            line.truncate(pos);
            process_prefetch_line(&line, &state, &mut accepted, &mut rejected, &mut dropped)?;
        }
        if buf.len() > MAX_PREFETCH_LINE_BYTES {
            warn!("prefetch line exceeds max {MAX_PREFETCH_LINE_BYTES} bytes");
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    // A final line with no trailing newline is still processed; `buf` here
    // is bounded by the same per-chunk check above.
    if !buf.is_empty() {
        process_prefetch_line(&buf, &state, &mut accepted, &mut rejected, &mut dropped)?;
    }

    imetric!("object_cache_prefetch_requests", "count", 1_u64);
    imetric!(
        "object_cache_prefetch_keys_enqueued",
        "count",
        accepted as u64
    );
    debug!("POST prefetch: accepted={accepted} rejected={rejected} dropped={dropped}");

    let resp_body = PrefetchResponse {
        accepted,
        rejected,
        dropped,
    };
    let response = Response::builder()
        .status(StatusCode::ACCEPTED)
        .header("Content-Type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&resp_body).expect("serialize PrefetchResponse"),
        ))
        .expect("build prefetch response");
    Ok(response)
}
