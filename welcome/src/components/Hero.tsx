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
        {/* Vertical compact logo — rings + wordmark */}
        <div className="mb-8 flex justify-center">
          <svg xmlns="http://www.w3.org/2000/svg" viewBox="-120 -50 240 160" className="h-64 w-auto sm:h-80">
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
              <filter id="logo-glow" x="-50%" y="-50%" width="200%" height="200%">
                <feGaussianBlur stdDeviation="2" result="coloredBlur" />
                <feMerge>
                  <feMergeNode in="coloredBlur" />
                  <feMergeNode in="SourceGraphic" />
                </feMerge>
              </filter>
            </defs>
            <g transform="translate(0, 0)">
              <ellipse cx="0" cy="0" rx="100" ry="38" fill="none" stroke="url(#logo-ring1)" strokeWidth="2.5" transform="rotate(-20)" opacity="0.9" />
              <ellipse cx="0" cy="0" rx="79" ry="30" fill="none" stroke="url(#logo-ring2)" strokeWidth="2.5" transform="rotate(25)" opacity="0.9" />
              <ellipse cx="0" cy="0" rx="58" ry="22" fill="none" stroke="url(#logo-ring3)" strokeWidth="2.5" transform="rotate(-8)" opacity="0.9" />
              <g filter="url(#logo-glow)">
                <polygon points="0,-12 2,-4 9,-4 3,1 6,9 0,4 -6,9 -3,1 -9,-4 -2,-4" fill="#ffffff" />
              </g>
            </g>
            <text x="0" y="90" textAnchor="middle" fill="#ffffff" fontFamily="system-ui, -apple-system, 'Segoe UI', sans-serif" fontSize="28" fontWeight="300" letterSpacing="4">micromegas</text>
          </svg>
        </div>

        <h1 className="sr-only">Micromegas</h1>
        <p className="mb-4 text-2xl font-medium text-theme-text-secondary sm:text-3xl">
          Unified Observability —{' '}
          <span className="bg-gradient-to-r from-brand-rust via-brand-gold to-brand-blue bg-clip-text text-transparent">
            Logs, Metrics, Traces
          </span>
        </p>

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
