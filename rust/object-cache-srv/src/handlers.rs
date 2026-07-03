use crate::app_state::AppState;
use crate::validation::{is_not_found, parse_range_header};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use bytes::{BufMut, Bytes, BytesMut};
use futures::{Stream, StreamExt};
use micromegas_object_cache::blocks::blocks_for_range;
use micromegas_object_cache::prefetch::{PrefetchItem, PrefetchResponse};
use micromegas_object_cache::range_cache::RangeError;
use micromegas_object_cache::validation::validate_key;
use micromegas_tracing::prelude::*;
use serde::Deserialize;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::mpsc::error::TrySendError;

/// Maximum number of ranges accepted in a single multi-range request. A
/// parquet/block reader fetches at most a few thousand column chunks per file,
/// so this is comfortably above legitimate use while bounding per-request work.
const MAX_RANGES_PER_REQUEST: usize = 4096;

/// Maximum total requested bytes (summed across all ranges) for a single
/// multi-range request. The handler assembles all results in memory, so this
/// caps peak allocation regardless of how many ranges overlap the same bytes.
const MAX_TOTAL_REQUESTED_BYTES: u64 = 512 * 1024 * 1024; // 512 MiB

/// Maximum size of a single NDJSON line in a `POST /prefetch` body. An item at
/// the `MAX_RANGES_PER_REQUEST` cap serializes to roughly 100 KiB, so this
/// leaves about 10x headroom while still bounding per-line memory; unlike a
/// whole-body cap, this doesn't limit how many items a client can batch.
const MAX_PREFETCH_LINE_BYTES: usize = 1024 * 1024; // 1 MiB

const BYTES_PER_MEM_PERMIT: u64 = 1024 * 1024;

/// Number of `mem_permits` (1 MiB each) needed to cover `bytes`.
fn permits_for_bytes(bytes: u64) -> u32 {
    bytes.div_ceil(BYTES_PER_MEM_PERMIT) as u32
}

/// A one-shot response body that owns a memory-budget permit for its entire
/// lifetime: the permit is released whenever this value is dropped, whether
/// that's after the body was fully sent or because the connection was
/// aborted mid-stream. This is what makes the memory-budget guard cover the
/// response's full lifetime rather than just the assembly window (see
/// `object_cache_fetch_rework_plan.md` §5).
struct PermitBody {
    data: Option<Bytes>,
    _permit: OwnedSemaphorePermit,
}

impl Stream for PermitBody {
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.get_mut().data.take().map(Ok))
    }
}

fn permit_body(data: Bytes, permit: OwnedSemaphorePermit) -> Body {
    Body::from_stream(PermitBody {
        data: Some(data),
        _permit: permit,
    })
}

