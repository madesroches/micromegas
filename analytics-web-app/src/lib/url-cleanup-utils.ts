import { type MutableRefObject, useCallback, useEffect } from 'react'
import type { SetURLSearchParams } from 'react-router-dom'
import type { ScreenConfig } from './screens-api'

/** URL params managed by the routing layer (not notebook variables) */
export const RESERVED_URL_PARAMS = new Set(['from', 'to', 'type'])

/**
 * Mutates params: removes `from`/`to` if they match savedConfig's time range.
 * Only removes params that were explicitly present and now match the saved baseline.
 */
export function cleanupTimeParams(
  params: URLSearchParams,
  savedConfig: ScreenConfig,
): void {
  const urlFrom = params.get('from')
  const urlTo = params.get('to')

  if (urlFrom !== null && urlFrom === savedConfig.timeRangeFrom) {
    params.delete('from')
  }
  if (urlTo !== null && urlTo === savedConfig.timeRangeTo) {
    params.delete('to')
  }
}

/**
 * Returns a wrapped handleSave that calls onSave, then applies time cleanup
 * via a single setSearchParams call. Returns null if onSave is null.
 */
export function useDefaultSaveCleanup(
  onSave: (() => Promise<ScreenConfig | null>) | null,
  setSearchParams: SetURLSearchParams,
): (() => Promise<void>) | null {
  const wrapped = useCallback(async (): Promise<void> => {
    if (!onSave) return
    const savedConfig = await onSave()
    if (!savedConfig) return
    setSearchParams(prev => {
      const next = new URLSearchParams(prev)
      cleanupTimeParams(next, savedConfig)
      return next
    })
  }, [onSave, setSearchParams])

  return onSave ? wrapped : null
}

/**
 * Exposes a renderer's wrapped save handler to the parent via a ref.
 * The parent (title bar) calls this ref instead of the raw onSave,
 * so URL cleanup logic in the renderer is always invoked.
 */
export function useExposeSaveRef(
  onSaveRef: MutableRefObject<(() => Promise<void>) | null> | undefined,
  handleSave: (() => Promise<void>) | null,
): void {
  useEffect(() => {
    if (onSaveRef) { onSaveRef.current = handleSave ?? null }
    return () => { if (onSaveRef) { onSaveRef.current = null } }
  }, [onSaveRef, handleSave])
}
