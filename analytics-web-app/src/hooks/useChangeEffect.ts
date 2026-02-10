import { useEffect, useRef } from 'react'

/**
 * Runs a callback when a string value changes, skipping the initial render
 * and the first transition from empty to a value (which is handled by
 * initial-load effects). Only fires on subsequent value-to-value changes.
 * Uses a ref for the callback so the effect only re-fires when the value changes.
 */
export function useChangeEffect(value: string | undefined, callback: () => void) {
  const prevRef = useRef<string | null>(null)
  const callbackRef = useRef(callback)
  callbackRef.current = callback

  useEffect(() => {
    const normalized = value || ''
    if (prevRef.current === null) {
      prevRef.current = normalized
      return
    }
    if (prevRef.current !== normalized) {
      const hadValue = !!prevRef.current
      prevRef.current = normalized
      if (hadValue) {
        callbackRef.current()
      }
    }
  }, [value])
}
