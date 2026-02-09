import { Github, BookOpen } from 'lucide-react'

export default function Navbar() {
  return (
    <nav className="fixed top-0 left-0 right-0 z-50 border-b border-theme-border bg-app-bg/80 backdrop-blur-lg">
      <div className="mx-auto flex max-w-6xl items-center justify-between px-6 py-3">
        <a href="/micromegas/" className="flex items-center gap-2 text-theme-text-primary font-semibold text-lg">
          <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 200" className="h-8 w-8">
            <defs>
              <linearGradient id="nav-ring1" x1="0%" y1="0%" x2="100%" y2="100%">
                <stop offset="0%" style={{ stopColor: '#bf360c' }} />
                <stop offset="100%" style={{ stopColor: '#8d3a14' }} />
              </linearGradient>
              <linearGradient id="nav-ring2" x1="0%" y1="0%" x2="100%" y2="100%">
                <stop offset="0%" style={{ stopColor: '#1565c0' }} />
                <stop offset="100%" style={{ stopColor: '#0d47a1' }} />
              </linearGradient>
              <linearGradient id="nav-ring3" x1="0%" y1="0%" x2="100%" y2="100%">
                <stop offset="0%" style={{ stopColor: '#ffb300' }} />
                <stop offset="100%" style={{ stopColor: '#e6a000' }} />
              </linearGradient>
            </defs>
            <g transform="translate(100, 100)">
              <ellipse cx="0" cy="0" rx="85" ry="32" fill="none" stroke="url(#nav-ring1)" strokeWidth="3" transform="rotate(-20)" opacity="0.9" />
              <ellipse cx="0" cy="0" rx="67" ry="26" fill="none" stroke="url(#nav-ring2)" strokeWidth="3" transform="rotate(25)" opacity="0.9" />
              <ellipse cx="0" cy="0" rx="50" ry="19" fill="none" stroke="url(#nav-ring3)" strokeWidth="3" transform="rotate(-8)" opacity="0.9" />
              <polygon points="0,-10 2,-3.5 7.5,-3.5 3,0.5 5,7.5 0,4 -5,7.5 -3,0.5 -7.5,-3.5 -2,-3.5" fill="#ffffff" />
            </g>
          </svg>
          Micromegas
        </a>
        <div className="flex items-center gap-4">
          <a
            href="/micromegas/docs/"
            className="flex items-center gap-1.5 text-sm text-theme-text-secondary hover:text-theme-text-primary transition-colors"
          >
            <BookOpen size={16} />
            Docs
          </a>
          <a
            href="https://github.com/madesroches/micromegas"
            className="flex items-center gap-1.5 text-sm text-theme-text-secondary hover:text-theme-text-primary transition-colors"
          >
            <Github size={16} />
            GitHub
          </a>
        </div>
      </div>
    </nav>
  )
}
