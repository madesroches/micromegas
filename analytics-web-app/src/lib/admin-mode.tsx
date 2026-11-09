import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from 'react'

// `/admin` and `/admin/ingestion-keys` are viewable by every authenticated user (role-filtered
// content), so they're deliberately excluded here — only the routes still fully gated by
// `<AuthGuard requireAdmin>` are admin pages.
export const ADMIN_ONLY_PATHS = [
  '/admin/data-sources',
  '/admin/export-screens',
  '/admin/import-screens',
  '/admin/maps',
  '/admin/analytics-keys',
  '/admin/query-deny-list',
  '/admin/view-sets',
  '/admin/groups',
  '/admin/settings',
]

/** Exact path or a sub-path; `/admin/maps-foo` must not match `/admin/maps`. */
export function isAdminRoute(pathname: string): boolean {
  return ADMIN_ONLY_PATHS.some((p) => pathname === p || pathname.startsWith(`${p}/`))
}

export interface AdminModeValue {
  /** `page`: an admin page, always elevated. `toggle`: a notebook where the viewer can opt in. */
  kind: 'none' | 'page' | 'toggle'
  active: boolean
  setActive?: (active: boolean) => void
}

const NONE: AdminModeValue = { kind: 'none', active: false }

const AdminModeContext = createContext<AdminModeValue>(NONE)

interface Toggle {
  active: boolean
  setActive: (active: boolean) => void
}

const AdminModeRegistrationContext = createContext<(toggle: Toggle | null) => void>(() => {})

export function useAdminMode(): AdminModeValue {
  return useContext(AdminModeContext)
}

export function AdminModeProvider({
  pathname,
  isAdmin,
  children,
}: {
  pathname: string
  isAdmin: boolean
  children: ReactNode
}) {
  const [toggle, setToggle] = useState<Toggle | null>(null)
  // A non-admin on an admin route sees only the access-denied card, not an elevated page.
  const onAdminPage = isAdmin && isAdminRoute(pathname)

  const value = useMemo<AdminModeValue>(() => {
    if (onAdminPage) return { kind: 'page', active: true }
    if (toggle) return { kind: 'toggle', active: toggle.active, setActive: toggle.setActive }
    return NONE
  }, [onAdminPage, toggle])

  return (
    <AdminModeRegistrationContext.Provider value={setToggle}>
      <AdminModeContext.Provider value={value}>{children}</AdminModeContext.Provider>
    </AdminModeRegistrationContext.Provider>
  )
}

/** Lets a page offer the admin-mode toggle; the registration is cleared on unmount. */
export function useRegisterAdminModeToggle(
  enabled: boolean,
  active: boolean,
  setActive: (active: boolean) => void
) {
  const register = useContext(AdminModeRegistrationContext)
  useEffect(() => {
    if (!enabled) return
    register({ active, setActive })
    return () => register(null)
  }, [enabled, active, setActive, register])
}

export const ADMIN_COLOR = '#dc2626'

/** Hatched stripe shown above the global header while admin mode is on. */
export const ADMIN_STRIPE_STYLE = {
  backgroundImage: `repeating-linear-gradient(135deg, ${ADMIN_COLOR} 0 6px, #7f1d1d 6px 12px)`,
}
