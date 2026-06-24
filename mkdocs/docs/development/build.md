# Build Guide

This guide covers building Micromegas from source and setting up a development environment.

## Prerequisites

- **[Rust](https://rustup.rs/)** - Toolchain version pinned in `rust/rust-toolchain.toml` (currently `1.96.0`); `rustup` will install it automatically on first build
- **[Python 3.8+](https://www.python.org/downloads/)**
- **[Docker](https://www.docker.com/get-started/)** - For running PostgreSQL
- **[Git](https://git-scm.com/downloads)**
- **Build tools** - C/C++ compiler and linker (required for Rust compilation)
  - Linux: `sudo apt-get install build-essential clang mold`
  - macOS: `xcode-select --install`
  - Windows: Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/)

!!! note "mold linker requirement"
    On Linux, the project requires the [mold linker](https://github.com/rui314/mold) as configured in `.cargo/config.toml`. This provides faster linking for large projects.

### Additional CI Tools

For running the full CI pipeline locally, you'll need:

```bash
# Install cargo-machete for unused dependency checking
cargo install cargo-machete
```

## Rust Development

### Clone and Build

```bash
git clone https://github.com/madesroches/micromegas.git
cd micromegas/rust

# Build all components
cargo build

# Build with optimizations
cargo build --release

# Build specific component
cargo build -p telemetry-ingestion-srv
```

### Testing

```bash
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run specific test
cargo test -p micromegas-tracing
```

### Format and Lint

```bash
# Format code (required before commits)
cargo fmt

# Run linter
cargo clippy --workspace -- -D warnings

# Run full CI pipeline
python3 ../build/rust_ci.py
```

### Advanced Builds

```bash
# Clean build
cargo clean && cargo build

# Release with debug symbols for profiling
cargo build --profile release-debug

# Profiling build
cargo build --profile profiling

# Cross-compile for Windows (from Linux)
rustup target add x86_64-pc-windows-gnu
cargo build --target x86_64-pc-windows-gnu
```

### ARM64 Cross-Compilation

Production Docker images support ARM64 (aarch64) via a Docker-based cross-compilation environment. The toolchain lives in `local_test_env/arm64/`.

```bash
# Build the ARM64 cross-compilation Docker image
cd local_test_env/arm64
python3 build.py

# Run the image to cross-compile (mounts the repo)
python3 run.py
```

The `Dockerfile` installs `g++-aarch64-linux-gnu`, adds the `aarch64-unknown-linux-gnu` Rust target, and cross-compiles OpenSSL statically for ARM64. The `build_docker_images.py` script at the repo root also accepts `--arm64` to build production images for ARM64.

```bash
python3 build/build_docker_images.py --arm64
```

## JavaScript / TypeScript Development

The repository has two independent JS/TS workspaces. Both use Yarn 4 (Berry) — run `corepack enable` once per machine to activate the pinned version.

### Analytics Web App (`analytics-web-app/`)

Vite + React 19 frontend for the analytics UI.

```bash
cd analytics-web-app

corepack enable     # Once per machine
yarn install        # Install dependencies

yarn dev            # Vite dev server on port 3000
yarn build          # Production build to dist/
yarn lint           # ESLint
yarn type-check     # TypeScript check (no emit)
yarn test           # Jest unit tests
```

### Grafana Plugin (`grafana/`)

Grafana datasource plugin (React frontend + Go backend).

**Additional prerequisites**: Go 1.23+ and `mage` (`go install github.com/magefile/mage@latest`).

```bash
cd grafana

corepack enable     # Once per machine
yarn install        # Install Node dependencies

mage -v build       # Build Go backend binaries
yarn build          # Production bundle
yarn dev            # Dev mode with hot reload
yarn test:ci        # Tests
yarn lint:fix       # Lint + autofix
yarn server         # Start Grafana via docker compose at http://localhost:3000
```

## Python Development

```bash
cd python/micromegas

# Install dependencies
poetry install

# Run tests
pytest

# Format code (required before commits)
black .
```

## Documentation

```bash
# Install dependencies
pip install -r mkdocs/docs-requirements.txt

# Start development server
cd mkdocs
mkdocs serve

# Build static site
mkdocs build
```

## Self-Hosted CI Runner

Developer workstations can contribute to CI builds using a Docker-based self-hosted GitHub Actions runner. Builds from the repo owner route to the dev worker when it's online, falling back to GitHub-hosted runners when it's not.

### Prerequisites

- Docker
- A fine-grained GitHub PAT with `Administration: Read and write` scoped to `madesroches/micromegas`

### Setup

Store the PAT locally (choose one):

```bash
# Option 1: environment variable
export MICROMEGAS_RUNNER_PAT=ghp_xxx

# Option 2: file (recommended for persistent use)
mkdir -p ~/.config/micromegas
echo "ghp_xxx" > ~/.config/micromegas/runner-pat
chmod 600 ~/.config/micromegas/runner-pat
```

The same PAT must be stored as the repository secret `RUNNER_PAT`:

```bash
gh secret set RUNNER_PAT
```

### Usage

```bash
# Start the worker (runs until Ctrl+C)
python3 build/dev_worker.py

# With resource limits
python3 build/dev_worker.py --cpus 8 --memory 16g

# Build the container image without starting the worker
python3 build/dev_worker.py --build-image

# Remove offline dev-worker runners from GitHub and exit
python3 build/dev_worker.py --cleanup
```

### How It Works

Each workflow has a `check-runner` job that runs on `ubuntu-latest` and decides where the real jobs run:

1. If the build author is the repo owner **and** a dev worker is online, jobs route to `dev-worker`
2. Otherwise, jobs run on `ubuntu-latest` (existing behavior)

The runner container is ephemeral: each container registers with GitHub, picks up one job, executes it, and exits. The worker loop then starts a fresh container for the next job. Each container gets a unique name so successive runs cannot collide while Docker drains the previous `--rm` cleanup.

The build caches live in a named Docker volume (`micromegas-runner-cache`) mounted at `/cache`, so they persist across the ephemeral containers. The volume holds:

- Cargo registry and target directories (`CARGO_HOME`, `CARGO_TARGET_DIR`)
- Yarn package downloads (`YARN_CACHE_FOLDER`)
- Go module and build cache (`GOMODCACHE`, `GOCACHE`)
- Playwright browser downloads (`PLAYWRIGHT_BROWSERS_PATH`)

This assumes a single worker per workstation — two concurrent workers sharing the volume would corrupt cargo's locks. To wipe the cache, stop the worker and run `docker volume rm micromegas-runner-cache`.

See `tasks/container_based_dev_worker_plan.md` for the full design.

## Next Steps

- **[Contributing Guide](../contributing.md)** - How to contribute to the project
- **[Getting Started](../getting-started.md)** - Set up a development instance
- **[Architecture Overview](../architecture/index.md)** - Understand the system design
