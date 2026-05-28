// Map catalog helpers.
//
// The backend's `/api/maps/catalog` endpoint lists every `*.glb` object
// directly under the configured object-store prefix. Saved notebooks
// store the bare filename in `options.mapUrl`; the viewer composes the
// blob URL at render time.

import { authenticatedFetch } from './api'

export interface MapCatalogEntry {
  file: string
  size: number
  last_modified: string
}

export interface UploadMapResponse {
  file: string
  size: number
}

export interface MapApiErrorBody {
  code?: string
  message?: string
}

export class MapApiError extends Error {
  constructor(
    public code: string,
    message: string,
    public status: number
  ) {
    super(message)
    this.name = 'MapApiError'
  }
}

async function readErrorBody(response: Response): Promise<MapApiErrorBody | undefined> {
  try {
    return (await response.json()) as MapApiErrorBody
  } catch {
    return undefined
  }
}

/**
 * The bare filename for a stored/selected map value.
 *
 * The blob route captures a single path segment and rejects any `/` (see
 * `is_direct_child` / `maps_blob` in the backend), so a valid map filename
 * never contains a slash, and the catalog only ever lists bare filenames.
 * Legacy or hand-authored notebooks sometimes stored a path-prefixed value
 * (e.g. `/maps/Foo.glb`); reduce to the last segment so those still resolve
 * and still match catalog entries. Returns `undefined` when there is no
 * filename segment (empty input or a trailing-slash value).
 */
export function mapFileBasename(file: string | undefined): string | undefined {
  if (!file) return undefined
  return file.split('/').pop() || undefined
}

/** Compose the blob URL the renderer hands to drei's `useGLTF`. */
export function resolveMapBlobUrl(file: string | undefined, basePath: string): string | undefined {
  // Reduce to the basename: a path prefix would otherwise produce a
  // multi-segment URL (`…/blob//maps/Foo.glb`) that misses the blob route
  // and falls through to the SPA's index.html — which the GLB loader then
  // fails to parse as JSON.
  const filename = mapFileBasename(file)
  if (!filename) return undefined
  return `${basePath}/api/maps/blob/${filename}`
}

/** Display name: strip the `.glb` extension. Underscores are preserved. */
export function formatMapName(file: string): string {
  return file.replace(/\.glb$/i, '')
}

let catalogPromise: Promise<MapCatalogEntry[]> | null = null

/**
 * One-shot network fetch — throws on non-OK responses. Used by the admin
 * page where the caller wants to surface failures, and as the inner core
 * of the cached `fetchMapCatalog`.
 */
async function fetchMapCatalogRaw(basePath: string): Promise<MapCatalogEntry[]> {
  const res = await fetch(`${basePath}/api/maps/catalog`, { credentials: 'include' })
  if (!res.ok) throw new Error(`catalog fetch returned ${res.status}`)
  return (await res.json()) as MapCatalogEntry[]
}

/**
 * Strict variant: throws on error. Use this when the caller can surface
 * the failure (e.g. the admin page); the read path of Map cells uses the
 * forgiving `fetchMapCatalog` below.
 */
export function fetchMapCatalogStrict(basePath: string): Promise<MapCatalogEntry[]> {
  return fetchMapCatalogRaw(basePath)
}

/**
 * Fetch the maps catalog once per tab. Multiple cells share the same
 * in-flight promise; a successful response is cached for the tab's
 * lifetime. Errors clear the cache so the next caller retries — a
 * transient network blip on the first call won't leave every Map cell
 * with an empty dropdown until the user hard-reloads.
 */
export function fetchMapCatalog(basePath: string): Promise<MapCatalogEntry[]> {
  if (!catalogPromise) {
    catalogPromise = fetchMapCatalogRaw(basePath).catch(() => {
      catalogPromise = null
      return [] as MapCatalogEntry[]
    })
  }
  return catalogPromise
}

/** Clears the cached catalog so the next `fetchMapCatalog` call re-fetches. */
export function invalidateMapCatalog(): void {
  catalogPromise = null
}

/** Test-only alias kept so existing tests don't need to change. */
export const __resetMapCatalogForTest = invalidateMapCatalog

/**
 * Upload (or replace) a map GLB. The body is sent as the raw bytes from the
 * `File` blob — multipart would buy nothing here. Cached catalog is
 * invalidated on success so the next render sees the new entry.
 */
export async function uploadMap(
  file: File,
  basePath: string
): Promise<UploadMapResponse> {
  const response = await authenticatedFetch(
    `${basePath}/api/maps/blob/${encodeURIComponent(file.name)}`,
    {
      method: 'PUT',
      headers: { 'Content-Type': 'model/gltf-binary' },
      body: file,
    }
  )
  if (!response.ok) {
    // 413 is emitted by axum's DefaultBodyLimit layer before the handler
    // runs, so the body is plaintext rather than the JSON shape mutation
    // handlers produce. Surface a friendlier message for that case.
    if (response.status === 413) {
      throw new MapApiError(
        'TOO_LARGE',
        'Upload exceeds the configured size cap',
        413
      )
    }
    const body = await readErrorBody(response)
    throw new MapApiError(
      body?.code ?? 'UNKNOWN_ERROR',
      body?.message ?? `HTTP ${response.status}`,
      response.status
    )
  }
  invalidateMapCatalog()
  return (await response.json()) as UploadMapResponse
}

/** Delete a map GLB. Idempotent: a missing object is still treated as success. */
export async function deleteMap(filename: string, basePath: string): Promise<void> {
  const response = await authenticatedFetch(
    `${basePath}/api/maps/blob/${encodeURIComponent(filename)}`,
    { method: 'DELETE' }
  )
  if (!response.ok) {
    const body = await readErrorBody(response)
    throw new MapApiError(
      body?.code ?? 'UNKNOWN_ERROR',
      body?.message ?? `HTTP ${response.status}`,
      response.status
    )
  }
  invalidateMapCatalog()
}
