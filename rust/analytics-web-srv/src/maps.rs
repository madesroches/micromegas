//! Map asset endpoints.
//!
//! GLBs are stored gzipped under a `.gz` suffix on the configured prefix —
//! the suffix is the *only* signal we need on the read path. Catalog
//! listing filters for it, GET tacks `.gz` onto the requested name and
//! always responds with `Content-Encoding: gzip`, and PUT gzips the body
//! before writing. That keeps the design portable: no
//! `PutOptions::attributes`, so `object_store::LocalFileSystem` (which
//! rejects attributes with `NotImplemented`) works the same as S3/GCS.

use crate::auth::{AdminRequired, AdminUser};
use anyhow::{Context, Result};
use axum::{
    Extension, Json,
    body::{Body, Bytes},
    extract::Path as AxumPath,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use flate2::{Compression, write::GzEncoder};
use futures::StreamExt;
use micromegas::tracing::prelude::*;
use object_store::{ObjectStore, path::Path as ObjectPath, prefix::PrefixStore};
use serde::Serialize;
use std::io::Write;
use std::sync::Arc;

/// Default max upload body size (256 MiB). Configurable via the
/// `MICROMEGAS_MAPS_MAX_UPLOAD_BYTES` env var, surfaced on `MapsState`.
pub const DEFAULT_MAX_UPLOAD_BYTES: usize = 256 * 1024 * 1024;

/// Suffix appended to every stored object key. PUT writes `<name>.gz`,
/// GET reads `<name>.gz`, and catalog listing only surfaces keys carrying
/// this suffix (with the suffix stripped for display).
const GZ_SUFFIX: &str = ".gz";

/// The maps prefix is reserved for direct children — any subdirectory
/// content is ignored. The configured store is treated as the source of
/// truth: whatever flat `*.gz` files live there are the catalog.
///
/// In practice, axum's `{filename}` path capture is already a single
/// segment (so `/` cannot reach the blob handler from the URL side), and
/// `object_store::path::Path` keys are opaque (no `..` traversal).
/// Filtering out keys that contain `/` keeps the catalog flat and stops
/// nested objects from sneaking into the listing.
pub fn is_direct_child(name: &str) -> bool {
    !name.is_empty() && !name.contains('/')
}

/// The stored key for a user-facing filename: `level.glb` → `level.glb.gz`.
fn stored_key(filename: &str) -> String {
    format!("{filename}{GZ_SUFFIX}")
}

/// Shared state for the maps handlers.
///
/// Held as `Extension<MapsState>` to match the local convention (every
/// existing protected route receives its dependencies through Extensions).
#[derive(Clone)]
pub struct MapsState {
    pub store: Option<Arc<dyn ObjectStore>>,
    /// Configured cap on the upload body, threaded through `MapsState` so
    /// the router-build site can size the per-route `DefaultBodyLimit`
    /// layer from a single source of truth. Not referenced inside handlers
    /// — the 413 response from the layer is axum's plaintext default.
    pub max_upload_bytes: usize,
}

impl MapsState {
    pub fn new(store: Option<Arc<dyn ObjectStore>>) -> Self {
        Self {
            store,
            max_upload_bytes: DEFAULT_MAX_UPLOAD_BYTES,
        }
    }

    pub fn with_max_upload_bytes(store: Option<Arc<dyn ObjectStore>>, max: usize) -> Self {
        Self {
            store,
            max_upload_bytes: max,
        }
    }
}

/// Connect to the store named by `MICROMEGAS_MAPS_OBJECT_STORE_URI`.
///
/// Returns `Ok(None)` when the env var is unset — the catalog endpoint
/// will respond with 503 in that case.
pub fn connect_maps_store(uri: Option<&str>) -> Result<Option<Arc<dyn ObjectStore>>> {
    let Some(uri) = uri else {
        return Ok(None);
    };
    let url = url::Url::parse(uri).context("parsing MICROMEGAS_MAPS_OBJECT_STORE_URI")?;
    let (store, prefix) =
        object_store::parse_url_opts(&url, std::env::vars().map(|(k, v)| (k.to_lowercase(), v)))
            .context("connecting to MICROMEGAS_MAPS_OBJECT_STORE_URI")?;
    Ok(Some(Arc::new(PrefixStore::new(store, prefix))))
}

/// Entry returned by the catalog endpoint. `size` is the on-disk
/// (gzipped) byte count — that is what storage is billed against and what
/// the GET response will set as `Content-Length`.
#[derive(Debug, Serialize)]
pub struct CatalogEntry {
    pub file: String,
    pub size: u64,
    pub last_modified: DateTime<Utc>,
}

/// GET /api/maps/catalog
///
/// Lists every flat `*.gz` object directly under the configured prefix and
/// strips the suffix for display. The catalog is the listing — there is no
/// separate `maps.json` to keep in sync.
#[span_fn]
pub async fn maps_catalog(Extension(state): Extension<MapsState>) -> Response {
    let Some(store) = state.store else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Maps endpoint not configured: set MICROMEGAS_MAPS_OBJECT_STORE_URI",
        )
            .into_response();
    };

    let mut entries: Vec<CatalogEntry> = Vec::new();
    let mut list_stream = store.list(None);
    while let Some(item) = list_stream.next().await {
        match item {
            Ok(meta) => {
                let location = meta.location.as_ref();
                if !is_direct_child(location) {
                    continue;
                }
                let Some(display) = location.strip_suffix(GZ_SUFFIX) else {
                    // Non-`.gz` entries are not part of the API surface —
                    // a manual upload that skipped the PUT handler. Skip
                    // it rather than mislead the UI.
                    continue;
                };
                entries.push(CatalogEntry {
                    file: display.to_string(),
                    size: meta.size,
                    last_modified: meta.last_modified,
                });
            }
            Err(e) => {
                error!("listing maps prefix: {e:?}");
                return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to list maps").into_response();
            }
        }
    }

    entries.sort_by(|a, b| a.file.cmp(&b.file));

    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        Json(entries),
    )
        .into_response()
}

