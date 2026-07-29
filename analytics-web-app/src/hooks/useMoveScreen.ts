import { useCallback } from 'react'
import { updateScreen, Screen, ScreenApiError } from '@/lib/screens-api'
import { notifyFoldersChanged } from '@/lib/folders-sync'

/** Shared "move a screen to a folder" handler used by both the persistent sidebar and ScreensPage. */
export function useMoveScreen(
  screens: Screen[],
  refresh: () => Promise<void>,
  setActionError: (message: string | null) => void
) {
  return useCallback(
    async (screenName: string, destPath: string) => {
      const screen = screens.find((s) => s.name === screenName)
      if (!screen || screen.folder_path === destPath) return
      setActionError(null)
      try {
        await updateScreen(screenName, { folder_path: destPath })
        notifyFoldersChanged()
        await refresh()
      } catch (err) {
        setActionError(err instanceof ScreenApiError ? `Failed to move: ${err.message}` : 'Failed to move screen')
      }
    },
    [screens, refresh, setActionError]
  )
}
