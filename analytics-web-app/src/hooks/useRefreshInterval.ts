import { useEffect, useRef } from 'react'

/**
 * Runs a callback at a fixed interval, skipping ticks while busy.
 * When isExecuting is true the timer pauses; it resumes (with a fresh
 * interval) once isExecuting flips back to false — matching Grafana's
 * "interval after completion" behaviour.
 */
export function useRefreshInterval(intervalMs: number, isExecuting: boolean, onTick: () => void): void {
  const tickRef = useRef(onTick)
  tickRef.current = onTick

  useEffect(() => {
    if (intervalMs <= 0 || isExecuting) return
    const id = setInterval(() => tickRef.current(), intervalMs)
    return () => clearInterval(id)
  }, [intervalMs, isExecuting])
}
