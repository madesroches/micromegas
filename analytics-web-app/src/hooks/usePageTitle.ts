import { useEffect } from 'react'

const APP_NAME = 'Micromegas'

export function usePageTitle(title: string | undefined | null): void {
  useEffect(() => {
    if (title) {
      document.title = `${title} - ${APP_NAME}`
    } else {
      document.title = APP_NAME
    }

    return () => {
      document.title = APP_NAME
    }
  }, [title])
}
