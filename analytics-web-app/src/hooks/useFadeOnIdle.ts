import { useState, useEffect } from 'react'

/**
 * Returns the CSS class string for fade-on-idle behavior.
 *
 * Adds 'revealed' on any non-idle status change for instant visibility.
 * For loading: stays revealed until status changes.
 * For terminal states: revealed for 200ms (CSS fade-in), then CSS
 * transition-delay (4s) handles the wait before fade-out.
 */
export function useFadeOnIdle(status: string): string {
  const [revealed, setRevealed] = useState(false)

  useEffect(() => {
    if (status === 'idle') return

    setRevealed(true)

    // During loading, stay revealed until status changes again
    if (status === 'loading') return

    // For terminal states, keep revealed 200ms (enough for CSS 150ms fade-in),
    // then remove — CSS transition-delay (4s) handles the wait before fade-out.
    const id = setTimeout(() => setRevealed(false), 200)
    return () => clearTimeout(id)
  }, [status])

  return revealed ? 'fade-on-idle revealed' : 'fade-on-idle'
}
