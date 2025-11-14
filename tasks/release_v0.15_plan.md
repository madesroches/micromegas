# Release Plan: Micromegas v0.15.0

## Overview
This document tracks the release of version 0.15.0 of Micromegas, a major milestone that includes:
- **First release of the new `micromegas-auth` crate** with OIDC and API key authentication
- **First release of the Grafana plugin** to the Grafana plugin catalog
- All Rust crates, Python library, and the official Grafana datasource plugin

## Current Status
- **Version**: Currently at 0.15.0 (post v0.14.0 release)
- **Last Release**: v0.14.0 (October 23, 2025)
- **Target**: v0.15.0
- **Branch**: main
- **Outstanding work**: Need to prepare for release

## Pre-Release Checklist

### 1. Code Quality & Testing

#### Rust Workspace (from `/rust` directory)
- [x] Ensure main branch is up to date
- [x] Run full CI pipeline: `python3 ../build/rust_ci.py` ✅ All tests passed (137 tests)
- [x] Ensure all tests pass: `cargo test` ✅ Passed
- [x] Code formatting check: `cargo fmt --check` ✅ Passed
- [x] Lint check: `cargo clippy --workspace -- -D warnings` ✅ Passed
- [x] Build all binaries: `cargo build --release` ✅ Passed

#### Python Package (from `/python/micromegas` directory)
- [x] Run Python tests: `poetry run pytest` ✅ Unit tests passed (33/33), integration tests skipped (require server)
- [x] Python code formatting: `poetry run black .` ✅ All files properly formatted (40 files)
- [x] Verify Python dependencies are up to date ✅ Dependencies verified

#### Grafana Plugin (from `/grafana` directory)
- [x] Install dependencies: `yarn install` ✅ Dependencies installed
- [x] Run linter: `yarn lint:fix` ✅ Linting passed
- [x] Run tests: `yarn test:ci` ✅ All tests passed (47 tests in 5 suites)
- [x] Build plugin: `yarn build` ✅ Build successful
- [x] Verify build artifacts in `dist/` directory ✅ Verified

### 2. Version Verification
Current versions should already be at 0.15.0:
- [x] Verify workspace version in `/rust/Cargo.toml` (should be 0.15.0) ✅ Confirmed: version = "0.15.0"
- [x] Verify Python version in `/python/micromegas/pyproject.toml` (should be 0.15.0) ✅ Confirmed: version = "0.15.0"
- [x] Verify Grafana plugin version in `/grafana/package.json` (should be 0.15.0) ✅ Confirmed: "version": "0.15.0"
- [x] Check that all workspace dependencies reference 0.15.0 ✅ All 9 crates reference 0.15.0
- [x] Verify `micromegas-auth` is included in workspace dependencies ✅ Confirmed: micromegas-auth = { path = "auth", version = "0.15.0" }
- [x] Verify individual crate dependency versions ✅ Confirmed: transit uses ^0.15, tracing uses ^0.15
- [x] Create README for auth crate ✅ Created following pattern of other crates

### 3. Documentation Updates

#### CHANGELOG Updates
- [ ] **Review git log**: `git log --oneline v0.14.0..HEAD` to identify all changes
- [ ] **Update Grafana CHANGELOG**: `/grafana/CHANGELOG.md`
  - [ ] Change version from "1.0.0 (Unreleased)" to "1.0.0 (2025-XX-XX)"
  - [ ] Add comprehensive list of features for initial release:
    - FlightSQL datasource integration
    - SQL query editor with syntax highlighting
    - Query variable support
    - OAuth 2.0 and API key authentication
    - Datasource migration tools
    - Documentation and troubleshooting guides
- [ ] **Create/Update main CHANGELOG.md** at repository root (if exists)
  - [ ] Add new section for v0.15.0 with release date
  - [ ] List all major features, bug fixes, and breaking changes since v0.14.0

#### Major Features to Document (from commits since v0.14.0):
**Authentication & Security:**
- New `micromegas-auth` crate with OIDC and API key support
- HTTP authentication for ingestion service
- OAuth 2.0 client credentials for service accounts
- OIDC authentication support for CLI tools and Python client
- Unified JWKS architecture for service accounts
- Request validation in AuthProvider
- Client IP logging for server observability

