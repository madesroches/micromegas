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

## Next Steps

- **[Contributing Guide](../contributing.md)** - How to contribute to the project
- **[Getting Started](../getting-started.md)** - Set up a development instance
- **[Architecture Overview](../architecture/index.md)** - Understand the system design
