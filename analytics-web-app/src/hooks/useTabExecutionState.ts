import { useEffect } from 'react'

export function useTabExecutionState(state: 'idle' | 'busy' | 'error'): void {
  useEffect(() => {
    const link = document.querySelector<HTMLLinkElement>('link[rel="icon"]')
    if (!link) return
    // Capture the idle href once, from the link tag's own resolved URL, so this
    // works regardless of base path / how index.html's relative href resolved.
    const idleHref = link.dataset.idleHref ?? (link.dataset.idleHref = link.href)
    link.href =
      state === 'busy' ? idleHref.replace(/icon\.svg$/, 'icon-busy.svg') :
      state === 'error' ? idleHref.replace(/icon\.svg$/, 'icon-error.svg') :
      idleHref
    // Unmount reset: ScreenPageContent has no persistent layout/Outlet (router.tsx lists
    // `/screen/new` and `/screen/:name` as plain sibling <Route>s), so navigating to any
    // other route (`/processes`, `/screens`, `/admin`, ...) fully unmounts this component
    // without `load()`'s same-component reset (Design §5) ever running. Without this
    // cleanup, a busy/error favicon set here would persist indefinitely on unrelated pages.
    return () => {
      link.href = idleHref
    }
  }, [state])
}
