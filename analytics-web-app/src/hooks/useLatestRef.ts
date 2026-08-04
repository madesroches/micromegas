import { useRef, useLayoutEffect, type MutableRefObject } from 'react'

/**
 * Returns a ref that always holds the latest `value`, updated via a layout
 * effect (so it lands before any RAF callback, timer, or event handler
 * scheduled off the same commit could read a stale value). Use this to
 * read a fresh callback/value from a ref-based effect without adding the
 * value itself to that effect's dependency array.
 */
export function useLatestRef<T>(value: T): MutableRefObject<T> {
  const ref = useRef(value)
  useLayoutEffect(() => {
    ref.current = value
  })
  return ref
}
