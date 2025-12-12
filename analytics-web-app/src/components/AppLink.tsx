'use client'

import Link, { LinkProps } from 'next/link'
import { ReactNode } from 'react'
import { appLink } from '@/lib/config'

interface AppLinkProps extends Omit<LinkProps, 'href'> {
  href: string
  children: ReactNode
  className?: string
  title?: string
}

/**
 * App-aware Link component that:
 * 1. Prepends the runtime base path to href
 * 2. Disables prefetching (avoids 404s for RSC prefetch in static export)
 *
 * Use this instead of next/link for all internal navigation.
 */
export function AppLink({ href, children, className, title, ...props }: AppLinkProps) {
  return (
    <Link href={appLink(href)} prefetch={false} className={className} title={title} {...props}>
      {children}
    </Link>
  )
}
