'use client'

import { useEffect } from 'react'
import { getConfig } from '@/lib/config'

export default function HomePage() {
  useEffect(() => {
    // Redirect to processes page using runtime base path
    const { basePath } = getConfig()
    window.location.replace(`${basePath}/processes`)
  }, [])

  return null
}
