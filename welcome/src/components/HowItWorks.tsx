import { Code2, Database, Search, BarChart3 } from 'lucide-react'
import type { ReactNode } from 'react'

interface StepProps {
  icon: ReactNode
  number: number
  title: string
  description: string
  detail: string
}

function Step({ icon, number, title, description, detail }: StepProps) {
  return (
    <div className="relative flex flex-col items-center text-center">
      <div className="mb-4 flex h-14 w-14 items-center justify-center rounded-xl border border-theme-border bg-app-card">
        {icon}
      </div>
      <span className="mb-1 text-xs font-medium uppercase tracking-wider text-brand-gold">
        Step {number}
      </span>
      <h3 className="mb-2 text-lg font-semibold text-theme-text-primary">{title}</h3>
      <p className="mb-1 text-sm text-theme-text-secondary">{description}</p>
      <p className="text-xs text-theme-text-muted">{detail}</p>
    </div>
  )
}

export default function HowItWorks() {
  return (
    <section className="px-6 py-24">
      <div className="mx-auto max-w-6xl">
        <h2 className="mb-4 text-center text-3xl font-bold text-theme-text-primary sm:text-4xl">
          How It Works
        </h2>
        <p className="mx-auto mb-16 max-w-2xl text-center text-theme-text-secondary">
          From instrumentation to visualization in four steps.
        </p>

        <div className="grid grid-cols-1 gap-8 sm:grid-cols-2 lg:grid-cols-4">
          <Step
            icon={<Code2 size={24} className="text-brand-rust" />}
            number={1}
            title="Instrument"
            description="Drop-in tracing for Rust and Unreal Engine."
            detail="20ns per event, 100k events/sec"
          />
          <Step
            icon={<Database size={24} className="text-brand-blue" />}
            number={2}
            title="Ingest"
            description="HTTP ingestion service stores everything."
            detail="Metadata in PostgreSQL, payloads in S3/GCS"
          />
          <Step
            icon={<Search size={24} className="text-brand-gold" />}
            number={3}
            title="Analyze"
            description="Query all telemetry with standard SQL."
            detail="Apache DataFusion engine via FlightSQL"
          />
          <Step
            icon={<BarChart3 size={24} className="text-brand-rust" />}
            number={4}
            title="Visualize"
            description="Interactive notebooks, Grafana, Python."
            detail="Multiple ways to explore your data"
          />
        </div>
      </div>
    </section>
  )
}
