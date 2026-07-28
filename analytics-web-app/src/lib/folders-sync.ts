import { useEffect } from 'react'

const FOLDERS_CHANGED_EVENT = 'micromegas:folders-changed'

/**
 * The persistent sidebar and the Screens page grid each fetch folders
 * independently. Call this after any screen/folder mutation so the other
 * side refreshes instead of showing stale data.
 */
export function notifyFoldersChanged() {
  window.dispatchEvent(new Event(FOLDERS_CHANGED_EVENT))
}

export function useFoldersChangedListener(callback: () => void) {
  useEffect(() => {
    window.addEventListener(FOLDERS_CHANGED_EVENT, callback)
    return () => window.removeEventListener(FOLDERS_CHANGED_EVENT, callback)
  }, [callback])
}
