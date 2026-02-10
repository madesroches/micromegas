import { useEffect, useRef } from 'react'

/**
 * Runs a callback when a string value changes, skipping the initial render.
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
      prevRef.current = normalized
      callbackRef.current()
    }
  }, [value])
}
