# Contributing to Micromegas

We welcome contributions to the Micromegas project! Here are some ways you can contribute:

## Reporting Bugs

If you find a bug, please open an issue on our [GitHub Issues page](https://github.com/madesroches/micromegas/issues). Please include:

*   A clear and concise description of the bug.
*   Steps to reproduce the behavior.
*   Expected behavior.
*   Screenshots or error messages if applicable.
*   Your operating system and Micromegas version.

## Suggesting Enhancements

We're always looking for ways to improve Micromegas. If you have an idea for a new feature or an improvement to an existing one, please open an issue on our [GitHub Issues page](https://github.com/madesroches/micromegas/issues). Please include:

*   A clear and concise description of the enhancement.
*   Why you think it would be valuable to the project.
*   Any potential use cases.

## Code Contributions

We welcome code contributions! If you'd like to contribute code, please follow these steps:

1.  **Fork the repository** and clone it to your local machine.
2.  **Create a new branch** for your feature or bug fix: `git checkout -b feature/your-feature-name` or `git checkout -b bugfix/your-bug-fix-name`.
3.  **Make your changes** and ensure your code adheres to the project's coding style and conventions.
4.  **Write tests** for your changes, if applicable.
5.  **Run existing tests** to ensure nothing is broken.
6.  **Commit your changes** with a clear and concise commit message.
7.  **Push your branch** to your forked repository.
8.  **Open a Pull Request** to the `main` branch of the Micromegas repository. Please provide a detailed description of your changes.

## Development Setup

For information on setting up your local development environment, please refer to the [Getting Started](https://madesroches.github.io/micromegas/docs/getting-started/) guide.

## Monorepo Structure

Micromegas uses a monorepo structure with npm workspaces for JavaScript/TypeScript components and Cargo workspaces for Rust components.

### Repository Layout

```
micromegas/
├── rust/                    # Rust workspace (main application)
│   ├── Cargo.toml          # Root Cargo workspace
│   ├── analytics/          # Analytics engine
│   ├── tracing/            # Instrumentation library
│   ├── telemetry-ingestion-srv/
│   ├── flight-sql-srv/
│   └── ...
├── grafana/                # Grafana datasource plugin
│   ├── package.json
│   ├── src/
│   └── pkg/                # Go backend
├── typescript/             # Shared TypeScript packages
│   └── types/              # @micromegas/types package
├── python/                 # Python client
│   └── micromegas/         # Poetry package
├── package.json            # Root npm workspace
└── CONTRIBUTING.md         # This file
```

### Rust Workspace (Primary)

The Rust workspace is located in `rust/` and contains the core Micromegas platform. This is the main workspace of the project.

**Commands** (run from `rust/` directory):
```bash
cargo build              # Build all crates
cargo test               # Run all tests
cargo fmt                # Format code (REQUIRED before commit)
cargo clippy --workspace -- -D warnings  # Lint
```

**CI validation script:**
```bash
python3 build/rust_ci.py    # Runs format check, clippy, and tests (from repo root)
```

### Python Package

The Python client uses Poetry for dependency management.

**Location**: `python/micromegas/`

**Commands** (run from `python/micromegas/`):
```bash
poetry install          # Install dependencies
poetry run pytest       # Run tests
poetry run black <file> # Format code (REQUIRED before commit)
```

### TypeScript/JavaScript Workspaces

The repository uses npm workspaces to manage TypeScript/JavaScript packages, with `yarn` as the package manager.

- **Root workspace** (`package.json`): Defines workspaces and shared dev dependencies
- **`grafana/`**: Grafana FlightSQL datasource plugin (React + Go backend)
- **`typescript/types/`**: Shared TypeScript type definitions (`@micromegas/types`)

**Important**: Always use `yarn`, not `npm`, to avoid lockfile conflicts.

### Working with All Components

#### Installing Dependencies

**Rust** (from `rust/` directory):
```bash
cargo build              # Fetches and compiles Rust dependencies
```

**Python** (from `python/micromegas/` directory):
```bash
poetry install           # Installs Python dependencies
```

**TypeScript/JavaScript** (from repository root, use `yarn`):
```bash
yarn install             # Install all workspace dependencies (Grafana plugin, shared types)
```

**Go** (for Grafana backend, from `grafana/` directory):
```bash
go mod download          # Downloads Go dependencies
```

#### Building Components

**Rust workspace:**
```bash
cd rust && cargo build                   # Build all Rust crates
```

**Python package:**
```bash
cd python/micromegas && poetry install   # Python doesn't need a build step
```

**TypeScript/JavaScript workspaces** (use `yarn`):
```bash
yarn workspaces run build                # Build all workspaces (from root)
cd grafana && yarn build                 # Grafana plugin only
cd typescript/types && yarn build        # Shared types only
```

For the Grafana plugin development:
```bash
cd grafana
yarn build              # Production build
yarn dev                # Development mode with hot reload
```

#### Running Tests

**Rust workspace:**
```bash
cd rust && cargo test                    # All Rust tests
python3 build/rust_ci.py                 # Rust CI validation (from root)
```

**Python package:**
```bash
cd python/micromegas && poetry run pytest  # Python tests
```

**TypeScript/JavaScript workspaces** (use `yarn`):
```bash
yarn workspaces run test                 # Test all workspaces (from root)
cd grafana && yarn test:ci               # Grafana plugin tests only
```

#### Linting

**Rust workspace:**
```bash
cd rust && cargo clippy --workspace -- -D warnings
cd rust && cargo fmt                     # Format (REQUIRED before commit)
```

**Python package:**
```bash
cd python/micromegas && poetry run black .
```

**TypeScript/JavaScript workspaces** (use `yarn`):
```bash
yarn workspaces run lint                 # Lint all workspaces (from root)
cd grafana && yarn lint:fix              # Grafana plugin only
```

### Grafana Plugin Development

The Grafana plugin requires both Node.js and Go:

**Prerequisites:**
- Node.js 16+ (18.20.8 recommended)
- Go 1.23+ (for backend plugin)
- yarn (package manager for this repository)
- mage (for Go builds): `go install github.com/magefile/mage@latest`

**Development workflow:**
```bash
cd grafana

# Install dependencies
yarn install --ignore-engines

# Build Go backend binaries
mage -v build

# Start development server with hot reload
yarn dev

# Run tests
yarn test:ci

# Run linting
yarn lint

# Build production bundle
yarn build
```

**Starting Grafana with the plugin:**
```bash
cd grafana
yarn server             # Starts Grafana with docker compose (includes --build)
# Access Grafana at http://localhost:3000
```

## Code Style and Conventions

### Rust
- Dependencies in alphabetical order in Cargo.toml files
- Use `expect()` with descriptive messages instead of `unwrap()`
- Run `cargo fmt` before any commit
- Use inline format arguments: `format!("value: {variable}")`
- Import proc macros through parent crate: `micromegas_tracing::prelude::*`
- Always use `prelude::*` when importing from prelude modules

### TypeScript/JavaScript
- Follow existing ESLint configuration in each workspace
- Use Prettier for formatting
- Run `npm run lint:fix` before committing
- Prefer functional components and hooks in React code

### Python
- Use Black for formatting (required before commit)
- Follow PEP 8 guidelines
- Use type hints where appropriate

### Commit Messages
- Keep messages clear and concise
- Never include AI-generated credits or co-author tags
- Follow existing commit message patterns in the repository

## Cross-Component Development

When making changes that affect multiple components:

1. **Test Rust first**: Since Rust is the core platform, always test Rust changes first
   ```bash
   cd rust && cargo test
   python3 build/rust_ci.py        # Full CI validation
   ```

2. **Update shared types**: If changing `typescript/types/`, rebuild before testing consumers
   ```bash
   cd typescript/types && yarn build
   ```

3. **Test affected components**: After changing shared dependencies, test all affected components
   ```bash
   cd rust && cargo test                    # Rust workspace
   cd python/micromegas && poetry run pytest  # Python client
   yarn workspaces run test                 # TypeScript/JavaScript workspaces
   ```

4. **Update documentation**: If adding new shared types or APIs, update relevant READMEs

5. **PR guidelines**: When creating PRs that span multiple components:
   - Run `git log --oneline main..HEAD` to review all commits
   - Clearly describe changes in each component
   - Test the integration end-to-end

## Common Issues and Solutions

### Workspace Issues

**Problem**: Workspace dependencies not resolving
```bash
# Solution: Clean node_modules and reinstall (keeps yarn.lock)
rm -rf node_modules
rm -rf grafana/node_modules typescript/*/node_modules
yarn install
```

**Problem**: Peer dependency warnings
```bash
# Solution: Use --ignore-engines flag
yarn install --ignore-engines
```

**Important**: Always use `yarn`, not `npm`, to avoid lockfile conflicts. The repository uses `yarn.lock` for reproducible builds.

### Build Issues

**Problem**: TypeScript errors in Grafana plugin
- Most type errors are inherited from Grafana SDK compatibility
- Check if errors also exist in reference standalone version
- Build should succeed despite some type warnings

**Problem**: Go backend build fails
```bash
# Ensure mage is installed:
go install github.com/magefile/mage@latest

# Run verbose build to see detailed errors:
cd grafana && mage -v build
```

## Adding New Packages

### Adding a TypeScript Workspace Package

1. Create directory under `typescript/`:
   ```bash
   mkdir -p typescript/my-package/src
   ```

2. Create `package.json`:
   ```json
   {
     "name": "@micromegas/my-package",
     "version": "0.1.0",
     "main": "dist/index.js",
     "types": "dist/index.d.ts",
     "scripts": {
       "build": "tsc"
     }
   }
   ```

3. Create `tsconfig.json`

4. Install from root:
   ```bash
   yarn install
   ```

## Testing Requirements

Before submitting a PR, test all affected components:

- [ ] **Rust** (primary): Run `python3 build/rust_ci.py` from repo root (format, clippy, tests)
- [ ] **Python**: Run `poetry run pytest` and `poetry run black .` from `python/micromegas/`
- [ ] **Grafana plugin**: Run `python3 build/grafana_ci.py` from repo root (typecheck, lint, test, build)
- [ ] All builds pass without errors
- [ ] New features include tests
- [ ] Documentation updated if needed

Thank you for contributing to Micromegas!