**Grafana Plugin (First Release):**
- FlightSQL datasource plugin integration
- OAuth 2.0 authentication support
- Query variable editor and datasource migration tools
- CI/CD pipeline for plugin
- Security updates (Dependabot fixes, SDK updates)
- Comprehensive documentation

**Features & Improvements:**
- gRPC health check endpoint
- Modernized Unreal telemetry sink module
- Documentation improvements and consolidation
- Build reliability improvements

### 4. Grafana Plugin Preparation
- [ ] **Verify plugin.json metadata**:
  - [ ] Version matches package.json (0.15.0)
  - [ ] Author information is correct
  - [ ] Links (documentation, issues) are correct
- [ ] **Test plugin installation locally**:
  - [ ] Install in local Grafana instance
  - [ ] Verify datasource configuration works
  - [ ] Test query execution
  - [ ] Test OAuth 2.0 authentication

### 5. Git Preparation
- [ ] Create release tag: `git tag v0.15.0`
- [ ] Verify tag points to correct commit

## Release Process

### Phase 1: Rust Crates Release
Use the automated release script (from `/build` directory):
```bash
cd /home/mad/micromegas/build
python3 release.py
```

**Crates to publish (in dependency order):**
1. [ ] **micromegas-derive-transit** - Transit derive macros (no internal deps)
2. [ ] **micromegas-tracing-proc-macros** - Tracing procedural macros (no internal deps)
3. [ ] **micromegas-transit** - Data serialization framework (depends on derive-transit)
4. [ ] **micromegas-tracing** - Core tracing library (depends on proc-macros, transit)
5. [ ] **micromegas-auth** - **NEW**: Authentication providers (depends on tracing)
6. [ ] **micromegas-telemetry** - Telemetry data structures (depends on tracing, transit)
7. [ ] **micromegas-ingestion** - Data ingestion utilities (depends on telemetry, tracing, transit)
8. [ ] **micromegas-telemetry-sink** - Telemetry data sinks (depends on telemetry, tracing)
9. [ ] **micromegas-perfetto** - Perfetto trace generation (depends on tracing, transit)
10. [ ] **micromegas-analytics** - Analytics and query engine (depends on ingestion, telemetry, tracing, transit, perfetto)
11. [ ] **micromegas-proc-macros** - Top-level procedural macros (depends on tracing, analytics)
12. [ ] **micromegas** - Main public crate (depends on all others including auth)

**Verification:**
- [ ] Verify all crates are published on crates.io at v0.15.0
- [ ] Specifically verify `micromegas-auth` appears on crates.io

### Phase 2: Python Library Release
From `/python/micromegas` directory:
- [ ] Build package: `poetry build`
- [ ] Publish to PyPI: `poetry publish`
- [ ] Verify package on PyPI: https://pypi.org/project/micromegas/
- [ ] Test installation: `pip install micromegas==0.15.0`

### Phase 3: Grafana Plugin Release
From `/grafana` directory:
- [ ] Create plugin archive: `tar -czf micromegas-datasource-0.15.0.tar.gz dist/`
- [ ] Move archive to release artifacts: `mv micromegas-datasource-0.15.0.tar.gz ../`
- [ ] Verify archive contents are correct

### Phase 4: Git Release
- [ ] Push tags: `git push origin v0.15.0`
- [ ] **Create GitHub release**:
  - [ ] Use tag v0.15.0
  - [ ] Title: "Micromegas v0.15.0 - Authentication & Grafana Plugin"
  - [ ] Include comprehensive description with major features:
    - Highlight new `micromegas-auth` crate
    - Announce first Grafana plugin release
    - List authentication features
    - List all published crates with links
  - [ ] Include installation instructions for:
    - Rust crates
    - Python library
    - Grafana plugin (manual installation)
  - [ ] Attach Grafana plugin archive
  - [ ] Mark as latest release

