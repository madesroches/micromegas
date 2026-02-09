# Welcome Page for madesroches.github.io/micromegas

**Branch**: `welcome`
**Status**: Planning

## Goal

Replace the current redirect page at `madesroches.github.io/micromegas/` with a proper welcome/landing page that presents Micromegas as a full observability platform. The existing landing page in `../posts/analytics-webapp/landing.html` was scoped to the analytics web app only — this broadens the message to cover instrumentation, ingestion, analytics, and all visualization options.

## Tech Stack

**Vite + React + TypeScript + Tailwind CSS** — same stack as `analytics-web-app/`.

- Yarn (project convention, not npm)
- Lucide React for icons (same as analytics-web-app)
- No react-router — single page
- No web fonts — system font stack
- Builds to static files for GitHub Pages
- Vite `base` set to `/micromegas/` for correct asset paths under GitHub Pages subpath

Rationale: aligning with the existing project stack means shared knowledge of tooling, consistent Tailwind theming, and familiar patterns.

## Project Structure

```
welcome/
├── package.json
├── vite.config.ts
├── tsconfig.json
├── tailwind.config.ts
├── postcss.config.mjs
├── index.html
└── src/
    ├── main.tsx
    ├── App.tsx
    ├── styles/
    │   └── globals.css          # Brand CSS variables
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

### 1. Scaffold

Create the `welcome/` directory with Vite + React + TS + Tailwind. Reuse brand color CSS variables from `analytics-web-app/src/styles/globals.css` (rust `#bf360c`, blue `#1565c0`, gold `#ffb300`, dark bg `#0a0a0f`, panel `#12121a`, card `#1a1a2e`). Key deps: react, react-dom, vite, @vitejs/plugin-react, tailwindcss, postcss, autoprefixer, typescript, lucide-react.

### 2. Hero

Broaden the hero from the original ("What if all your telemetry lived in one place?") to position Micromegas as a full observability platform:

- Platform-level tagline about unified observability (logs, metrics, traces)
- Sub-tagline: open source, high-performance, cost-efficient
- Inline SVG logo from `branding/micromegas-icon-transparent.svg`
- CTA buttons: "Get Started" → `/docs/getting-started/`, "Star on GitHub" → repo
- Dark background, glassmorphism effects from original landing page

### 3. How It Works

New section not in the original. Four-step architecture flow:

1. **Instrument** — Drop-in tracing library for Rust and Unreal Engine. 20ns per event, 100k events/sec.
2. **Ingest** — HTTP ingestion service. Metadata in PostgreSQL, payloads in object storage (S3/GCS).
3. **Analyze** — Apache DataFusion SQL engine via FlightSQL. Standard SQL across all telemetry.
4. **Visualize** — Interactive notebooks, Grafana plugin, Python API.

Horizontal flow with icons and cards.

### 4. Key Differentiators

Expand the original "Built Different" 3-column grid to 4 cards:

- **20ns overhead** — Instrumentation so fast you never turn it off
- **Just SQL** — No PromQL, KQL, or NRQL. Full DataFusion SQL.
- **Object storage pricing** — Raw data on S3/GCS, orders of magnitude cheaper
- **Unified data model** — Logs, metrics, traces queryable together
- **Self-service & shareable** — Accessible web interface designed for self-service exploration. Share any view by copying the URL.

Dark cards with brand-colored accents, glassmorphism.

### 5. Analytics Notebooks

Carry forward the best content from the original landing page:

- SQL cells with syntax highlighting
- Multiple cell types (charts, tables, logs, swimlanes, variables, markdown)
- Drag-to-zoom updates the entire notebook
- URL-encoded state for sharing
- Placeholder wireframes for screenshots (to be replaced with real captures later)

Framed as "one of the ways you interact with your data," not the entire pitch.

### 6. Integrations

New section. 2x2 grid of cards:

- **Analytics Web App** — Interactive notebooks for exploration
- **Grafana Plugin** — Native Grafana data source for dashboards
- **Python API** — Programmatic access via Arrow FlightSQL
- **FlightSQL** — Standard protocol, any compatible client

Each card links to its docs section. Also mention platform support: Rust, Unreal Engine, HTTP gateway.

### 7. Footer + CTA

- Logo, "Open source. Apache 2.0 + MIT."
- Buttons: "Read the Docs" → `/docs/`, "View on GitHub" → repo, "Cost Comparison" → `/docs/cost-effectiveness/`
- Minimal footer: project name, GitHub link, docs link

### 8. Polish

- IntersectionObserver fade-in on scroll
- Responsive: mobile-first, grid → single column on small screens
- Fixed navbar with logo + GitHub/Docs links, glassmorphism (backdrop-filter blur)
- Subtle background effects (gradient mesh or constellation dots)
- All brand colors via CSS variables

### 9. GitHub Pages Deployment

Update `.github/workflows/publish-docs.yml`:

1. Add build step: `cd welcome && yarn install && yarn build`
2. Copy `welcome/dist/*` into `public_docs/` root (replaces the current redirect `index.html`)
3. Existing paths stay intact: `/docs/`, `/rustdoc/`, `/doc/`, `/high-frequency-observability/`, `/unified-observability-for-games/`

The welcome page becomes `madesroches.github.io/micromegas/` while all other content stays at its current URLs.

## Task Dependencies

```
[1. Scaffold]
    ├── [2. Hero]
    ├── [3. How It Works]
    ├── [4. Differentiators]
    ├── [5. Notebooks]
    ├── [6. Integrations]
    └── [7. Footer + CTA]
            └── [8. Polish]
                    └── [9. Deploy]
```

Sections 2-7 can be built in parallel after the scaffold is in place. Polish depends on all sections. Deployment is last.

## Reference Material

- Original landing page: `../posts/analytics-webapp/landing.html` (20 KB, pure HTML/CSS/JS)
- Brand assets: `branding/` (14 SVG logos, extended color palette)
- Analytics web app styling: `analytics-web-app/src/styles/globals.css` (80+ CSS variables)
- Existing GitHub Pages workflow: `.github/workflows/publish-docs.yml`
- Current site: `madesroches.github.io/micromegas/` (redirects to `/docs/`)
