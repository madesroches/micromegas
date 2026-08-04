import { useEffect } from 'react'
import { useLatestRef } from './useLatestRef'

/**
 * Runs a callback at a fixed interval, skipping ticks while busy.
 * When isExecuting is true the timer pauses; it resumes (with a fresh
 * interval) once isExecuting flips back to false — matching Grafana's
 * "interval after completion" behaviour.
 */
export function useRefreshInterval(intervalMs: number, isExecuting: boolean, onTick: () => void): void {
  const tickRef = useLatestRef(onTick)

  useEffect(() => {
    if (intervalMs <= 0 || isExecuting) return
    const id = setInterval(() => tickRef.current(), intervalMs)
    return () => clearInterval(id)
  }, [intervalMs, isExecuting, tickRef])
}
