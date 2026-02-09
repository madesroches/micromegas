import { ArrowRight, Github, BookOpen } from 'lucide-react'

export default function Footer() {
  return (
    <footer className="border-t border-theme-border px-6 py-24">
      <div className="mx-auto max-w-6xl text-center">
        {/* CTA */}
        <h2 className="mb-4 text-3xl font-bold text-theme-text-primary sm:text-4xl">
          Get Started
        </h2>
        <p className="mx-auto mb-8 max-w-xl text-theme-text-secondary">
          Open source. Apache 2.0 + MIT dual licensed.
        </p>

        <div className="mb-16 flex flex-wrap items-center justify-center gap-4">
          <a
            href="/micromegas/docs/"
            className="inline-flex items-center gap-2 rounded-lg bg-brand-blue px-6 py-3 text-sm font-medium text-white transition-colors hover:bg-brand-blue-dark"
          >
            <BookOpen size={16} />
            Read the Docs
          </a>
          <a
            href="https://github.com/madesroches/micromegas"
            className="inline-flex items-center gap-2 rounded-lg border border-theme-border px-6 py-3 text-sm font-medium text-theme-text-primary transition-colors hover:border-theme-border-hover hover:bg-app-card/50"
          >
            <Github size={16} />
            View on GitHub
          </a>
          <a
            href="/micromegas/docs/cost-effectiveness/"
            className="inline-flex items-center gap-2 rounded-lg border border-theme-border px-6 py-3 text-sm font-medium text-theme-text-primary transition-colors hover:border-theme-border-hover hover:bg-app-card/50"
          >
            Cost Comparison
            <ArrowRight size={16} />
          </a>
        </div>

      </div>
    </footer>
  )
}
