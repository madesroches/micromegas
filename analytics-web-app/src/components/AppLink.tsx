import { Link, LinkProps } from 'react-router-dom'
import { ReactNode } from 'react'
import { appLink } from '@/lib/config'

interface AppLinkProps extends Omit<LinkProps, 'to'> {
  href: string
  children: ReactNode
  className?: string
  title?: string
}

/**
 * App-aware Link component that:
 * 1. Prepends the runtime base path to href
 *
 * Use this instead of react-router-dom Link for all internal navigation.
 */
export function AppLink({ href, children, className, title, ...props }: AppLinkProps) {
  return (
    <Link to={appLink(href)} className={className} title={title} {...props}>
      {children}
    </Link>
  )
}
