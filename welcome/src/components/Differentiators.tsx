import { Zap, Terminal, HardDrive, Layers, Share2 } from 'lucide-react'
import type { ReactNode } from 'react'

interface CardProps {
  icon: ReactNode
  title: string
  description: string
}

function Card({ icon, title, description }: CardProps) {
  return (
    <div className="rounded-xl border border-theme-border bg-app-card/50 p-6 backdrop-blur-sm transition-colors hover:border-theme-border-hover">
      <div className="mb-4">{icon}</div>
      <h3 className="mb-2 text-lg font-semibold text-theme-text-primary">{title}</h3>
      <p className="text-sm leading-relaxed text-theme-text-secondary">{description}</p>
    </div>
  )
}

export default function Differentiators() {
  return (
    <section className="px-6 py-24">
      <div className="mx-auto max-w-6xl">
        <h2 className="mb-4 text-center text-3xl font-bold text-theme-text-primary sm:text-4xl">
          Why Micromegas
        </h2>
        <p className="mx-auto mb-16 max-w-2xl text-center text-theme-text-secondary">
          Built from the ground up for high-frequency, cost-efficient observability.
        </p>

        <div className="grid grid-cols-1 gap-6 sm:grid-cols-2 lg:grid-cols-3">
          <Card
            icon={<Zap size={24} className="text-brand-gold" />}
            title="20ns Overhead"
            description="Instrumentation so fast you never turn it off. Capture every event in production without impacting performance."
          />
          <Card
            icon={<Terminal size={24} className="text-brand-blue" />}
            title="Just SQL"
            description="No PromQL, KQL, or NRQL to learn. Full Apache DataFusion SQL across all your telemetry data."
          />
          <Card
            icon={<HardDrive size={24} className="text-brand-rust" />}
            title="Object Storage Pricing"
            description="Raw data lives on S3 or GCS — orders of magnitude cheaper than proprietary observability vendors."
          />
          <Card
            icon={<Layers size={24} className="text-brand-gold" />}
            title="Unified Data Model"
            description="Logs, metrics, and traces stored together and queryable in the same SQL query. No context switching."
          />
          <Card
            icon={<Share2 size={24} className="text-brand-blue" />}
            title="Self-Service & Shareable"
            description="Web interface designed for exploration. Share any view by copying the URL — state encoded right in the link."
          />
        </div>
      </div>
    </section>
  )
}
