import { useState, useEffect, useRef } from 'react'

const FADE_DELAY = 4000
const FADE_DURATION = 1000 // must match CSS fade-out duration

type FadeState = 'hidden' | 'revealed' | 'fading'

/**
 * Returns the CSS class string for fade-on-idle behavior.
 *
 * Three states:
 *  - hidden:   opacity 0, 4s CSS delay (handles hover-leave)
 *  - revealed: opacity 1, shown for FADE_DELAY after status changes
 *  - fading:   opacity 0, no CSS delay (immediate 1s fade after JS timer)
 */
export function useFadeOnIdle(status: string): string {
  const [state, setState] = useState<FadeState>('hidden')
  const prevStatusRef = useRef(status)
  const timerRef = useRef<ReturnType<typeof setTimeout>>()
  const fadeTimerRef = useRef<ReturnType<typeof setTimeout>>()

  useEffect(() => {
    if (status !== prevStatusRef.current) {
      setState('revealed')
      prevStatusRef.current = status
    }
    clearTimeout(timerRef.current)
    clearTimeout(fadeTimerRef.current)
    if (status !== 'loading') {
      timerRef.current = setTimeout(() => {
        setState('fading')
        fadeTimerRef.current = setTimeout(() => setState('hidden'), FADE_DURATION)
      }, FADE_DELAY)
    }
    return () => {
      clearTimeout(timerRef.current)
      clearTimeout(fadeTimerRef.current)
    }
  }, [status])

  if (state === 'revealed') return 'fade-on-idle revealed'
  if (state === 'fading') return 'fade-on-idle fading'
  return 'fade-on-idle'
}
