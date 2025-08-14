# Build Guide

This guide covers building Micromegas from source and setting up a development environment.

## Prerequisites

- **[Rust](https://rustup.rs/)** - Latest stable version
- **[Python 3.8+](https://www.python.org/downloads/)**
- **[Docker](https://www.docker.com/get-started/)** - For running PostgreSQL
- **[Git](https://git-scm.com/downloads)**

## Building from Source

### 1. Clone Repository

```bash
git clone https://github.com/madesroches/micromegas.git
cd micromegas
```

### 2. Build Rust Components

```bash
cd rust

# Build all components
cargo build

# Build with optimizations
cargo build --release

# Build specific component
cargo build -p telemetry-ingestion-srv
```

### 3. Run Tests

```bash
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run specific test
cargo test -p micromegas-tracing
```

### 4. Format and Lint

```bash
# Format code (required before commits)
cargo fmt

# Run linter
cargo clippy --workspace -- -D warnings

# Run full CI pipeline
python3 ../build/rust_ci.py
```

## Python Client Development

### 1. Set Up Environment

```bash
cd python/micromegas

# Install Poetry (if not already installed)
curl -sSL https://install.python-poetry.org | python3 -

# Install dependencies
poetry install

# Activate virtual environment
poetry shell
```

### 2. Run Tests

```bash
# Run tests
pytest

# Run with coverage
pytest --cov=micromegas

# Run specific test file
pytest tests/test_client.py
```

### 3. Format Code

```bash
# Format with black (required before commits)
black .

# Check formatting
black --check .
```

## Documentation Development

### 1. Install Dependencies

```bash
# Install MkDocs and theme
pip install -r docs/docs-requirements.txt

# Or use the build script
python docs/build-docs.py
```

### 2. Development Server

```bash
# Start development server
mkdocs serve

# Visit http://localhost:8000
# Changes automatically reload
```

### 3. Build Static Site

```bash
# Build documentation
mkdocs build

# Output in site/ directory
python -m http.server 8000 --directory site
```

## IDE Setup

### VS Code

Recommended extensions:
- **rust-analyzer** - Rust language support
- **Python** - Python language support  
- **Even Better TOML** - TOML file support
- **Error Lens** - Inline error display

### Settings

Add to `.vscode/settings.json`:
```json
{
    "rust-analyzer.cargo.features": "all",
    "python.defaultInterpreterPath": "./python/micromegas/.venv/bin/python",
    "python.formatting.provider": "black"
}
```

## Common Build Tasks

### Full Clean Build

```bash
# Clean Rust build artifacts
cd rust
cargo clean

# Clean Python artifacts
cd ../python/micromegas
poetry env remove --all
poetry install
```

### Release Build

```bash
cd rust

# Build optimized release
cargo build --release

# Run optimized tests
cargo test --release
```

### Cross-Platform Build

```bash
# Add target
rustup target add x86_64-pc-windows-gnu

# Build for target
cargo build --target x86_64-pc-windows-gnu
```

## Troubleshooting

### Common Issues

**Rust compilation errors**:
- Ensure you have the latest stable Rust: `rustup update`
- Clear build cache: `cargo clean`

**Python dependency conflicts**:
- Remove and recreate environment: `poetry env remove --all && poetry install`

**Database connection issues**:
- Ensure PostgreSQL container is running
- Check environment variables are set correctly

**Permission errors on Windows**:
- Run PowerShell as Administrator
- Use Windows Subsystem for Linux (WSL)

### Build Environment

Check your build environment:

```bash
# Rust version
rustc --version

# Cargo version  
cargo --version

# Python version
python --version

# Docker version
docker --version
```

## Performance Builds

### Optimized Release

```bash
cd rust

# Maximum optimization
cargo build --release

# With debug symbols for profiling
cargo build --profile release-debug
```

### Profiling Build

```bash
# Build with profiling
cargo build --profile profiling

# Enable specific features
cargo build --features "profiling,metrics"
```

## Continuous Integration

The CI pipeline runs:

1. **Format check**: `cargo fmt --check`
2. **Linting**: `cargo clippy --workspace -- -D warnings`  
3. **Tests**: `cargo test --workspace`
4. **Documentation**: `cargo doc --workspace`

Run locally with:
```bash
cd rust
python3 ../build/rust_ci.py
```

## Next Steps

- **[Contributing Guide](../contributing.md)** - How to contribute to the project
- **[Getting Started](../getting-started.md)** - Set up a development instance
- **[Architecture Overview](../architecture/index.md)** - Understand the system design
