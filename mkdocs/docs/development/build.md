# Build Guide

This guide covers building Micromegas from source and setting up a development environment.

## Prerequisites

- **[Rust](https://rustup.rs/)** - Latest stable version
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

# Cross-platform build
rustup target add x86_64-pc-windows-gnu
cargo build --target x86_64-pc-windows-gnu
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

# Clear the build cache
python3 build/dev_worker.py --clear-cache

# Rotate cache: clear, restart, trigger a warming build on main
python3 build/dev_worker.py --rotate-cache
```

### Nightly Cache Rotation

To keep the build cache fresh, add a cron job:

```bash
# crontab -e
0 3 * * * cd /home/madesroches/git/micromegas && python3 build/dev_worker.py --rotate-cache
```

This wipes the cache, restarts the worker, and triggers a full build on main so daytime builds hit a warm cache.

### How It Works

Each workflow has a `check-runner` job that runs on `ubuntu-latest` and decides where the real jobs run:

1. If the build author is the repo owner **and** a dev worker is online, jobs route to `dev-worker`
2. Otherwise, jobs run on `ubuntu-latest` (existing behavior)

The runner container is ephemeral (one job per container) and uses a Docker volume (`micromegas-build-cache`) to persist cargo registry and target directories across builds.

See `tasks/container_based_dev_worker_plan.md` for the full design.

## Next Steps

- **[Contributing Guide](../contributing.md)** - How to contribute to the project
- **[Getting Started](../getting-started.md)** - Set up a development instance
- **[Architecture Overview](../architecture/index.md)** - Understand the system design
