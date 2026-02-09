export default function Notebooks() {
  return (
    <section className="px-6 py-24">
      <div className="mx-auto max-w-6xl">
        <h2 className="mb-4 text-center text-3xl font-bold text-theme-text-primary sm:text-4xl">
          Analytics Notebooks
        </h2>
        <p className="mx-auto mb-16 max-w-2xl text-center text-theme-text-secondary">
          One of the ways you interact with your data — interactive notebooks
          that combine SQL, charts, and exploration.
        </p>

        {/* Screenshot showcase */}
        <div className="mb-6 grid grid-cols-1 gap-6 lg:grid-cols-2">
          <div className="overflow-hidden rounded-xl border border-theme-border">
            <img
              src="/micromegas/screenshots/perf-notebook.png"
              alt="Performance analysis notebook with chart, variables, and thread coverage swimlanes"
              className="w-full"
            />
          </div>
          <div className="overflow-hidden rounded-xl border border-theme-border">
            <img
              src="/micromegas/screenshots/process-list.png"
              alt="Process list notebook with SQL query and table output"
              className="w-full"
            />
          </div>
        </div>
        <div className="mb-12 overflow-hidden rounded-xl border border-theme-border">
          <div className="border-b border-theme-border bg-app-card/50 px-6 py-3">
            <h3 className="text-sm font-semibold text-theme-text-primary">
              Metrics and logs, side by side
            </h3>
            <p className="text-xs text-theme-text-muted">
              Correlate thread coverage swimlanes with log entries in the same notebook — no context switching.
            </p>
          </div>
          <img
            src="/micromegas/screenshots/metrics-logs.png"
            alt="Thread coverage swimlanes alongside log entries for easy correlation"
            className="w-full"
          />
        </div>

        {/* Feature list */}
        <div className="grid grid-cols-1 gap-6 sm:grid-cols-2 lg:grid-cols-4">
          <Feature
            title="SQL Cells with Syntax Highlighting"
            description="Write and run SQL queries directly in the notebook with full syntax coloring."
          />
          <Feature
            title="Multiple Cell Types"
            description="Charts, tables, log viewers, swimlanes, variables, and markdown — all in one notebook."
          />
          <Feature
            title="Drag-to-Zoom"
            description="Select a time range on any chart and the entire notebook updates to that window."
          />
          <Feature
            title="Share by URL"
            description="Notebook state is encoded in the URL. Copy and paste to share any view with your team."
          />
        </div>
      </div>
    </section>
  )
}

function Feature({ title, description }: { title: string; description: string }) {
  return (
    <div>
      <h3 className="mb-1 text-base font-semibold text-theme-text-primary">{title}</h3>
      <p className="text-sm leading-relaxed text-theme-text-secondary">{description}</p>
    </div>
  )
}
