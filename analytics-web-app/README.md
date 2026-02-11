# Analytics Web App

Web application for exploring and analyzing micromegas telemetry data.

## Prerequisites

- Node.js 18+
- Rust 1.70+
- Yarn (`npm install -g yarn`)
- Running micromegas services (PostgreSQL, ingestion, flight-sql)

For Local Query screens (optional):
- `wasm32-unknown-unknown` target: `rustup target add wasm32-unknown-unknown`
- `wasm-bindgen-cli`: `cargo install wasm-bindgen-cli`
- `wasm-opt` (from binaryen): install via your package manager

## Quick Start

The start script handles everything: builds the backend, builds the WASM engine (if needed), installs JS dependencies, and starts both servers with hot reloading.

```bash
# Start micromegas services first
python3 local_test_env/ai_scripts/start_services.py

# Set required environment variables
export MICROMEGAS_DB_USERNAME=telemetry
export MICROMEGAS_DB_PASSWD=<your-password>
export MICROMEGAS_DB_PORT=6432

# Start the web app (from repo root)
python3 analytics-web-app/start_analytics_web.py
```

The frontend runs at http://localhost:3000/mmlocal/ and the backend at http://localhost:8000/mmlocal/.

Use `--disable-auth` to skip OIDC authentication during development.

## Manual Setup

### Backend

```bash
cd rust
cargo run --bin analytics-web-srv -- --port 8000 --disable-auth
```

### WASM Engine (optional, for Local Query screens)

```bash
python3 rust/datafusion-wasm/build.py
```

This compiles DataFusion to WebAssembly and copies the artifacts to `src/lib/datafusion-wasm/`. The `.wasm` binary is not checked into git and must be rebuilt after a fresh clone or whenever the Rust source in `rust/datafusion-wasm/` changes. The quick start script builds it automatically on first run.

### Frontend

```bash
cd analytics-web-app
yarn install
yarn dev
```

The Vite dev server runs on http://localhost:3000 and proxies API requests to the backend.

## Production Build

```bash
cd analytics-web-app
yarn build

cd ../rust
cargo run --bin analytics-web-srv -- --frontend-dir ../analytics-web-app/dist
```

The entire application is served from a single port.

## Commands

| Command | Description |
|---------|-------------|
| `yarn dev` | Start Vite dev server with hot reloading |
| `yarn build` | Production build to `dist/` |
| `yarn lint` | Run ESLint |
| `yarn type-check` | Run TypeScript type checking |
| `yarn test` | Run Jest tests |

## Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `MICROMEGAS_DB_USERNAME` | Yes | - | PostgreSQL username |
| `MICROMEGAS_DB_PASSWD` | Yes | - | PostgreSQL password |
| `MICROMEGAS_DB_PORT` | Yes | - | PostgreSQL port |
| `MICROMEGAS_FLIGHTSQL_URL` | No | `grpc://127.0.0.1:50051` | FlightSQL server address |
| `MICROMEGAS_OIDC_CONFIG` | No | - | OIDC configuration JSON (see below) |
| `MICROMEGAS_BASE_PATH` | No | `/mmlocal` | URL base path |
| `MICROMEGAS_BACKEND_PORT` | No | `8000` | Backend server port |
| `MICROMEGAS_FRONTEND_PORT` | No | `3000` | Frontend dev server port |
| `MICROMEGAS_WEB_CORS_ORIGIN` | No | `http://localhost:3000` | CORS origin |

### OIDC Configuration

Set `MICROMEGAS_OIDC_CONFIG` to enable authentication:

```json
{
  "issuers": [
    {
      "issuer": "https://accounts.google.com",
      "audience": "your-client-id.apps.googleusercontent.com"
    }
  ]
}
```

The `audience` field serves as the OAuth client_id for the authorization code flow. Only a single issuer is supported.