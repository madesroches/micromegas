# Welcome Page for madesroches.github.io/micromegas

**Branch**: `welcome`
**Status**: Planning

## Goal

Replace the current redirect page at `madesroches.github.io/micromegas/` with a proper welcome/landing page that presents Micromegas as a full observability platform. This broadens the message beyond the analytics web app to cover instrumentation, ingestion, analytics, and all visualization options.

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

Create the `welcome/` directory with Vite + React + TS + Tailwind. Slim copy of brand color CSS variables from `analytics-web-app/src/styles/globals.css` — only the brand palette (`--brand-rust`, `--brand-blue`, `--brand-gold` and dark variants), background/surface colors (`--app-bg`, `--panel-bg`, `--card-bg`), border colors, and text colors. Skip the full shadcn/radix HSL color system (`primary`, `secondary`, `destructive`, etc.) since this is a static landing page, not a component library.

`postcss.config.mjs`: standard `tailwindcss` + `autoprefixer` plugins, nothing else.

`tailwind.config.ts`: `brand.*` and `app.*` color mappings only — no `@tailwindcss/typography` plugin (no prose/markdown content on this page).

Set `"type": "module"` in `package.json` (matches `analytics-web-app`).

Build script: `"build": "tsc && vite build"` (type-check before building, same as `analytics-web-app`).

Pin dependency versions to match `analytics-web-app/` to avoid drift:
- react / react-dom `^18.3.0`
- vite `^6.2.0`
- @vitejs/plugin-react `^4.5.0`
- typescript `^5.4.0`
- tailwindcss `^3.3.0`
- postcss `^8.4.31`
- autoprefixer `^10.4.16`
- lucide-react `^0.292.0`

### 2. Hero

Broaden the hero from the original ("What if all your telemetry lived in one place?") to position Micromegas as a full observability platform:

- Platform-level tagline about unified observability (logs, metrics, traces)
- Sub-tagline: open source, high-performance, cost-efficient
- Logo: inline the SVG markup from `branding/micromegas-icon-transparent.svg` directly into the Hero component (no extra Vite plugin needed). Prefix gradient IDs with `logo-` (e.g., `logo-ring1`) to avoid collisions when inlined into the page.
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

5 cards:

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
- OG meta tags in `index.html`: `og:title`, `og:description`. Skip `og:image` for now (SVG not supported by most platforms — add a pre-rasterized PNG later).

### 9. GitHub Pages Deployment

Update `.github/workflows/publish-docs.yml`:

1. Add build step: `cd welcome && corepack enable && yarn install && yarn build` (ensure yarn is available — Node 20 setup action may not provide it by default)
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

- Branding guide: `branding/extended-palette.md` (full color palette with Van Gogh–inspired naming, CSS variables, TypeScript constants, chart color sequences)
- Brand sheet: `branding/micromegas-brand-sheet.svg` (visual reference with color swatches)
- Brand assets: `branding/` (13 SVG logos including `micromegas-icon-transparent.svg` for hero, `micromegas-social-avatar.svg` for OG image)
- Analytics web app styling: `analytics-web-app/src/styles/globals.css` (80+ CSS variables — use slim subset)
- Analytics web app tailwind config: `analytics-web-app/tailwind.config.ts` (reference for color mappings)
- Existing GitHub Pages workflow: `.github/workflows/publish-docs.yml`
- Current site: `madesroches.github.io/micromegas/` (redirects to `/docs/`)
