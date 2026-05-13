// Map catalog helpers.
//
// The backend's `/api/maps/catalog` endpoint lists every `*.glb` object
// directly under the configured object-store prefix. Saved notebooks
// store the bare filename in `options.mapUrl`; the viewer composes the
// blob URL at render time.

export interface MapCatalogEntry {
  file: string
  size: number
}

/**
 * Strip a leading `/maps/` from a legacy `mapUrl` so saved notebooks from
 * the static-files era keep loading. Returns the input verbatim if it
 * doesn't start with `/maps/`, or `undefined` for empty/missing input.
 */
export function normalizeMapFilename(raw: string | undefined | null): string | undefined {
  if (!raw) return undefined
  return raw.startsWith('/maps/') ? raw.slice('/maps/'.length) : raw
}

/** Compose the blob URL the renderer hands to drei's `useGLTF`. */
export function resolveMapBlobUrl(file: string | undefined, basePath: string): string | undefined {
  const filename = normalizeMapFilename(file)
  if (!filename) return undefined
  return `${basePath}/api/maps/blob/${filename}`
}

/** Display name: strip the `.glb` extension. Underscores are preserved. */
export function formatMapName(file: string): string {
  return file.replace(/\.glb$/i, '')
}

let catalogPromise: Promise<MapCatalogEntry[]> | null = null

/**
 * Fetch the maps catalog once per tab. Multiple cells share the same
 * in-flight promise; a successful response is cached for the tab's
 * lifetime. Errors clear the cache so the next caller retries — a
 * transient network blip on the first call won't leave every Map cell
 * with an empty dropdown until the user hard-reloads.
 */
export function fetchMapCatalog(basePath: string): Promise<MapCatalogEntry[]> {
  if (!catalogPromise) {
    catalogPromise = fetch(`${basePath}/api/maps/catalog`, { credentials: 'include' })
      .then((res) => {
        if (!res.ok) throw new Error(`catalog fetch returned ${res.status}`)
        return res.json() as Promise<MapCatalogEntry[]>
      })
      .catch(() => {
        catalogPromise = null
        return [] as MapCatalogEntry[]
      })
  }
  return catalogPromise
}

/** Test-only helper: clear the shared promise so tests can re-trigger fetches. */
export function __resetMapCatalogForTest(): void {
  catalogPromise = null
}
