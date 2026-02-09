# Welcome Page for madesroches.github.io/micromegas

**Branch**: `welcome`
**Status**: Completed
**PR**: #785

## Goal

Replace the current redirect page at `madesroches.github.io/micromegas/` with a proper welcome/landing page that presents Micromegas as a full observability platform.

## Tech Stack

**Vite + React + TypeScript + Tailwind CSS** — same stack as `analytics-web-app/`.

- Yarn (project convention)
- Lucide React for icons
- ESLint matching analytics-web-app config
- No react-router — single page
- No web fonts — system font stack
- `"type": "module"` in package.json
- Build script: `tsc && vite build` (type-check before building)
- Vite `base` set to `/micromegas/` for GitHub Pages subpath

## Project Structure

```
welcome/
├── .eslintrc.json
├── .gitignore
├── package.json
├── vite.config.ts
├── tsconfig.json
├── tailwind.config.ts
├── postcss.config.mjs
├── index.html
├── public/
│   └── screenshots/
│       ├── perf-notebook.png
│       ├── process-list.png
│       ├── metrics-logs.png
│       └── perfetto-trace.png
└── src/
    ├── main.tsx
    ├── App.tsx              # FadeIn wrapper with IntersectionObserver
    ├── styles/
    │   └── globals.css      # Slim brand CSS variables
    └── components/
        ├── Navbar.tsx
        ├── Hero.tsx
        ├── HowItWorks.tsx
        ├── Differentiators.tsx
        ├── Notebooks.tsx
        ├── Integrations.tsx
        └── Footer.tsx
```

## Page Sections

### Navbar
Fixed glassmorphism navbar (`backdrop-blur-lg`) with logo icon + "Micromegas" wordmark, Docs and GitHub links.

### Hero
Large vertical-compact logo (rings + wordmark with glow filter) inlined as SVG. Gradient IDs prefixed with `logo-` to avoid collisions. Tagline: "Unified Observability — Logs, Metrics, Traces". Sub-text about open-source, SQL, cost-efficiency. CTA buttons: "Get Started" → `/docs/getting-started/`, "Star on GitHub" → repo.

### How It Works
Four-step architecture flow: Instrument → Ingest → Analyze → Visualize. Each step has an icon, description, and technical detail.

### Key Differentiators
5 cards: 20ns overhead, Just SQL, Object storage pricing, Unified data model, Self-service & shareable.

### Analytics Notebooks
Real app screenshots (optimized with pngquant+optipng):
- Performance notebook with chart + thread coverage swimlanes
- Process list with SQL query and table output
- Metrics+logs correlation (swimlanes alongside log entries)

Feature list: SQL cells, multiple cell types, drag-to-zoom, share by URL.

### Integrations
2x2 grid linking to actual docs paths:
- Analytics Web App → `/docs/admin/web-app/`
- Grafana Plugin → `/docs/grafana/`
- Python API → `/docs/query-guide/python-api/`
- FlightSQL Protocol → `/docs/query-guide/`

Perfetto trace export showcase with real screenshot.

### Footer
CTA section with "Read the Docs", "View on GitHub", "Cost Comparison" buttons. Apache 2.0 + MIT licensing. Minimal footer links.

### Polish
- IntersectionObserver fade-in on scroll for all sections below Hero
- Responsive mobile-first layout
- Background gradient blur effects in Hero
- OG meta tags (`og:title`, `og:description`); `og:image` deferred (needs rasterized PNG)

## GitHub Pages Deployment

Updated `.github/workflows/publish-docs.yml`:
1. Build step: `cd welcome && yarn install && yarn build`
2. Copy `welcome/dist/*` into `public_docs/` root (replaces redirect `index.html`)
3. PR path trigger includes `welcome/**`
4. Existing paths unaffected: `/docs/`, `/rustdoc/`, `/doc/`, `/high-frequency-observability/`, `/unified-observability-for-games/`
