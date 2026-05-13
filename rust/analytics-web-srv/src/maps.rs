//! Map asset endpoints.
//!
//! The catalog is derived by listing the configured prefix; GLB bytes are
//! streamed body-as-stream so the server doesn't buffer 30 MB blobs in RAM
//! per request. All paths are scoped to `MICROMEGAS_MAPS_OBJECT_STORE_URI`.

use anyhow::{Context, Result};
use axum::{
    Extension, Json,
    body::Body,
    extract::Path as AxumPath,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use micromegas::tracing::prelude::*;
use object_store::{Attribute, ObjectStore, path::Path as ObjectPath, prefix::PrefixStore};
use serde::Serialize;
use std::sync::Arc;

/// The maps prefix is reserved for direct children — any subdirectory
/// content is ignored. The configured store is treated as the source of
/// truth: whatever flat files live there are the catalog.
///
/// In practice, axum's `{filename}` path capture is already a single
/// segment (so `/` cannot reach the blob handler from the URL side), and
/// `object_store::path::Path` keys are opaque (no `..` traversal).
/// Filtering out keys that contain `/` keeps the catalog flat and stops
/// nested objects from sneaking into the listing.
pub fn is_direct_child(name: &str) -> bool {
    !name.is_empty() && !name.contains('/')
}

/// Shared state for the maps handlers.
///
/// Held as `Extension<MapsState>` to match the local convention (every
/// existing protected route receives its dependencies through Extensions).
#[derive(Clone)]
pub struct MapsState {
    pub store: Option<Arc<dyn ObjectStore>>,
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

/// Entry returned by the catalog endpoint.
#[derive(Debug, Serialize)]
pub struct CatalogEntry {
    pub file: String,
    pub size: u64,
}

/// GET /api/maps/catalog
///
/// Lists every flat object directly under the configured prefix. The
/// extension is not enforced — the prefix is reserved for map assets, so
/// whatever flat files live there are the catalog. Listing is the only
/// source of truth; there is no separate `maps.json` to keep in sync.
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
                if is_direct_child(location) {
                    entries.push(CatalogEntry {
                        file: location.to_string(),
                        size: meta.size,
                    });
                }
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
/// Streams the named object out of the configured store. The axum
/// `{filename}` path capture is a single segment, and `object_store::path::Path`
/// keys are opaque — together those bound the request inside the configured
/// `PrefixStore`. Anything that survives URL routing but still contains a
/// `/` (e.g. percent-encoded forms decoded into the segment) is rejected
/// as a defense-in-depth check.
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

    let get_result = match store.get(&ObjectPath::from(filename.as_str())).await {
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
    let content_type = get_result
        .attributes
        .get(&Attribute::ContentType)
        .and_then(|v| HeaderValue::from_str(v.as_ref()).ok())
        .unwrap_or_else(|| HeaderValue::from_static("model/gltf-binary"));
    headers.insert(header::CONTENT_TYPE, content_type);

    if let Some(enc) = get_result.attributes.get(&Attribute::ContentEncoding)
        && let Ok(v) = HeaderValue::from_str(enc.as_ref())
    {
        headers.insert(header::CONTENT_ENCODING, v);
    }

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
