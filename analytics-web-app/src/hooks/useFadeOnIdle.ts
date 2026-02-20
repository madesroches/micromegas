import { useState, useEffect, useRef } from 'react'

const FADE_DELAY = 4000

/**
 * Returns `true` while metadata should be visible, `false` when it should fade.
 * Only reveals on actual status changes (not on mount). Stays revealed during loading.
 */
export function useFadeOnIdle(status: string, delay = FADE_DELAY): boolean {
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
      timerRef.current = setTimeout(() => setRevealed(false), delay)
    }
    return () => clearTimeout(timerRef.current)
  }, [status, delay])

  return revealed
}
