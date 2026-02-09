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

        <div className="grid grid-cols-1 gap-8 lg:grid-cols-2">
          {/* Notebook wireframe placeholder */}
          <div className="rounded-xl border border-theme-border bg-app-panel overflow-hidden">
            {/* Title bar */}
            <div className="flex items-center gap-2 border-b border-theme-border px-4 py-2">
              <div className="h-3 w-3 rounded-full bg-brand-rust/60" />
              <div className="h-3 w-3 rounded-full bg-brand-gold/60" />
              <div className="h-3 w-3 rounded-full bg-green-500/60" />
              <span className="ml-2 text-xs text-theme-text-muted">notebook.sql</span>
            </div>
            {/* SQL cell */}
            <div className="border-b border-theme-border p-4">
              <div className="mb-2 text-xs font-medium uppercase tracking-wider text-brand-blue">SQL Cell</div>
              <pre className="text-sm text-theme-text-secondary">
                <code>
                  <span className="text-[#1976d2]">SELECT</span> time, level, msg{'\n'}
                  <span className="text-[#1976d2]">FROM</span> log_entries{'\n'}
                  <span className="text-[#1976d2]">WHERE</span> level = <span className="text-[#c3e88d]">'ERROR'</span>{'\n'}
                  <span className="text-[#1976d2]">ORDER BY</span> time <span className="text-[#1976d2]">DESC</span>
                </code>
              </pre>
            </div>
            {/* Chart placeholder */}
            <div className="p-4">
              <div className="mb-2 text-xs font-medium uppercase tracking-wider text-brand-gold">Chart Output</div>
              <div className="flex h-32 items-end gap-1">
                {[40, 65, 35, 80, 55, 70, 45, 90, 60, 50, 75, 85].map((h, i) => (
                  <div
                    key={i}
                    className="flex-1 rounded-t bg-gradient-to-t from-brand-rust to-brand-rust/40"
                    style={{ height: `${h}%` }}
                  />
                ))}
              </div>
            </div>
          </div>

          {/* Feature list */}
          <div className="flex flex-col justify-center space-y-6">
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
