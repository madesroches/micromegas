import { useEffect } from 'react'

const APP_NAME = 'Micromegas'

export function usePageTitle(title: string | undefined | null, busy = false): void {
  useEffect(() => {
    const base = title ? `${title} - ${APP_NAME}` : APP_NAME
    document.title = busy ? `[*] ${base}` : base
  }, [title, busy])
}