/// GET /api/maps/blob/{filename}
///
/// Streams the gzipped object stored at `{filename}.gz`. The axum
/// `{filename}` path capture is a single segment, and
/// `object_store::path::Path` keys are opaque — together those bound the
/// request inside the configured `PrefixStore`. Anything that survives
/// URL routing but still contains a `/` (e.g. percent-encoded forms
/// decoded into the segment) is rejected as a defense-in-depth check.
///
/// The response is always `Content-Type: model/gltf-binary` +
/// `Content-Encoding: gzip` — the suffix on the stored key is the
/// contract; we don't read it back from object metadata.
#[span_fn]
pub async fn maps_blob(
    Extension(state): Extension<MapsState>,
    AxumPath(filename): AxumPath<String>,
) -> Response {
    if !is_direct_child(&filename) {
        return (StatusCode::BAD_REQUEST, "Invalid filename").into_response();
    }

    let Some(store) = state.store else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Maps endpoint not configured: set MICROMEGAS_MAPS_OBJECT_STORE_URI",
        )
            .into_response();
    };

    let key = stored_key(&filename);
    let get_result = match store.get(&ObjectPath::from(key.as_str())).await {
        Ok(r) => r,
        Err(object_store::Error::NotFound { .. }) => {
            return (StatusCode::NOT_FOUND, "Map not found").into_response();
        }
        Err(e) => {
            error!("fetching map blob '{filename}': {e:?}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch map").into_response();
        }
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("model/gltf-binary"),
    );
    headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    headers.insert(header::CONTENT_LENGTH, get_result.meta.size.into());
    // `private`: the route requires cookie auth, so shared caches (CDNs,
    // corporate proxies) must not store and re-serve responses to other users.
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=3600"),
    );

    let stream = get_result
        .into_stream()
        .map(|res| res.map_err(std::io::Error::other));

    (headers, Body::from_stream(stream)).into_response()
}

/// JSON error body returned by the mutation handlers.
#[derive(Debug, Serialize)]
struct MapsErrorBody {
    code: &'static str,
    message: String,
}

fn maps_error(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
    let body = MapsErrorBody {
        code,
        message: message.into(),
    };
    (status, Json(body)).into_response()
}

/// 200 response body for a successful upload. The frontend refetches the
/// catalog after each upload, so the response only needs to confirm what
/// was stored (filename + on-disk byte count); `last_modified` is owned
/// by the object store and surfaces on the next catalog fetch.
#[derive(Debug, Serialize)]
pub struct UploadResponse {
    pub file: String,
    pub size: u64,
}