pub async fn head_handler(
    Path(key): Path<String>,
    State(state): State<AppState>,
) -> Result<Response, StatusCode> {
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

pub async fn get_range_handler(
    Path(key): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
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
    // range whose start is past EOF) is unsatisfiable per RFC 7233 and must be
    // 416, not 413. This is checked before the 512 MiB span cap so an
    // out-of-bounds request is never misreported as too large.
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

    // The cache assembles the requested span contiguously in memory, so cap the
    // single-range read at the same limit as the multi-range POST path to bound
    // peak allocation. The client falls back to the direct store on any non-2xx,
    // so an oversized read still succeeds (just uncached).
    let requested_bytes = byte_range.end - byte_range.start;
    if requested_bytes > MAX_TOTAL_REQUESTED_BYTES {
        warn!(
            "rejected range {byte_range:?} for {key}: requested bytes exceed max {MAX_TOTAL_REQUESTED_BYTES}"
        );
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }

    let permits_needed = permits_for_bytes(requested_bytes);
    if permits_needed > state.memory_budget_mb {
        warn!(
            "rejected range {byte_range:?} for {key}: {requested_bytes} bytes exceeds the \
             whole memory budget ({} MiB)",
            state.memory_budget_mb
        );
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let mem_permit = state
        .mem_permits
        .clone()
        .acquire_many_owned(permits_needed)
        .await
        .expect("mem_permits semaphore is never closed");

    match state.cache.get_range(&key, byte_range.clone()).await {
        Ok(data) => {
            let content_length = data.len();
            imetric!("object_cache_get_requests", "count", 1_u64);
            imetric!(
                "object_cache_get_bytes_served",
                "bytes",
                content_length as u64
            );
            debug!(
                "GET {key} {}-{} served {content_length} bytes",
                byte_range.start,
                byte_range.end.saturating_sub(1)
            );
            let response = Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header("Content-Type", "application/octet-stream")
                .header("Content-Length", content_length.to_string())
                .header(
                    "Content-Range",
                    format!(
                        "bytes {}-{}/{}",
                        byte_range.start,
                        byte_range.end.saturating_sub(1),
                        file_size
                    ),
                )
                .body(permit_body(data, mem_permit))
                .expect("build GET response");
            Ok(response)
        }
        Err(e) => {
            if let Some(RangeError::OutOfBounds { .. }) = e.downcast_ref::<RangeError>() {
                warn!("range {byte_range:?} out of bounds for {key}: {e}");
                return Err(StatusCode::RANGE_NOT_SATISFIABLE);
            }
            error!("get_range {key} {byte_range:?}: {e:?}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(Deserialize)]
struct RangesRequest {
    ranges: Vec<[u64; 2]>,
}

pub async fn post_ranges_handler(
    Path(key): Path<String>,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Response, StatusCode> {
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
    // While iterating, sum the requested bytes to bound the in-memory assembled
    // response (overlapping ranges can otherwise amplify allocation).
    let mut total_requested: u64 = 0;
    for &[s, e] in &req.ranges {
        if s >= e {
            warn!("rejected inverted range [{s}, {e}] for {key}");
            return Err(StatusCode::BAD_REQUEST);
        }
        total_requested = total_requested.saturating_add(e - s);
        if total_requested > MAX_TOTAL_REQUESTED_BYTES {
            warn!(
                "rejected ranges for {key}: total requested bytes exceeds max {MAX_TOTAL_REQUESTED_BYTES}"
            );
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
    }

    // Each range is written into the response body behind an 8-byte
    // little-endian length prefix (see `buf.put_u64_le` below); account for
    // that overhead so the permit budget matches the actual assembled size.
    let response_bytes = total_requested.saturating_add(8 * req.ranges.len() as u64);

    // `get_ranges` retains a full block for every distinct block the ranges
    // touch, all held simultaneously until assembly completes — so scattered
    // small ranges amplify well past the requested bytes (4096 one-byte
    // ranges in distinct 1 MiB blocks retain ~4 GiB). Dedupe blocks the same
    // way `get_ranges` does and charge the budget for whichever is larger:
    // the retained blocks or the framed response body.
    let block_size = state.cache.block_size();
    let mut touched_blocks = std::collections::BTreeSet::new();
    for &[s, e] in &req.ranges {
        let blk = blocks_for_range(s, e, block_size);
        touched_blocks.extend(blk.start..blk.end);
    }
    let block_bytes = touched_blocks.len() as u64 * block_size;

    let charged_bytes = response_bytes.max(block_bytes);
    let permits_needed = permits_for_bytes(charged_bytes);
    if permits_needed > state.memory_budget_mb {
        warn!(
            "rejected ranges for {key}: {charged_bytes} charged bytes (response \
             {response_bytes}, blocks {block_bytes}) exceeds the whole memory budget \
             ({} MiB)",
            state.memory_budget_mb
        );
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let mem_permit = state
        .mem_permits
        .clone()
        .acquire_many_owned(permits_needed)
        .await
        .expect("mem_permits semaphore is never closed");

    let ranges: Vec<std::ops::Range<u64>> = req.ranges.iter().map(|&[s, e]| s..e).collect();

    match state.cache.get_ranges(&key, &ranges).await {
        Ok(results) => {
            let mut buf = BytesMut::new();
            for chunk in &results {
                buf.put_u64_le(chunk.len() as u64);
                buf.put_slice(chunk);
            }
            let bytes_served = buf.len();
            imetric!("object_cache_ranges_requests", "count", 1_u64);
            imetric!("object_cache_ranges_count", "count", ranges.len() as u64);
            imetric!(
                "object_cache_ranges_bytes_served",
                "bytes",
                bytes_served as u64
            );
            debug!(
                "POST ranges {key}: {} ranges served {bytes_served} bytes",
                ranges.len()
            );
            let response = Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/octet-stream")
                .body(permit_body(buf.freeze(), mem_permit))
                .expect("build ranges response");
            Ok(response)
        }
        Err(e) => {
            if let Some(RangeError::OutOfBounds { .. }) = e.downcast_ref::<RangeError>() {
                warn!("ranges {ranges:?} out of bounds for {key}: {e}");
                return Err(StatusCode::RANGE_NOT_SATISFIABLE);
            }
            if is_not_found(&e) {
                return Err(StatusCode::NOT_FOUND);
            }
            error!("get_ranges {key}: {e:?}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
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
