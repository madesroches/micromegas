import { useEffect, useRef } from 'react'

/**
 * Runs a callback at a fixed interval. Does nothing when intervalMs is 0.
 * Uses a ref for the callback so the timer isn't reset when the callback identity changes.
 */
export function useRefreshInterval(intervalMs: number, onTick: () => void): void {
  const tickRef = useRef(onTick)
  tickRef.current = onTick

  useEffect(() => {
    if (intervalMs <= 0) return
    const id = setInterval(() => tickRef.current(), intervalMs)
    return () => clearInterval(id)
  }, [intervalMs])
}
