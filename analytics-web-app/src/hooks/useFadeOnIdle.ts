import { useState, useEffect, useRef } from 'react'

// Brief buffer so the 150ms CSS reveal animation finishes before we
// hand off to the CSS transition-delay (4s in globals.css).
const REVEAL_BUFFER = 200

/**
 * Returns `true` while metadata should be visible, `false` when it should fade.
 * Only reveals on actual status changes (not on mount). Stays revealed during loading.
 * The 4-second visibility window after reveal is handled by CSS transition-delay.
 */
export function useFadeOnIdle(status: string): boolean {
  const [revealed, setRevealed] = useState(false)
  const prevStatusRef = useRef(status)
  const timerRef = useRef<ReturnType<typeof setTimeout>>()

  useEffect(() => {
    if (status !== prevStatusRef.current) {
      setRevealed(true)
      prevStatusRef.current = status
    }
    clearTimeout(timerRef.current)
    if (status !== 'loading') {
      timerRef.current = setTimeout(() => setRevealed(false), REVEAL_BUFFER)
    }
    return () => clearTimeout(timerRef.current)
  }, [status])

  return revealed
}