### Phase 5: Post-Release Version Bump to 0.16.0
Update all versions for next development cycle:

#### Rust Workspace Files:
- [ ] **`/rust/Cargo.toml`**:
  - [ ] Update `[workspace.package].version = "0.16.0"`
  - [ ] Update all workspace dependencies versions to `"0.16.0"`

#### Individual Crate Files:
- [ ] **`/rust/tracing/Cargo.toml`**: Update proc-macros dependency to `^0.16`
- [ ] **`/rust/transit/Cargo.toml`**: Update derive-transit dependency to `^0.16`

#### Python Package:
- [ ] **`/python/micromegas/pyproject.toml`**: Update to `version = "0.16.0"`

#### Grafana Plugin:
- [ ] **`/grafana/package.json`**: Update to `"version": "0.16.0"`

#### Lock Files:
- [ ] Regenerate Rust lock file: `cargo update` (from `/rust` directory)
- [ ] Regenerate Grafana lock file: `yarn install` (from `/grafana` directory)

#### Commit Version Bump:
- [ ] Version bump committed: `git commit -m "Bump version to 0.16.0 for next development cycle"`
- [ ] Push to main branch

## Rollback Plan
If issues are discovered after release:
- [ ] Yank problematic Rust crates: `cargo yank --vers 0.15.0 <crate-name>`
- [ ] Remove problematic Python package version from PyPI (if possible)
- [ ] Update GitHub release notes with issue documentation
- [ ] Prepare patch release v0.15.1 if critical issues found

## Key Features in v0.15.0

### 🆕 New Components

#### micromegas-auth Crate (First Release)
- **OIDC Authentication**: Full OpenID Connect support for user authentication
- **API Key Authentication**: Secure API key validation with constant-time comparison
- **OAuth 2.0 Client Credentials**: Service account support for machine-to-machine auth
- **Multi-Provider Architecture**: Extensible AuthProvider trait with MultiAuthProvider
- **JWKS Integration**: Unified JSON Web Key Set architecture
- **Request Validation**: Comprehensive authentication middleware for HTTP and gRPC services

#### Grafana Plugin (First Release)
- **FlightSQL Datasource**: Native integration with Micromegas analytics server
- **Query Editor**: SQL syntax highlighting and query builder
- **Query Variables**: Support for dashboard template variables
- **OAuth 2.0 Authentication**: Secure authentication with OIDC providers
- **API Key Support**: Alternative authentication method
- **Migration Tools**: Datasource migration utilities
- **CI/CD Pipeline**: Automated build, test, and release workflow
- **Comprehensive Documentation**: Installation, configuration, usage, and troubleshooting guides

### 🔐 Authentication & Security

