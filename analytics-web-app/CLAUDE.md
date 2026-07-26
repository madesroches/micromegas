# Analytics Web App (analytics-web-app/)

## Essential Commands (from `analytics-web-app/` directory)
- **IMPORTANT**: Use `yarn`, NOT `npm` (project uses Yarn 4 / Berry via corepack — run `corepack enable` once on a new machine)
- **Install**: `yarn install`
- **Dev**: `yarn dev` (starts Vite dev server on port 3000)
- **Build**: `yarn build` (production build to `dist/`)
- **Lint**: `yarn lint` (REQUIRED before commit)
- **Type check**: `yarn type-check`
- **Test**: `yarn test`
- **Quick start**: `./start_analytics_web.py` (starts both backend and frontend)
- **Backend**: `cd rust && cargo run --bin analytics-web-srv` (runs on port 8000)
