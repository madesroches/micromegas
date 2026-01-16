import { useEffect } from 'react'

const APP_NAME = 'Micromegas'

export function usePageTitle(title: string | undefined | null): void {
  useEffect(() => {
    document.title = title ? `${title} - ${APP_NAME}` : APP_NAME
  }, [title])
}
