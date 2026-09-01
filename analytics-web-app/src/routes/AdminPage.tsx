import { Suspense } from 'react'
import { usePageTitle } from '@/hooks/usePageTitle'
import { Database, Download, Upload, Map, KeyRound, ShieldCheck, ShieldBan, Users } from 'lucide-react'
import { PageLayout } from '@/components/layout'
import { AuthGuard } from '@/components/AuthGuard'
import { AppLink } from '@/components/AppLink'
import { useAuth } from '@/lib/auth'

interface AdminCard {
  href: string
  icon: React.ReactNode
  iconBg: string
  iconColor: string
  title: string
  description: string
  /** `true` for a card with no non-admin capability at all -- hidden from a non-admin. */
  adminOnly: boolean
}

function cards(isAdmin: boolean): AdminCard[] {
  return [
    {
      href: '/admin/data-sources',
      icon: <Database className="w-6 h-6" />,
      iconBg: 'bg-green-500/15',
      iconColor: 'text-green-500',
      title: 'Data Sources',
      description: 'Manage FlightSQL server connections used for queries and analytics.',
      adminOnly: true,
    },
    {
      href: '/admin/export-screens',
      icon: <Download className="w-6 h-6" />,
      iconBg: 'bg-accent-link/15',
      iconColor: 'text-accent-link',
      title: 'Export Screens',
      description:
        'Download screen configurations as a JSON file for backup or transfer to another environment.',
      adminOnly: true,
    },
    {
      href: '/admin/import-screens',
      icon: <Upload className="w-6 h-6" />,
      iconBg: 'bg-yellow-500/15',
      iconColor: 'text-yellow-500',
      title: 'Import Screens',
      description:
        'Upload a screens export file to restore or migrate screen configurations into this environment.',
      adminOnly: true,
    },
    {
      href: '/admin/maps',
      icon: <Map className="w-6 h-6" />,
      iconBg: 'bg-rust-500/15',
      iconColor: 'text-accent-link',
      title: 'Maps',
      description: 'Upload and remove GLB map assets served to map cells.',
      adminOnly: true,
    },
    {
      href: '/admin/ingestion-keys',
      icon: <KeyRound className="w-6 h-6" />,
      iconBg: 'bg-orange-500/15',
      iconColor: 'text-orange-500',
      title: 'Ingestion API Keys',
      description: isAdmin
        ? 'Mint, list, and revoke write credentials for telemetry ingestion clients.'
        : 'Mint your own write credentials for telemetry ingestion clients.',
      adminOnly: false,
    },
    {
      href: '/admin/analytics-keys',
      icon: <ShieldCheck className="w-6 h-6" />,
      iconBg: 'bg-purple-500/15',
      iconColor: 'text-purple-500',
      title: 'Analytics API Keys',
      description: 'Mint, list, and revoke read credentials for FlightSQL/analytics access.',
      adminOnly: true,
    },
    {
      href: '/admin/query-deny-list',
      icon: <ShieldBan className="w-6 h-6" />,
      iconBg: 'bg-red-500/15',
      iconColor: 'text-red-500',
      title: 'Query Deny List',
      description: 'Reject a misbehaving query at the FlightSQL service, without a deploy.',
      adminOnly: true,
    },
    {
      href: '/audiences',
      icon: <Users className="w-6 h-6" />,
      iconBg: 'bg-blue-500/15',
      iconColor: 'text-blue-500',
      title: 'Audience Access',
      description: 'See who can read from and mint into each audience, and grant access.',
      adminOnly: false,
    },
  ]
}

function AdminPageContent() {
  usePageTitle('Admin')
  const { user } = useAuth()
  const isAdmin = user?.is_admin ?? false

  const visibleCards = cards(isAdmin).filter((card) => isAdmin || !card.adminOnly)

  return (
    <AuthGuard>
      <PageLayout>
        <div className="p-6 flex flex-col h-full">
          <div className="mb-6">
            <h1 className="text-2xl font-semibold text-theme-text-primary">Admin</h1>
            <p className="mt-1 text-theme-text-secondary">
              {isAdmin
                ? 'System administration and data management tools.'
                : 'Tools you have access to.'}
            </p>
          </div>

          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
            {visibleCards.map((card) => (
              <AppLink key={card.href} href={card.href} className="block">
                <div className="p-6 rounded-xl border border-theme-border bg-app-panel hover:border-accent-link hover:bg-app-card transition-colors">
                  <div
                    className={`w-11 h-11 rounded-lg flex items-center justify-center mb-4 ${card.iconBg} ${card.iconColor}`}
                  >
                    {card.icon}
                  </div>
                  <h3 className="text-base font-semibold text-theme-text-primary mb-1.5">
                    {card.title}
                  </h3>
                  <p className="text-sm text-theme-text-muted leading-relaxed">
                    {card.description}
                  </p>
                </div>
              </AppLink>
            ))}
          </div>
        </div>
      </PageLayout>
    </AuthGuard>
  )
}

export default function AdminPage() {
  return (
    <Suspense
      fallback={
        <AuthGuard>
          <PageLayout>
            <div className="p-6">
              <div className="flex items-center justify-center h-64">
                <div className="animate-spin rounded-full h-8 w-8 border-2 border-accent-link border-t-transparent" />
              </div>
            </div>
          </PageLayout>
        </AuthGuard>
      }
    >
      <AdminPageContent />
    </Suspense>
  )
}
