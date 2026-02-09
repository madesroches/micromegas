import { ArrowRight, Star } from 'lucide-react'

export default function Hero() {
  return (
    <section className="relative flex min-h-screen items-center justify-center overflow-hidden px-6 pt-16">
      {/* Background gradient effects */}
      <div className="pointer-events-none absolute inset-0">
        <div className="absolute left-1/4 top-1/4 h-96 w-96 rounded-full bg-brand-blue/10 blur-[128px]" />
        <div className="absolute right-1/4 bottom-1/4 h-96 w-96 rounded-full bg-brand-rust/10 blur-[128px]" />
      </div>

      <div className="relative z-10 mx-auto max-w-4xl text-center">
        {/* Inlined logo SVG with prefixed gradient IDs */}
        <div className="mb-8 flex justify-center">
          <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 200" className="h-28 w-28">
            <defs>
              <linearGradient id="logo-ring1" x1="0%" y1="0%" x2="100%" y2="100%">
                <stop offset="0%" style={{ stopColor: '#bf360c' }} />
                <stop offset="100%" style={{ stopColor: '#8d3a14' }} />
              </linearGradient>
              <linearGradient id="logo-ring2" x1="0%" y1="0%" x2="100%" y2="100%">
                <stop offset="0%" style={{ stopColor: '#1565c0' }} />
                <stop offset="100%" style={{ stopColor: '#0d47a1' }} />
              </linearGradient>
              <linearGradient id="logo-ring3" x1="0%" y1="0%" x2="100%" y2="100%">
                <stop offset="0%" style={{ stopColor: '#ffb300' }} />
                <stop offset="100%" style={{ stopColor: '#e6a000' }} />
              </linearGradient>
            </defs>
            <g transform="translate(100, 100)">
              <ellipse cx="0" cy="0" rx="85" ry="32" fill="none" stroke="url(#logo-ring1)" strokeWidth="3" transform="rotate(-20)" opacity="0.9" />
              <ellipse cx="0" cy="0" rx="67" ry="26" fill="none" stroke="url(#logo-ring2)" strokeWidth="3" transform="rotate(25)" opacity="0.9" />
              <ellipse cx="0" cy="0" rx="50" ry="19" fill="none" stroke="url(#logo-ring3)" strokeWidth="3" transform="rotate(-8)" opacity="0.9" />
              <polygon points="0,-10 2,-3.5 7.5,-3.5 3,0.5 5,7.5 0,4 -5,7.5 -3,0.5 -7.5,-3.5 -2,-3.5" fill="#ffffff" />
            </g>
          </svg>
        </div>

        <h1 className="mb-6 text-5xl font-bold tracking-tight text-theme-text-primary sm:text-6xl lg:text-7xl">
          Unified Observability
          <br />
          <span className="bg-gradient-to-r from-brand-rust via-brand-gold to-brand-blue bg-clip-text text-transparent">
            Logs, Metrics, Traces
          </span>
        </h1>

        <p className="mx-auto mb-10 max-w-2xl text-lg text-theme-text-secondary sm:text-xl">
          Open-source, high-performance observability platform.
          Instrument once, query everything with SQL.
          Cost-efficient storage on S3/GCS.
        </p>

        <div className="flex flex-wrap items-center justify-center gap-4">
          <a
            href="/micromegas/docs/getting-started/"
            className="inline-flex items-center gap-2 rounded-lg bg-brand-blue px-6 py-3 text-sm font-medium text-white transition-colors hover:bg-brand-blue-dark"
          >
            Get Started
            <ArrowRight size={16} />
          </a>
          <a
            href="https://github.com/madesroches/micromegas"
            className="inline-flex items-center gap-2 rounded-lg border border-theme-border px-6 py-3 text-sm font-medium text-theme-text-primary transition-colors hover:border-theme-border-hover hover:bg-app-card/50"
          >
            <Star size={16} />
            Star on GitHub
          </a>
        </div>
      </div>
    </section>
  )
}