/// PUT /api/maps/blob/{filename}
///
/// Admin-only. Gzips the body server-side (or accepts a client-gzipped
/// body verbatim) and stores it under `{filename}.gz`. The `.gz` suffix
/// is server-owned: the user-facing filename must NOT end in `.gz`.
#[span_fn]
pub async fn maps_upload(
    Extension(state): Extension<MapsState>,
    // `AdminUser` is `FromRequestParts`, so this runs *before* the `Bytes`
    // body extractor — a non-admin gets 403 without buffering the upload.
    AdminUser(user): AdminUser,
    AxumPath(filename): AxumPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AdminRequired> {
    if !is_direct_child(&filename) || filename.ends_with(GZ_SUFFIX) {
        return Ok(maps_error(
            StatusCode::BAD_REQUEST,
            "INVALID_FILENAME",
            "Filename must be a single path segment and must not end in .gz",
        ));
    }

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if content_type != "model/gltf-binary" {
        return Ok(maps_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "UNSUPPORTED_MEDIA_TYPE",
            "Content-Type must be model/gltf-binary",
        ));
    }

    let Some(store) = state.store else {
        return Ok(maps_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "MAPS_STORE_UNAVAILABLE",
            "Maps endpoint not configured: set MICROMEGAS_MAPS_OBJECT_STORE_URI",
        ));
    };

    // If the client pre-gzipped the body, store it verbatim. Otherwise
    // gzip server-side so the stored object is always a gzip frame and
    // the GET path can stream it untouched with `Content-Encoding: gzip`.
    let client_pregzipped = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("gzip"))
        .unwrap_or(false);

    let stored_bytes: Vec<u8> = if client_pregzipped {
        body.to_vec()
    } else {
        // gzip is CPU-bound and can run for seconds on a 256 MiB upload at
        // the default compression level — offload it to a blocking thread
        // so other futures on this tokio worker keep making progress.
        let to_encode = body;
        let encode_result = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&to_encode)?;
            encoder.finish()
        })
        .await;
        match encode_result {
            Ok(Ok(buf)) => buf,
            Ok(Err(e)) => {
                error!("gzip failed for '{filename}': {e:?}");
                return Ok(maps_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "GZIP_FAILED",
                    "Failed to compress upload",
                ));
            }
            Err(e) => {
                error!("gzip task join failed for '{filename}': {e:?}");
                return Ok(maps_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "GZIP_FAILED",
                    "Failed to compress upload",
                ));
            }
        }
    };

    let stored_size = stored_bytes.len() as u64;
    let key = stored_key(&filename);

    if let Err(e) = store
        .put(&ObjectPath::from(key.as_str()), stored_bytes.into())
        .await
    {
        error!("storing map '{filename}': {e:?}");
        return Ok(maps_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "STORE_FAILED",
            "Failed to store map",
        ));
    }

    info!(
        "Uploaded map '{filename}' ({stored_size} bytes gzipped) by {sub}",
        sub = user.email.as_deref().unwrap_or(&user.subject)
    );

    Ok((
        StatusCode::OK,
        Json(UploadResponse {
            file: filename,
            size: stored_size,
        }),
    )
        .into_response())
}

/// DELETE /api/maps/blob/{filename}
///
/// Admin-only. Idempotent — returns 204 whether or not the object existed,
/// matching S3's native DELETE semantics and avoiding a useless `head()`
/// round-trip just to manufacture a 404.
#[span_fn]
pub async fn maps_delete(
    Extension(state): Extension<MapsState>,
    AdminUser(user): AdminUser,
    AxumPath(filename): AxumPath<String>,
) -> Result<Response, AdminRequired> {
    if !is_direct_child(&filename) {
        return Ok(maps_error(
            StatusCode::BAD_REQUEST,
            "INVALID_FILENAME",
            "Filename must be a single path segment",
        ));
    }

    let Some(store) = state.store else {
        return Ok(maps_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "MAPS_STORE_UNAVAILABLE",
            "Maps endpoint not configured: set MICROMEGAS_MAPS_OBJECT_STORE_URI",
        ));
    };

    let key = stored_key(&filename);
    match store.delete(&ObjectPath::from(key.as_str())).await {
        Ok(()) | Err(object_store::Error::NotFound { .. }) => {
            info!(
                "Deleted map '{filename}' by {sub}",
                sub = user.email.as_deref().unwrap_or(&user.subject)
            );
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        Err(e) => {
            error!("deleting map '{filename}': {e:?}");
            Ok(maps_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DELETE_FAILED",
                "Failed to delete map",
            ))
        }
    }
}
