import { LayoutDashboard, BarChart3, Code, Plug } from 'lucide-react'
import type { ReactNode } from 'react'

interface IntegrationCardProps {
  icon: ReactNode
  title: string
  description: string
  href: string
}

function IntegrationCard({ icon, title, description, href }: IntegrationCardProps) {
  return (
    <a
      href={href}
      className="group rounded-xl border border-theme-border bg-app-card/50 p-6 backdrop-blur-sm transition-colors hover:border-theme-border-hover"
    >
      <div className="mb-4">{icon}</div>
      <h3 className="mb-2 text-lg font-semibold text-theme-text-primary group-hover:text-brand-blue transition-colors">
        {title}
      </h3>
      <p className="text-sm leading-relaxed text-theme-text-secondary">{description}</p>
    </a>
  )
}

export default function Integrations() {
  return (
    <section className="px-6 py-24">
      <div className="mx-auto max-w-6xl">
        <h2 className="mb-4 text-center text-3xl font-bold text-theme-text-primary sm:text-4xl">
          Integrations
        </h2>
        <p className="mx-auto mb-16 max-w-2xl text-center text-theme-text-secondary">
          Multiple ways to access your telemetry data — from interactive notebooks to programmatic APIs.
        </p>

        <div className="grid grid-cols-1 gap-6 sm:grid-cols-2">
          <IntegrationCard
            icon={<LayoutDashboard size={24} className="text-brand-rust" />}
            title="Analytics Web App"
            description="Interactive notebooks for exploration and investigation. SQL cells, charts, tables, and log viewers in a shareable interface."
            href="/micromegas/docs/analytics-web-app/"
          />
          <IntegrationCard
            icon={<BarChart3 size={24} className="text-brand-blue" />}
            title="Grafana Plugin"
            description="Native Grafana data source for building dashboards. Query your telemetry data alongside other Grafana sources."
            href="/micromegas/docs/grafana-plugin/"
          />
          <IntegrationCard
            icon={<Code size={24} className="text-brand-gold" />}
            title="Python API"
            description="Programmatic access via Arrow FlightSQL. Build custom analysis pipelines with pandas, polars, or any Arrow-compatible library."
            href="/micromegas/docs/python-api/"
          />
          <IntegrationCard
            icon={<Plug size={24} className="text-brand-rust" />}
            title="FlightSQL Protocol"
            description="Standard Apache Arrow FlightSQL protocol. Any compatible client can query your data — no vendor lock-in."
            href="/micromegas/docs/flight-sql/"
          />
        </div>

        {/* Perfetto trace export showcase */}
        <div className="mt-12 overflow-hidden rounded-xl border border-theme-border">
          <div className="border-b border-theme-border bg-app-card/50 px-6 py-3">
            <h3 className="text-sm font-semibold text-theme-text-primary">
              Export traces to Perfetto
            </h3>
            <p className="text-xs text-theme-text-muted">
              One-click export from notebooks into the Perfetto trace viewer for deep span analysis.
            </p>
          </div>
          <img
            src="/micromegas/screenshots/perfetto-trace.png"
            alt="Micromegas trace data visualized in the Perfetto trace viewer"
            className="w-full"
          />
        </div>

        <p className="mt-10 text-center text-sm text-theme-text-muted">
          Platform support: Rust, Unreal Engine, HTTP gateway for any language.
        </p>
      </div>
    </section>
  )
}
