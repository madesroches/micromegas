'use client'

import { useEffect } from 'react'
import { useRouter } from 'next/navigation'

export default function HomePage() {
  const router = useRouter()

  useEffect(() => {
    // Use client-side redirect so Next.js basePath is respected
    router.replace('/processes')
  }, [router])

  return null
}