#### Service Authentication
- HTTP authentication for telemetry ingestion service (#551)
- OIDC authentication support for Rust services and Python client (#548)
- OAuth 2.0 client credentials for service accounts (#552)
- Authentication framework with OIDC and API key support (#546)
- Unified JWKS architecture for service accounts (#547)

#### Security Improvements
- Client IP logging to server observability (#566)
- Request validation in AuthProvider (#571)
- Security vulnerability fixes via Dependabot (#555, #556)

#### CLI & Client Tools
- OIDC authentication support for CLI tools (#549)
- Python client OAuth 2.0 support (#548)

### 🔧 Features & Improvements

#### Infrastructure
- gRPC health check endpoint (#570)
- MultiAuthProvider refactoring for extensibility (#569)
- Fix CI linker crashes and improve build reliability (#572)

#### Unreal Engine
- Modernize Unreal telemetry sink module (#584)

### 📚 Documentation

#### Grafana Plugin Documentation
- Consolidated and streamlined documentation (#559)
- OAuth 2.0 authentication guide for Grafana plugin (#564)
- Comprehensive admin guide with authentication docs (#550)

#### General Documentation
- Documentation links to all Rust crate READMEs (#578)
- Update hosted documentation links (#563)
- Documentation build improvements (#573)
- Build tools installation guide (#582)
- Clean up task documentation (#567)

#### Presentations
- Update high-frequency observability presentation (#574)
- Replace presentation images (#562)

### 🏗️ Code Quality & Maintenance

#### Refactoring
- Refactor OIDC connection to library module (#588)
- Rework AuthProvider to use request validation (#571)
- Refactor MultiAuthProvider for extensibility (#569)

#### Dependency Updates
- Update Grafana plugin SDK to 11.6.7 (#555)
- Fix 28 Dependabot security vulnerabilities (#556)

#### Cleanup
- Clean up presentation files (#568)
- Remove stale files (#557)
- Update README structure (#560, #561, #586)
- Update changelog (#565, #586)

## Dependencies Order (from release.py)
The release script publishes crates in this specific order to respect dependencies:
1. micromegas-derive-transit (no internal deps)
2. micromegas-tracing-proc-macros (no internal deps)
3. micromegas-transit (depends on derive-transit)
4. micromegas-tracing (depends on proc-macros, transit)
5. **micromegas-auth** (depends on tracing) ← **NEW in v0.15.0**
6. micromegas-telemetry (depends on tracing, transit)
7. micromegas-ingestion (depends on telemetry, tracing, transit, auth)
8. micromegas-telemetry-sink (depends on telemetry, tracing)
9. micromegas-perfetto (depends on tracing, transit)
10. micromegas-analytics (depends on ingestion, telemetry, tracing, transit, perfetto)
11. micromegas-proc-macros (depends on tracing, analytics)
12. micromegas (public crate - depends on all others including auth)

## Post-Release Tasks
- [ ] **Announce release**:
  - [ ] GitHub Discussions announcement
  - [ ] Social media posts (if applicable)
  - [ ] Relevant Rust/observability communities
  - [ ] Grafana community (forum, Slack)
- [ ] **Update package registry descriptions** (if needed)
- [ ] **Monitor for issues**:
  - [ ] Watch GitHub issues for bug reports
  - [ ] Check crates.io download stats
- [ ] **Prepare patch release** if critical issues found
- [ ] **Update roadmap** based on v0.15.0 completion

## Grafana Plugin Specific Notes

### Plugin Metadata
- **Name**: micromegas-micromegas-datasource
- **Display Name**: Micromegas
- **Type**: Datasource
- **Version**: 0.15.0 (first public release, internally called 1.0.0)
- **License**: Apache-2.0

### Manual Installation Documentation
For users to install the plugin:
```bash
# Download the plugin archive from GitHub release
curl -LO https://github.com/madesroches/micromegas/releases/download/v0.15.0/micromegas-datasource-0.15.0.tar.gz

# Extract to Grafana plugins directory
mkdir -p /var/lib/grafana/plugins/micromegas-datasource
tar -xzf micromegas-datasource-0.15.0.tar.gz -C /var/lib/grafana/plugins/micromegas-datasource

# Restart Grafana
systemctl restart grafana-server
```

## Emergency Contacts
- Primary: Marc-Antoine Desroches <madesroches@gmail.com>
- Repository: https://github.com/madesroches/micromegas/
- Issues: https://github.com/madesroches/micromegas/issues

## Notes
- All Rust crates use Apache-2.0 license
- All Rust crates target Rust edition 2024
- Python library requires Python ^3.10
- Grafana plugin requires Node.js >=16
- Release script uses `cargo release` with automated publishing
- Grace period of 60 seconds between publishes to allow crates.io indexing
- **This is the first release to include the `micromegas-auth` crate**
- **This is the first public release of the Grafana plugin**

---

## Release Execution Log

### Pre-Release Phase
- [ ] Started: ____
- [ ] Completed: ____

### Release Phase
- [ ] Started: ____
- [ ] Completed: ____

### Post-Release Phase
- [ ] Started: ____
- [ ] Completed: ____

---

**Status**: 📋 Planning phase - ready for execution
- Rust workspace at v0.15.0 (includes new `micromegas-auth` crate)
- Python package at v0.15.0
- Grafana plugin at v0.15.0 (first release)
- 41 commits since v0.14.0 (major authentication and Grafana plugin work)
