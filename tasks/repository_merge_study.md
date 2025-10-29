# Grafana Plugin and Micromegas Repository Merge Study

**Study Status**: ✅ **Phase 1 & 2 COMPLETE** - Phases 3 & 4 deferred pending decision

**Completion Date**: 2025-10-29

**Recommendation**: ✅ **PROCEED with monorepo integration** using npm workspaces

---

## Overview

Study the feasibility and implications of merging the Grafana datasource plugin repository into the main Micromegas monorepo.

**Current State**:
- **Micromegas Repository**: https://github.com/madesroches/micromegas
- **Grafana Plugin Repository**: https://github.com/madesroches/grafana-micromegas-datasource

**Related Work**:
- OAuth 2.0 client credentials support being added to Grafana plugin (see `tasks/auth/grafana_oauth_plan.md`)
- FlightSQL integration completed in `#241` and `#244`

**Fork Context**:
The Grafana plugin is a fork of `influxdata/grafana-flightsql-datasource`, which was archived on May 1, 2024. Among 10 total forks, the Micromegas fork appears to be the most actively maintained, with planned OAuth 2.0 support and Micromegas-specific customizations. Other forks are largely dormant or personal-use only.

## Executive Summary

**Technical Feasibility**: ✅ **HIGH** - Integration is technically sound and offers significant benefits

**Key Findings**:
- Plugin is small (350KB source, 30 commits) and well-structured
- npm workspaces provide sufficient monorepo tooling
- Shared type definitions offer immediate value (ProcessInfo, LogEntry, AuthConfig, etc.)
- Dependency conflicts are manageable (TypeScript 4.4→5.4, future Grafana SDK upgrade)
- Coordinated release strategy aligns with existing Rust/Python release process

**Recommended Approach**:
1. Use **npm workspaces** for TypeScript project management
2. Create shared packages: `@micromegas/types`, `@micromegas/test`, `@micromegas/time`
3. Phased migration: TypeScript upgrade → Workspace setup → Full integration

**Benefits**:
- Single source of truth for data models
- Unified development environment (one clone, one install)
- Atomic commits across plugin and server
- Simplified dependency management
- Easier refactoring and testing

**Challenges**:
- Dual language build (Node.js + Go + Rust)
- Developer onboarding complexity
- CI/CD pipeline redesign
- Maintaining separate version numbers

**Risk Level**: **Low to Medium** - Technical risks are well-understood with clear mitigation strategies

## Goals

1. **Assess Technical Feasibility**: Understand the technical challenges and opportunities of merging repositories
2. **Evaluate Development Workflow**: Analyze impact on CI/CD, release processes, and developer experience
3. **Identify Dependencies**: Map out all external dependencies and version management requirements
4. **Document Trade-offs**: Clear pros/cons analysis for decision making
5. **Create Migration Plan**: If beneficial, outline step-by-step migration strategy

## Current Repository Structure Analysis

### 1. Micromegas Repository

**Location**: `/home/mad/micromegas`

**Structure**:
```
micromegas/
├── rust/                    # Main Rust workspace
│   ├── Cargo.toml          # Workspace manifest
│   ├── analytics/          # Analytics crates
│   ├── telemetry-ingestion-srv/
│   ├── flight-sql-srv/     # FlightSQL server (Grafana integration)
│   ├── analytics-web-srv/  # Web UI for analytics
│   └── public/             # User-facing crates
├── python/                 # Python client
│   └── micromegas/
├── unreal/                 # Unreal Engine integration
├── build/                  # Build scripts
├── docs/                   # Documentation
├── local_test_env/         # Testing utilities
└── tasks/                  # Planning documents
```

**Language Mix**: Rust (primary), Python, C++ (Unreal), TypeScript (analytics-web-app)

**Build Systems**: Cargo (Rust), Poetry (Python), CMake (Unreal), npm (TypeScript)

**CI/CD**: GitHub Actions with language-specific workflows

### 2. Grafana Plugin Repository (To Be Analyzed)

**Location**: https://github.com/madesroches/grafana-micromegas-datasource

**Expected Structure** (to be verified):
```
grafana-micromegas-datasource/
├── src/                    # TypeScript/JavaScript source
│   ├── datasource.ts       # Main datasource implementation
│   ├── components/         # React components
│   └── auth/               # Authentication (OAuth, API keys)
├── pkg/                    # Go backend (if using backend plugin)
├── docker/                 # Docker configuration
├── provisioning/           # Grafana provisioning configs
├── package.json            # npm dependencies
├── tsconfig.json           # TypeScript config
├── webpack.config.js       # Build configuration
└── plugin.json             # Grafana plugin metadata
```

**Language Mix**: TypeScript/JavaScript (frontend), Go (backend)

**Build System**: npm/webpack, Go modules (Mage)

**Release**: GitHub releases (ZIP artifacts), direct installation only (not published to Grafana.com)

## Research Tasks

### Phase 1: Repository Discovery ✅ COMPLETED

#### Task 1.1: Clone and Analyze Grafana Plugin Repository ✅
- [x] Clone repository: `git clone https://github.com/madesroches/grafana-micromegas-datasource`
- [x] Analyze directory structure and file organization
- [x] Document all configuration files (package.json, tsconfig.json, webpack.config.js, etc.)
- [x] Identify all dependencies (npm, Go modules if applicable)
- [x] Review plugin.json metadata and Grafana compatibility requirements

#### Task 1.2: Build System Analysis ✅
- [x] Document build process: npm scripts, webpack configuration
- [x] Identify development dependencies vs production dependencies
- [ ] Test local build: `npm install && npm run build` (not needed for study)
- [x] Analyze output artifacts and their structure
- [x] Document any custom build tools or scripts

#### Task 1.3: Git History Analysis ✅
- [x] Analyze commit history and branching strategy
- [x] Identify contributors and ownership patterns
- [x] Review release tags and versioning scheme
- [x] Check for any git submodules or external dependencies
- [x] Document commit message conventions

#### Task 1.4: CI/CD Pipeline Review ✅
- [x] Document GitHub Actions workflows
- [x] Identify automated tests (unit, integration, e2e)
- [x] Review release automation and artifact publishing
- [x] Analyze Docker build process if applicable
- [x] Check for any external CI/CD services (CircleCI, Travis, etc.)

#### Task 1.5: Documentation Review ✅
- [x] Read all README files and developer guides
- [x] Document setup instructions and prerequisites
- [x] Identify any deployment documentation
- [x] Review API documentation and architecture diagrams
- [x] Check for changelog or release notes

### Phase 2: Integration Analysis ✅ COMPLETED

#### Task 2.1: Shared Dependencies Audit ✅
- [x] List all npm dependencies used by Grafana plugin
- [x] Compare with analytics-web-app dependencies (Next.js project)
- [x] Identify version conflicts or compatibility issues
- [x] Document shared libraries and potential reuse opportunities
- [x] Analyze TypeScript version requirements

#### Task 2.2: Code Coupling Assessment ✅
- [x] Identify API contracts between plugin and FlightSQL server
- [x] Document data models and type definitions
- [x] Check for duplicated code or types (e.g., process metadata, query structures)
- [x] Analyze authentication code overlap (OAuth, API keys)
- [x] Review SQL query generation and Flight protocol usage

#### Task 2.3: Build Tool Compatibility ✅
- [x] Assess npm/webpack compatibility with existing TypeScript projects
- [x] Evaluate potential for shared build configuration
- [x] Check for conflicts in build scripts or tooling
- [x] Analyze bundling requirements (plugin vs web app)
- [x] Document any Grafana-specific build requirements

#### Task 2.4: Testing Infrastructure Review ✅
- [x] Document existing test frameworks (Jest, Playwright, etc.)
- [x] Compare with micromegas testing approach
- [x] Identify test data or fixtures requirements
- [x] Review mocking strategies for FlightSQL integration
- [x] Assess test coverage metrics and goals

#### Task 2.5: Release Process Mapping ✅
- [x] Document Grafana plugin release workflow
- [x] Identify Grafana plugin registry requirements
- [x] Compare with micromegas release process (crates.io, PyPI, etc.)
- [x] Analyze versioning strategy (semantic versioning, plugin version, etc.)
- [x] Document any breaking change handling procedures

### Phase 3: Organizational Impact Analysis

#### Task 3.1: Developer Workflow Impact
- [ ] Assess impact on local development setup
- [ ] Evaluate monorepo tooling needs (workspace management)
- [ ] Consider impact on code review process
- [ ] Analyze IDE/editor configuration requirements
- [ ] Document learning curve for new contributors

#### Task 3.2: CI/CD Redesign Requirements
- [ ] Design unified CI/CD pipeline for all components
- [ ] Identify selective build/test strategies (changed files only)
- [ ] Plan for parallel build execution
- [ ] Estimate CI/CD resource requirements (build time, runner costs)
- [ ] Document failure isolation strategies

#### Task 3.3: Repository Size and Performance
- [ ] Calculate total repository size after merge
- [ ] Estimate git clone time and disk space requirements
- [ ] Analyze git history complexity (number of commits, branches)
- [ ] Consider git LFS requirements for large artifacts
- [ ] Document partial clone strategies if needed

#### Task 3.4: Dependency Management Strategy
- [ ] Evaluate monorepo dependency management tools (Lerna, Nx, Turborepo)
- [ ] Plan for shared dependency version coordination
- [ ] Assess impact of dependency updates across projects
- [ ] Document strategies for handling dependency conflicts
- [ ] Consider vendoring or lock file management

#### Task 3.5: Documentation Consolidation
- [ ] Plan for unified documentation structure
- [ ] Identify overlapping documentation (setup guides, API docs)
- [ ] Design navigation for multi-language documentation
- [ ] Plan for component-specific vs global documentation
- [ ] Consider documentation build and hosting strategy

### Phase 4: Alternative Approaches Analysis

#### Task 4.1: Monorepo Patterns Research
- [ ] Research successful multi-language monorepos (Google, Microsoft, etc.)
- [ ] Evaluate monorepo tools: Bazel, Nx, Turborepo, Rush
- [ ] Document best practices for Rust + TypeScript monorepos
- [ ] Identify anti-patterns and common pitfalls
- [ ] Assess tooling maturity and community support

#### Task 4.2: Git Submodule Approach
- [ ] Evaluate git submodules as alternative to full merge
- [ ] Document submodule workflow and developer experience
- [ ] Identify submodule versioning and update strategies
- [ ] Assess impact on CI/CD with submodules
- [ ] List pros/cons vs full monorepo

#### Task 4.3: Git Subtree Approach
- [ ] Evaluate git subtree as alternative to submodules
- [ ] Test subtree merge with sample repositories
- [ ] Document subtree workflow for synchronized changes
- [ ] Identify subtree update and sync strategies
- [ ] Compare with submodule approach

#### Task 4.4: Polyrepo with Shared Packages
- [ ] Design strategy for shared packages (npm, crates.io)
- [ ] Evaluate version coordination across repositories
- [ ] Plan for breaking change management
- [ ] Assess developer experience with polyrepo
- [ ] Document communication and coordination overhead

#### Task 4.5: Hybrid Approaches
- [ ] Consider partial integration (types only, not full code)
- [ ] Evaluate shared configuration repository
- [ ] Plan for synchronized releases without code merge
- [ ] Document API contract versioning strategies
- [ ] Assess trade-offs of hybrid solutions

## Decision Criteria

### Technical Criteria

1. **Build Performance**: Impact on CI/CD time and local development
2. **Type Safety**: Ability to share types between Rust and TypeScript
3. **Dependency Management**: Complexity of managing multi-language dependencies
4. **Tooling Support**: Quality of monorepo tooling for Rust + TypeScript
5. **Test Isolation**: Ability to run focused tests without cross-contamination

### Organizational Criteria

1. **Developer Experience**: Impact on onboarding and daily workflow
2. **Release Coordination**: Ease of coordinating releases across components
3. **Code Ownership**: Clarity of component ownership in unified repository
4. **Breaking Changes**: Handling breaking changes across components
5. **Community Contributions**: Impact on external contributors

### Operational Criteria

1. **Repository Size**: Total size and clone performance
2. **CI/CD Costs**: Computational and monetary costs of builds
3. **Maintenance Burden**: Ongoing effort to maintain monorepo infrastructure
4. **Rollback Complexity**: Ease of reverting problematic changes
5. **Security**: Vulnerability management and dependency updates

## Expected Deliverables

### 1. Analysis Report
- **Executive Summary**: High-level findings and recommendation
- **Technical Analysis**: Detailed technical feasibility assessment
- **Organizational Impact**: Developer workflow and process changes
- **Cost-Benefit Analysis**: Quantified pros/cons
- **Risk Assessment**: Identified risks and mitigation strategies

### 2. Comparison Matrix
- **Monorepo vs Polyrepo**: Side-by-side comparison
- **Tooling Options**: Evaluation of monorepo management tools
- **Migration Approaches**: Different strategies with pros/cons

### 3. Migration Plan (If Recommended)
- **Phase-by-phase migration**: Step-by-step instructions
- **Rollback Strategy**: How to revert if issues arise
- **Communication Plan**: Team notifications and documentation updates
- **Testing Strategy**: Validation at each migration phase
- **Timeline**: Estimated duration and milestones

### 4. Decision Document
- **Recommendation**: Clear recommendation with rationale
- **Alternative Considered**: Summary of rejected approaches
- **Success Metrics**: How to measure if merge was successful
- **Review Period**: Timeline for reassessing decision

## Preliminary Hypotheses (To Be Validated)

### Potential Benefits of Merging

1. **Shared Type Definitions**: Single source of truth for data models
2. **Synchronized Releases**: Coordinated versioning across components
3. **Code Reuse**: Share authentication code, query builders, etc.
4. **Simplified Development**: Single checkout, unified build commands
5. **Better Testing**: Integration tests spanning plugin and server
6. **Documentation Consolidation**: Single source for all documentation
7. **Atomic Changes**: Single commit updates both plugin and server

### Potential Drawbacks of Merging

1. **Repository Size**: Larger repository, slower clones
2. **Build Complexity**: More complex CI/CD pipeline
3. **Tool Friction**: Rust/TypeScript tooling may conflict
4. **Release Coupling**: Can't release plugin independently from server
5. **Developer Onboarding**: Steeper learning curve for contributors
6. **CI/CD Costs**: Longer build times, higher runner costs
7. **Git History Complexity**: Interleaved history from both projects

### Questions to Answer

1. **Can we maintain separate release cadences in a monorepo?**
2. **What monorepo tooling best supports Rust + TypeScript?**
3. **How do we handle Grafana plugin registry releases from monorepo?**
4. **Can we share TypeScript types between plugin and analytics-web-app?**
5. **What is the performance impact on CI/CD pipelines?**
6. **How do we manage conflicting dependency versions?**
7. **Can we preserve git history from both repositories?**
8. **What is the impact on external contributors?**

## Success Criteria

This study will be considered successful if it delivers:

1. ✅ **Clear Recommendation**: Definitive yes/no on merging with strong rationale
2. ✅ **Quantified Analysis**: Data-driven comparison of approaches
3. ✅ **Risk Mitigation**: Identified risks with concrete mitigation plans
4. ✅ **Actionable Plan**: If merging, a step-by-step migration plan ready to execute
5. ✅ **Team Alignment**: Stakeholder agreement on recommendation and path forward

## Related Resources

- **Grafana Plugin Repository**: https://github.com/madesroches/grafana-micromegas-datasource
- **Micromegas Repository**: https://github.com/madesroches/micromegas
- **OAuth Plan**: `tasks/auth/grafana_oauth_plan.md`
- **FlightSQL Integration**: Commits `08609e02e`, `c2ebb1b9b`
- **Analytics Web App**: `tasks/analytics_web_app/plan.md`
- **Grafana Plugin Development**: https://grafana.com/developers/plugin-tools/

## Notes

- This is a feasibility study, not a commitment to merge
- Focus on objective analysis over predetermined conclusions
- Consider both technical and organizational factors
- Engage with team for feedback throughout process
- Be prepared to recommend "no merge" if that's the best outcome
# Repository Merge Study - Phase 1 Findings

## Executive Summary

Phase 1 analysis has been completed for the grafana-micromegas-datasource repository. The plugin is a well-structured TypeScript + Go backend datasource with clear separation of concerns, automated CI/CD, and active development history.

**Key Finding**: The repository is relatively small (~350KB excluding node_modules), uses standard Grafana plugin tooling, and has minimal custom dependencies that would complicate integration.

## Repository Analysis

### Directory Structure

```
grafana-micromegas-datasource/
├── .config/                 # Build configuration
│   ├── webpack/            # Webpack config (TypeScript)
│   ├── jest/               # Jest test configuration
│   ├── .eslintrc           # ESLint rules
│   └── tsconfig.json       # TypeScript config
├── .github/workflows/       # CI/CD automation
│   ├── ci.yml              # Continuous integration
│   ├── release.yml         # Release automation
│   └── is-compatible.yml   # Compatibility checks
├── cypress/                 # E2E tests
│   └── integration/
├── pkg/                     # Go backend
│   ├── flightsql/          # FlightSQL client implementation
│   │   ├── client.go       # gRPC client
│   │   ├── arrow.go        # Arrow data conversion
│   │   ├── query.go        # Query execution
│   │   ├── query_data.go   # Data handling
│   │   ├── macro.go        # SQL macro expansion
│   │   └── resources.go    # Resource management
│   └── main.go             # Plugin entry point
├── src/                     # TypeScript frontend
│   ├── components/         # React components
│   │   ├── ConfigEditor.tsx    # Datasource configuration UI
│   │   ├── QueryEditor.tsx     # Query builder UI
│   │   ├── BuilderView.tsx     # Visual query builder
│   │   ├── RawEditor.tsx       # Raw SQL editor
│   │   └── VariableQueryEditor.tsx  # Variable support
│   ├── datasource.ts       # Main datasource class
│   ├── types.ts            # TypeScript type definitions
│   ├── module.ts           # Plugin module export
│   └── plugin.json         # Plugin metadata
├── Magefile.go              # Go build tool (Mage)
├── package.json             # npm package manifest
├── go.mod                   # Go module definition
├── tsconfig.json            # Root TypeScript config
├── docker-compose.yaml      # Local development environment
└── build-plugin.sh          # Build script
```

**Structure Observations**:
- Clean separation: TypeScript frontend (src/), Go backend (pkg/)
- Standard Grafana plugin layout
- Configuration in .config/ for easy monorepo adaptation
- Build tools: Mage (Go), Webpack (TS), npm/yarn

### Configuration Files

#### package.json
- **Package name**: `micromegas-datasource`
- **Version**: `0.1.1`
- **Author**: Marc-Antoine Desroches
- **License**: Apache-2.0
- **Node requirement**: `>=16`

**Key Scripts**:
```json
{
  "build": "webpack -c ./.config/webpack/webpack.config.ts --env production",
  "build-plugin": "./build-plugin.sh",
  "dev": "webpack -w -c ./.config/webpack/webpack.config.ts --env development",
  "test": "jest --watch --onlyChanged",
  "test:ci": "jest --maxWorkers 4 --passWithNoTests",
  "typecheck": "tsc --noEmit",
  "lint": "eslint --cache --ignore-path ./.gitignore --ext .js,.jsx,.ts,.tsx .",
  "e2e": "yarn cypress install && yarn grafana-e2e run",
  "server": "docker compose up --build",
  "sign": "npx --yes @grafana/sign-plugin"
}
```

#### plugin.json
- **Plugin ID**: `micromegas-datasource`
- **Plugin name**: Micromegas
- **Type**: datasource
- **Features**: logs, metrics, backend, alerting
- **Executable**: `gpx_flightsql_datasource` (Go binary)
- **Grafana requirement**: `>=9.2.5`
- **Keywords**: datasource, apache, arrow, flight, flightsql, micromegas

#### go.mod
- **Module**: `github.com/madesroches/grafana-micromegas-datasource`
- **Go version**: 1.22 (toolchain 1.23.3)

**Key Go Dependencies**:
```go
github.com/apache/arrow/go/v12 v12.0.0
github.com/grafana/grafana-plugin-sdk-go v0.260.1
github.com/magefile/mage v1.15.0
google.golang.org/grpc v1.67.1
```

### Dependencies Analysis

#### Frontend Dependencies (npm)

**Production Dependencies** (5):
```json
{
  "@emotion/css": "^11.1.3",
  "@grafana/data": "9.4.7",
  "@grafana/runtime": "9.4.7",
  "@grafana/ui": "9.4.7",
  "react": "17.0.2",
  "react-dom": "17.0.2",
  "react-test-renderer": "17.0.2",
  "sql-formatter-plus": "^1.3.6"
}
```

**Dev Dependencies** (35+ packages):
- **Build tools**: webpack 5.76, ts-node, typescript 4.4
- **Testing**: jest 29, @testing-library/react 12, cypress
- **Linting**: eslint 8.26, prettier 2.5
- **Grafana tooling**: @grafana/e2e, @grafana/eslint-config, @grafana/tsconfig

**Dependency Observations**:
- Relatively old Grafana version (9.4.7) - latest is 11.x
- React 17 (not React 18) - matches analytics-web-app
- TypeScript 4.4 - older, analytics-web-app uses TS 5.x
- Standard webpack-based build (not Vite)

#### Backend Dependencies (Go)

**Key Libraries**:
- **Apache Arrow**: v12.0.0 (data interchange)
- **gRPC**: v1.67.1 (communication protocol)
- **Grafana Plugin SDK**: v0.260.1 (plugin framework)
- **Mage**: v1.15.0 (build tool)

**Total Go Dependencies**: ~70 (including transitive)

**Backend Observations**:
- Arrow v12 is older - current is v17+
- Standard Grafana plugin SDK patterns
- Uses Mage instead of standard go build

### Build Process

#### Build Scripts

**1. build-plugin.sh**:
```bash
# Purpose: Build complete plugin (frontend + backend)
# Steps:
#   1. Run Mage to build Go backend
#   2. Run yarn build for frontend
#   3. Package for distribution
```

**2. Magefile.go**:
```go
// Mage targets:
// - build: Compile Go backend for multiple platforms
// - coverage: Run Go tests with coverage
// - clean: Remove build artifacts
```

**3. Webpack Config** (.config/webpack/webpack.config.ts):
- TypeScript-based webpack config
- Supports development and production modes
- Handles SASS, TypeScript compilation
- Copy plugin assets (images, plugin.json)
- LiveReload for development

#### Build Outputs

**Frontend**:
- Location: `dist/`
- Contents: Bundled JavaScript, CSS, plugin.json, images
- Format: Webpack bundle (module.js, plugin.json, assets/)

**Backend**:
- Location: `dist/`
- Binary: `gpx_flightsql_datasource_<os>_<arch>`
- Platforms: linux_amd64, darwin_amd64, windows_amd64

**Plugin Package**:
- Format: ZIP file
- Name: `micromegas-datasource-<version>.zip`
- Contents: dist/ directory with all assets and binaries
- Signing: Optional (requires GRAFANA_API_KEY)

### Git History Analysis

#### Repository Timeline

**Fork Date**: November 8, 2024 (from influxdata/grafana-flightsql-datasource)

**Development Activity**:
```
Nov 8, 2024   - Initial fork, renamed to Micromegas
Dec 2-5, 2024 - Active development (log visualization, time range support)
Dec 9, 2024   - Version 0.1.1 release
Feb 21, 2025  - Grafana 11.1.3 compatibility update
```

**Total Commits**: 30 (including upstream history from InfluxData)

**Micromegas-Specific Commits**: ~20 commits since fork

#### Key Development Milestones

**Initial Setup** (Nov-Dec 2024):
- `548e247` - Initial README for Micromegas
- `2ec8202` - Renamed plugin to micromegas-datasource
- `3f6d7fa` - Enabled log visualization

**Feature Development** (Dec 2024):
- `d7a0933` - Added time filter UI option
- `a9ea4d6` - Backend reads time filter from queries
- `f28c7ed` - Time range metadata for query limiting
- `d1a586d` - Query struct additions
- `3c18614` - Limit SQL query to Grafana's requested row count

**Build & Release** (Dec 2024):
- `8d4a27f` - Build plugin script
- `a208b0d` - Plugin validation script
- `d91609f` - Generate manifest file
- `46d8113` - Version 0.1.1

**Maintenance** (Dec 2024 - Feb 2025):
- `e94ca57` - Fixed reading of SQL info
- `c83c9cc` - Updated Go dependencies
- `3c9634d` - Updated to Grafana 11.1.3 (Feb 2025)

#### Branching Strategy

**Branches**:
- `main` - Primary development branch
- Feature branches with PR workflow (e.g., `time_range`, `validation`, `setup`)

**Release Strategy**:
- Tag-based releases (e.g., `v0.1.1`)
- GitHub releases with ZIP artifacts
- Direct installation only (not published to Grafana.com registry)
- Users install manually from GitHub releases

### CI/CD Workflows

#### 1. Continuous Integration (.github/workflows/ci.yml)

**Trigger**: Push to main, Pull requests to main

**Frontend Steps**:
1. Setup Node.js 16 with yarn cache
2. Install dependencies (`yarn install`)
3. Type checking (`yarn typecheck`)
4. Linting (`yarn lint`)
5. Unit tests (`yarn test:ci`)
6. Build frontend (`yarn build`)
7. E2E tests with Grafana Docker (`yarn e2e`)

**Backend Steps**:
1. Check for backend (Magefile.go exists)
2. Setup Go 1.21
3. Run tests with coverage (`mage coverage`)
4. Build backend (`mage build`)

**Infrastructure**:
- Runner: ubuntu-latest
- Uses docker-compose for local Grafana instance
- Caches: yarn cache, Go modules

#### 2. Release Workflow (.github/workflows/release.yml)

**Trigger**: Version tags (v*)

**Steps**:
1. Build frontend and backend
2. Sign plugin (requires GRAFANA_API_KEY secret)
3. Extract metadata from plugin.json
4. Read changelog for release notes
5. Package plugin as ZIP with MD5 checksum
6. Validate plugin using Grafana's plugincheck2
7. Create GitHub draft release
8. Attach ZIP and MD5 artifacts

**Release Artifacts**:
- `micromegas-datasource-<version>.zip`
- `micromegas-datasource-<version>.zip.md5`

**Distribution**:
- GitHub releases only (not published to Grafana.com)
- Users install manually by downloading ZIP and extracting to Grafana plugins directory
- Simpler release process - no Grafana.com submission required

#### 3. Compatibility Check (.github/workflows/is-compatible.yml)

**Purpose**: Verify plugin compatibility with different Grafana versions

### Code Organization

#### TypeScript Codebase

**datasource.ts** (Main datasource class):
- Extends `DataSourceWithBackend`
- Handles query execution via backend
- Manages datasource configuration
- Implements health checks

**components/**:
- `ConfigEditor.tsx` - Connection settings (host, auth, TLS, metadata)
- `QueryEditor.tsx` - Main query interface with builder/raw toggle
- `BuilderView.tsx` - Visual SQL builder (table, columns, where clauses)
- `RawEditor.tsx` - Raw SQL text editor with syntax highlighting
- `VariableQueryEditor.tsx` - Template variable support
- `QueryHelp.tsx` - Help text and examples

**types.ts**:
- Shared TypeScript interfaces
- Query model, datasource options, configuration

#### Go Codebase

**pkg/flightsql/**:

**client.go** - FlightSQL gRPC client
```go
// Manages gRPC connection to FlightSQL server
// Handles TLS, authentication, metadata
// Connection pooling and retries
```

**arrow.go** - Arrow data conversion
```go
// Converts Arrow Flight data to Grafana data frames
// Handles schema mapping
// Type conversions (Arrow types → Grafana field types)
```

**query.go** - Query execution
```go
// Executes SQL queries via FlightSQL
// Time range handling (from/to variables)
// Result streaming
```

**query_data.go** - Data handling
```go
// Processes query results
// Frame building
// Error handling
```

**macro.go** - SQL macro expansion
```go
// $__timeFilter() macro
// $__from, $__to macros
// Custom macro support
```

**resources.go** - Resource endpoints
```go
// Health check endpoint
// Metadata endpoints
// Table/schema discovery
```

### Testing Infrastructure

#### Frontend Tests

**Framework**: Jest 29 + React Testing Library

**Test Files**:
- `datasource.test.ts` - Datasource class tests
- `ConfigEditor.test.tsx` - Configuration UI tests
- `BuilderView.test.tsx` - Query builder tests
- `utils.test.ts` - Utility function tests

**Coverage**: Basic unit tests, no integration tests visible

#### Backend Tests

**Framework**: Go testing + testify

**Test Files**:
- `flightsql_test.go` - FlightSQL client tests
- `arrow_test.go` - Arrow conversion tests
- `macro_test.go` - SQL macro tests

**Test Execution**: `mage coverage` (run in CI)

#### E2E Tests

**Framework**: Cypress + @grafana/e2e

**Test Files**:
- `cypress/integration/01-smoke.spec.ts` - Basic smoke test

**Execution**: Runs against local Grafana via docker-compose

### Development Environment

**Local Development Stack**:
```yaml
# docker-compose.yaml
services:
  grafana:
    # Grafana instance with plugin auto-loading
    # Mounts local dist/ for hot reload

  # FlightSQL server would be configured separately
```

**Development Workflow**:
1. `yarn dev` - Watch mode webpack build
2. `yarn server` - Start Grafana in Docker
3. Make changes - auto-reload in Grafana
4. `yarn test` - Run tests in watch mode

**Prerequisites**:
- Node.js >=16
- Go 1.21+
- Yarn
- Docker & Docker Compose
- Mage (`go install github.com/magefile/mage`)

## Key Findings

### Strengths

1. **Clean Architecture**: Well-organized codebase with clear separation of concerns
2. **Standard Tooling**: Uses Grafana's recommended build tools and patterns
3. **Good Documentation**: README, DEVELOPMENT.md, inline code comments
4. **Active Development**: Recent commits show ongoing maintenance and feature work
5. **CI/CD Automation**: Comprehensive workflows for testing and releases
6. **Small Size**: Relatively small codebase (~350KB, excluding node_modules)
7. **Type Safety**: Full TypeScript on frontend, Go on backend

### Challenges for Monorepo Integration

1. **Dual Language Build**: Requires both Node.js/yarn and Go build chains
2. **Grafana-Specific Tooling**: Webpack config, plugin SDK, signing process
3. **Coordinated Release Management**: Plugin releases synchronized with Rust/Python releases
4. **Docker Compose Dev Environment**: Local development requires Grafana instance
5. **Dependency Versioning**: Grafana 9.4.7 vs latest (11.x), React 17 vs 18
6. **Release workflow**: Simple GitHub releases (no Grafana.com submission)

### Potential Shared Code

**TypeScript Types**:
- Query structures (SQL queries, time ranges)
- Data models (log entries, metrics)
- Authentication configuration (tokens, credentials)

**Testing Utilities**:
- Mock FlightSQL server
- Test data generators
- Integration test helpers

**Documentation**:
- FlightSQL protocol documentation
- Query syntax examples
- Authentication setup guides

## Dependency Comparison with Micromegas

### TypeScript Dependencies

**Grafana Plugin** uses:
- React 17.0.2
- TypeScript 4.4.0
- Webpack 5.76.0
- Jest 29.3.1
- Grafana SDK 9.4.7

**Analytics Web App** uses (from /home/mad/micromegas/analytics-web-app):
- React 18.3.0
- TypeScript 5.4.0
- Next.js 15.0.0
- (No Grafana dependencies)

**Conflict Analysis**:
- React version mismatch (17 vs 18) - **Minor issue**, both can coexist
- TypeScript version difference (4.4 vs 5.4) - **Potential issue**, may need alignment
- Build tools different (Webpack vs Next.js) - **Separate builds**, no conflict

### Go Dependencies

**Grafana Plugin** uses:
- apache/arrow/go v12.0.0
- grpc v1.67.1
- grafana-plugin-sdk-go v0.260.1

**Micromegas Rust** (no direct Go comparison):
- Rust uses arrow crate (different ecosystem)
- FlightSQL server in Rust, not Go

**No Go overlap** in main Micromegas repository - plugin is only Go codebase.

## Preliminary Recommendations

### Repository Size Impact

**Current Grafana Plugin Size** (excluding node_modules):
- Source code: ~350KB
- Git history: ~30 commits (Micromegas-specific)
- node_modules: ~250MB (gitignored)
- Go modules: ~50MB (cached)

**Impact on Micromegas Repo**:
- Minimal - less than 1% increase in repository size
- Git history relatively shallow (clean fork)
- Most bulk is in gitignored dependencies

### Build Complexity

**Current Micromegas Build**:
```bash
# Rust workspace
cd rust && cargo build

# Python package
cd python/micromegas && poetry install

# Analytics web app
cd analytics-web-app && npm run build
```

**With Grafana Plugin**:
```bash
# Additional build step
cd grafana && yarn install && yarn build-plugin
```

**Complexity Assessment**: **Low to Medium**
- Grafana plugin has self-contained build scripts
- No cross-dependencies with other Micromegas components
- Could be isolated workspace in monorepo

### Development Workflow Impact

**Current**: Separate repos, separate clones, manual coordination

**Monorepo Benefits**:
- Single clone for all components
- Atomic commits across plugin and server
- Shared types and utilities
- Unified versioning possible

**Monorepo Challenges**:
- Need monorepo tooling (workspace management)
- CI/CD needs selective builds (changed files only)
- Plugin has Go backend (adds build complexity)
- Developers need both Node.js and Go setup

## Next Steps for Phase 2

### Integration Analysis Tasks

1. **Shared Type Definitions**
   - Identify overlapping types between plugin and FlightSQL server
   - Design shared type package strategy
   - Evaluate TypeScript → Rust code generation

2. **Build Tool Evaluation**
   - Test monorepo tools: Nx, Turborepo, Lerna
   - Evaluate workspace management in package.json
   - Design unified build command strategy

3. **CI/CD Pipeline Design**
   - Selective build strategy (only changed components)
   - Grafana plugin release automation
   - Coordinated versioning across components

4. **Dependency Conflict Resolution**
   - Align TypeScript versions (4.4 → 5.x)
   - Evaluate Grafana SDK upgrade path (9.4.7 → 11.x)
   - React version compatibility (17 vs 18)

5. **Development Environment**
   - Unified development scripts
   - Docker Compose integration for all services
   - Hot reload across all components

### Questions to Answer in Phase 2

1. Can we use workspace features in yarn/npm to manage both plugin and web app?
2. How do we handle Grafana plugin signing in monorepo CI/CD?
3. Should we version components independently or in lockstep?
4. Can we share a common TypeScript configuration?
5. How do we handle the different React versions?
6. What's the migration path for dependency upgrades?

## Conclusion

The grafana-micromegas-datasource plugin is a well-structured, relatively small codebase that could be integrated into the Micromegas monorepo with **moderate effort**. The primary challenges are:

1. **Dual language build chains** (TypeScript + Go vs Rust + Python)
2. **Dependency version alignment** (TypeScript, React, Grafana SDK)
3. **Go backend compilation** (Mage-based build for plugin binary)
4. **Separate development environments** (Grafana Docker vs Micromegas services)

The benefits of integration include:
1. **Unified development experience** (single clone, atomic commits)
2. **Shared type definitions** (query structures, auth config)
3. **Coordinated releases** (plugin + server updates together)
4. **Simplified testing** (integration tests spanning both)

**Recommendation for Phase 2**: Proceed with deeper integration analysis, focusing on build tooling evaluation and dependency conflict resolution. The technical feasibility appears **high**, pending resolution of dependency versioning and build tooling decisions.
# Repository Merge Study - Phase 2 Findings

## Executive Summary

Phase 2 integration analysis reveals **significant opportunities** for code sharing and unified development experience with **manageable complexity**. The key finding is that while the Grafana plugin and analytics-web-app use different frameworks (Grafana SDK vs Next.js), they share conceptual patterns that could benefit from a monorepo structure.

**Primary Recommendation**: Proceed with monorepo integration using **npm workspaces** with selective dependency management.

## Dependency Analysis

### TypeScript Version Conflict Analysis

**Current State**:
- **Grafana Plugin**: TypeScript 4.4.0
- **Analytics Web App**: TypeScript 5.4.0
- **Conflict Severity**: **Medium**

**Resolution Strategy**:
1. **Upgrade Grafana plugin to TypeScript 5.4.x**
   - Grafana SDK 9.4.7 supports TS 4.x-5.x
   - Minimal breaking changes expected (mostly stricter type checking)
   - Test with `yarn typecheck` after upgrade

2. **Shared tsconfig.json base**:
```json
// packages/tsconfig.base.json
{
  "compilerOptions": {
    "target": "ES2020",
    "lib": ["ES2020", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "moduleResolution": "bundler",
    "resolveJsonModule": true,
    "allowJs": false,
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "forceConsistentCasingInFileNames": true
  }
}
```

### React Version Compatibility

**Current State**:
- **Grafana Plugin**: React 17.0.2
- **Analytics Web App**: React 18.3.0
- **Conflict Severity**: **Low**

**Analysis**:
- React 17 and 18 can coexist in monorepo (different node_modules)
- No shared React components between projects currently
- Grafana SDK 9.4.7 requires React 17.x
- Future Grafana SDK upgrade (to 10.x or 11.x) would bring React 18

**Resolution Strategy**:
1. **Keep separate React versions** (short term)
   - Use npm workspaces with separate dependencies
   - No hoisting for React packages

2. **Upgrade Grafana SDK** (medium term)
   - Target Grafana SDK 10.x or 11.x (supports React 18)
   - Aligns plugin with analytics-web-app
   - Requires testing with newer Grafana versions

### Build Tool Evaluation

#### Current Build Systems

**Grafana Plugin**:
```json
{
  "build": "webpack -c ./.config/webpack/webpack.config.ts --env production",
  "dev": "webpack -w -c ./.config/webpack/webpack.config.ts --env development"
}
```
- Webpack 5.76.0
- Custom TypeScript-based config
- Go backend built with Mage

**Analytics Web App**:
```json
{
  "build": "next build",
  "dev": "next dev"
}
```
- Next.js 15.0.0 (built-in webpack)
- No custom webpack config needed
- No backend compilation

**Backend Services (Rust)**:
```bash
cd rust && cargo build --workspace
```
- Cargo workspace (unified)
- All Rust crates built together

#### Monorepo Build Tool Options

**Option 1: npm Workspaces** (RECOMMENDED)
```json
// Root package.json
{
  "name": "micromegas-monorepo",
  "private": true,
  "workspaces": [
    "grafana",
    "typescript/*",
    "doc/high-frequency-observability",
    "doc/presentation-template"
  ],
  "scripts": {
    "build": "npm run build --workspaces",
    "build:grafana": "npm run build --workspace=grafana",
    "build:web": "npm run build --workspace=typescript/analytics-web-app",
    "build:presentations": "npm run build --workspace=doc/high-frequency-observability --workspace=doc/presentation-template",
    "dev:grafana": "npm run dev --workspace=grafana",
    "dev:web": "npm run dev --workspace=typescript/analytics-web-app",
    "dev:presentation": "npm run dev --workspace=doc/high-frequency-observability"
  }
}
```

**Pros**:
- Built into npm (no additional tools)
- Simple, well-understood
- Good IDE support
- Works with existing build scripts

**Cons**:
- No sophisticated caching
- No task orchestration
- No dependency graph analysis
- Manual selective builds

**Note on Presentations**: The workspace includes `doc/high-frequency-observability` and `doc/presentation-template` because they have build dependencies (Vite, reveal.js, mermaid). Including them in the workspace provides:
- Single `npm install` at root installs all dependencies
- Unified build commands: `npm run build:presentations`
- Shared dependency management (if presentations use common tools)
- Keeps presentations discoverable under `doc/` while still managed by npm workspaces

**Option 2: Turborepo**
```json
// turbo.json
{
  "$schema": "https://turbo.build/schema.json",
  "pipeline": {
    "build": {
      "dependsOn": ["^build"],
      "outputs": ["dist/**", ".next/**"]
    },
    "dev": {
      "cache": false,
      "persistent": true
    }
  }
}
```

**Pros**:
- Intelligent caching across builds
- Parallel task execution
- Dependency graph awareness
- Remote caching support

**Cons**:
- Additional dependency
- Learning curve
- Overkill for 2 TypeScript projects?

**Option 3: Nx**
```json
// nx.json
{
  "tasksRunnerOptions": {
    "default": {
      "runner": "@nrwl/workspace/tasks-runners/default",
      "options": {
        "cacheableOperations": ["build", "test", "lint"]
      }
    }
  }
}
```

**Pros**:
- Most powerful monorepo tool
- Code generators
- Affected project detection
- Computation caching

**Cons**:
- Heavy tooling
- Significant configuration
- May conflict with Grafana plugin tooling

**Recommendation**: **npm workspaces** for simplicity, with potential future migration to Turborepo if caching becomes important.

### Shared Dependencies

**Current Overlap**:
```
Both projects use:
- TypeScript (different versions)
- React (different versions)
- ESLint
- Prettier
- Jest (Grafana plugin) / No tests (analytics-web-app)

Grafana plugin only:
- @grafana/* packages (SDK, UI, data)
- webpack, webpack-cli
- cypress

Analytics-web-app only:
- Next.js
- @tanstack/react-query
- @radix-ui/* components
- tailwindcss
```

**Shareable Dev Dependencies**:

All dev dependencies managed in root `package.json` for consistency. Shared across all TypeScript projects via workspace hoisting.

**Benefits**:
- Single version for shared tooling
- Consistent code style across projects
- Easier version upgrades

## Shared Type Definitions Analysis

### Currently Duplicated Concepts

**Process Information**:
```typescript
// Grafana Plugin (doesn't have explicit ProcessInfo type)
// But queries return process data via FlightSQL

// Analytics Web App
export interface ProcessInfo {
  process_id: string;
  exe: string;
  start_time: string;
  last_update_time: string;
  computer: string;
  username: string;
  cpu_brand: string;
  distro: string;
  properties: Record<string, string>;
}
```

**SQL Query Structures**:
```typescript
// Grafana Plugin
export interface SQLQuery extends DataQuery {
  queryText?: string;
  format?: string;
  rawEditor?: boolean;
  table?: string;
  columns?: string[];
  wheres?: string[];
  orderBy?: string;
  groupBy?: string;
  limit?: string;
  timeFilter?: boolean;
  autoLimit?: boolean;
}

// Analytics Web App
// Uses direct SQL strings via API, no structured query type
```

**Authentication Config**:
```typescript
// Grafana Plugin
export interface FlightSQLDataSourceOptions extends DataSourceJsonData {
  host?: string;
  token?: string;
  secure?: boolean;
  username?: string;
  password?: string;
  selectedAuthType?: string;
  metadata?: any;
}

// Analytics Web App (analytics-web-srv Rust backend)
// No TypeScript auth types - handled in Rust
```

### Proposed Shared Types Package

**Create**: `typescript/types/`

```typescript
// typescript/types/src/process.ts
export interface ProcessInfo {
  process_id: string;
  exe: string;
  start_time: string; // RFC3339 format
  last_update_time: string;
  computer: string;
  username: string;
  cpu_brand: string;
  distro: string;
  properties: Record<string, string>;
}

export interface ProcessStatistics {
  process_id: string;
  log_entries: number;
  measures: number;
  trace_events: number;
  thread_count: number;
}

// typescript/micromegas-types/src/logs.ts
export interface LogEntry {
  time: string; // RFC3339 format
  level: string; // "FATAL" | "ERROR" | "WARN" | "INFO" | "DEBUG" | "TRACE"
  target: string;
  msg: string;
}

export enum LogLevel {
  FATAL = 1,
  ERROR = 2,
  WARN = 3,
  INFO = 4,
  DEBUG = 5,
  TRACE = 6
}

// typescript/micromegas-types/src/auth.ts
export interface FlightSQLConnection {
  host: string;
  port: number;
  secure: boolean; // TLS enabled
}

export type AuthType = 'none' | 'token' | 'username-password' | 'oauth';

export interface TokenAuth {
  type: 'token';
  token: string;
}

export interface UsernamePasswordAuth {
  type: 'username-password';
  username: string;
  password: string;
}

export interface OAuthAuth {
  type: 'oauth';
  issuer: string;
  client_id: string;
  client_secret: string;
  audience?: string;
}

export type AuthConfig = TokenAuth | UsernamePasswordAuth | OAuthAuth | { type: 'none' };

// typescript/micromegas-types/src/tracing.ts
export interface GenerateTraceRequest {
  time_range?: {
    begin: string; // RFC3339
    end: string;   // RFC3339
  };
  include_async_spans: boolean;
  include_thread_spans: boolean;
}

export interface TraceGenerationProgress {
  type: 'progress';
  percentage: number;
  message: string;
}
```

**Usage in Projects**:
```typescript
// grafana/src/types.ts
import { ProcessInfo, LogEntry } from '@micromegas/types';

// typescript/analytics-web-app/src/types/index.ts
export * from '@micromegas/types';
```

**Benefits**:
- Single source of truth for data models
- Type safety across plugin and web app
- Easier to keep in sync with Rust backend
- Can generate TypeScript from Rust types (future)

## API Contract Analysis

### FlightSQL Server Endpoints (Rust)

**Endpoints Used by Grafana Plugin**:
```
GET /flightsql/sql-info     - SQL capabilities metadata
GET /flightsql/tables       - List available tables
GET /flightsql/columns      - Get columns for a table
POST /query                 - Execute FlightSQL query (gRPC)
GET /plugin/macros          - SQL macro definitions
```

**Go Backend Resource Handlers** (in Grafana plugin):
```go
// pkg/flightsql/resources.go
func (ds *FlightSQLDatasource) CallResource(ctx context.Context, req *backend.CallResourceRequest, sender backend.CallResourceResponseSender) error {
  switch req.Path {
  case "flightsql/sql-info":
    // Return SQL capabilities
  case "flightsql/tables":
    // List tables from FlightSQL
  case "flightsql/columns":
    // Get table columns
  case "plugin/macros":
    // Return macro definitions
  }
}
```

**Analytics Web Server Endpoints** (Rust):
```
GET /analyticsweb/health                        - Health check
GET /analyticsweb/processes                     - List processes
GET /analyticsweb/process/{id}/log-entries      - Get log entries
GET /analyticsweb/process/{id}/statistics       - Process statistics
GET /analyticsweb/perfetto/{id}/info            - Trace metadata
POST /analyticsweb/perfetto/{id}/generate       - Generate trace (streaming)
```

**Overlap Analysis**:
- **No direct API overlap** - different endpoints
- Both talk to FlightSQL server (different use cases)
- Plugin: SQL query interface
- Web app: Process/log/trace interface

**Shared Backend Concern**: Authentication
- Both need OAuth 2.0 support (planned for plugin)
- Current plugin supports: none, token, username/password
- FlightSQL server supports: OIDC (OAuth 2.0)
- Alignment opportunity: OAuth configuration types

### Authentication Flow Comparison

**Grafana Plugin Flow**:
```
User configures datasource
  → Select auth type (none, token, username/password)
  → Credentials stored in Grafana (SecureJsonData)
  → Plugin backend reads credentials
  → Adds to gRPC metadata (bearer token or basic auth)
  → FlightSQL server validates
```

**Analytics Web App Flow** (planned with OAuth):
```
User accesses web app
  → OAuth 2.0 client credentials flow
  → Fetch token from OIDC provider
  → Cache token (expires_in)
  → Send Bearer token to analytics-web-srv
  → Analytics-web-srv validates with OIDC (JWKS)
  → Proxies requests to FlightSQL with validated identity
```

**Shared Auth Code Opportunity**:
- OAuth token fetching logic (when plugin adds OAuth support)
- OIDC discovery endpoint handling
- Token caching logic
- JWT validation (if done client-side)

**Proposed Shared Package**: `@micromegas/auth`
```typescript
// typescript/micromegas-auth/src/oauth-client.ts
export class OAuthTokenManager {
  constructor(config: OAuthConfig) {}
  async getToken(): Promise<string> {}
  async refreshToken(): Promise<string> {}
  clearCache(): void {}
}

// typescript/micromegas-auth/src/oidc-discovery.ts
export async function discoverOIDCEndpoints(issuer: string): Promise<OIDCMetadata> {}
```

**Usage**:
- Grafana plugin (when OAuth added)
- Analytics web app (for OAuth flow)
- CLI tools (future)

## Testing Infrastructure Comparison

### Grafana Plugin Testing

**Frontend Tests (Jest)**:
```json
{
  "test": "jest --watch --onlyChanged",
  "test:ci": "jest --maxWorkers 4 --passWithNoTests"
}
```
- **Framework**: Jest 29 + @testing-library/react 12
- **Coverage**: Basic unit tests (datasource, components, utils)
- **Mocking**: Mock Service Worker (MSW) for API calls

**Backend Tests (Go)**:
```go
// Run with: mage coverage
// Uses: testify/assert, go test
```
- **Framework**: Go testing + testify
- **Coverage**: FlightSQL client, Arrow conversion, macros
- **Integration**: No integration tests visible

**E2E Tests (Cypress)**:
```json
{
  "e2e": "yarn cypress install && yarn grafana-e2e run"
}
```
- **Framework**: Cypress + @grafana/e2e
- **Scope**: Smoke test (01-smoke.spec.ts)
- **Environment**: docker-compose Grafana instance

### Analytics Web App Testing

**Current State**: **No tests**
- No test framework configured
- No test files present
- `package.json` has no test scripts

**Recommendation**: Add testing
```json
{
  "devDependencies": {
    "@testing-library/react": "^14.0.0",
    "@testing-library/jest-dom": "^6.0.0",
    "@vitejs/plugin-react": "^4.0.0",
    "vitest": "^1.0.0"
  },
  "scripts": {
    "test": "vitest",
    "test:ci": "vitest run --coverage"
  }
}
```

### Unified Testing Strategy

**Recommendation**: Maintain separate test suites with shared utilities

**Shared Test Utilities Package**: `typescript/test/`
```typescript
// typescript/test/src/mock-data.ts
export const mockProcessInfo: ProcessInfo = {
  process_id: "550e8400-e29b-41d4-a716-446655440000",
  exe: "test-app",
  // ... mock data
};

export const mockLogEntries: LogEntry[] = [ /* ... */ ];

// typescript/test/src/mock-flightsql.ts
export class MockFlightSQLClient {
  query(sql: string): Promise<QueryResult> { /* ... */ }
}

// typescript/test/src/test-helpers.ts
export function createTestProcess(overrides?: Partial<ProcessInfo>): ProcessInfo {
  return { ...mockProcessInfo, ...overrides };
}
```

**Usage**:
```typescript
// grafana/src/datasource.test.ts
import { mockProcessInfo, MockFlightSQLClient } from '@micromegas/test';

// typescript/analytics-web-app/src/components/ProcessTable.test.tsx
import { mockProcessInfo, createTestProcess } from '@micromegas/test';
```

**CI/CD Testing Strategy**:
```yaml
# .github/workflows/test.yml
name: Test

on: [push, pull_request]

jobs:
  test-plugin:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: actions/setup-node@v3
      - run: npm ci
      - run: npm test --workspace=grafana
      - run: npm run e2e --workspace=grafana

  test-web-app:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: actions/setup-node@v3
      - run: npm ci
      - run: npm test --workspace=analytics-web-app
```

## Code Reuse Opportunities

### 1. Shared Type Definitions ✅ HIGH VALUE

**Package**: `@micromegas/types`

**Contents**:
- Process metadata types
- Log entry types
- Query structures
- Authentication config types
- Trace generation types

**Impact**: Medium effort, high value (type safety across projects)

### 2. Authentication Logic 🔄 MEDIUM VALUE (Future)

**Package**: `@micromegas/auth`

**Contents**:
- OAuth token manager
- OIDC discovery
- Token caching
- JWT validation helpers

**Impact**: Medium effort, medium value (useful when plugin adds OAuth)

### 3. SQL Utilities 📊 LOW VALUE

**Package**: `@micromegas/sql-utils`

**Contents**:
- SQL formatter (already in plugin)
- Query builder helpers
- Macro expansion

**Impact**: Low effort, low value (plugin-specific, not used in web app)

### 4. Date/Time Utilities ⏰ MEDIUM VALUE

**Package**: `@micromegas/time`

**Contents**:
- RFC3339 parsing/formatting
- Time range calculations
- Duration formatting
- Timezone handling

**Impact**: Low effort, medium value (both projects handle timestamps)

### 5. Test Utilities 🧪 HIGH VALUE

**Package**: `@micromegas/test`

**Contents**:
- Mock data generators
- Test helpers
- Mock FlightSQL client
- Test fixtures

**Impact**: Low effort, high value (improves test quality, reduces duplication)

### Priority Matrix

```
High Value, Low Effort:
  ✅ @micromegas/types          (Priority 1)
  ✅ @micromegas/test     (Priority 2)

High Value, Medium Effort:
  - None identified

Medium Value, Low Effort:
  ⏰ @micromegas/time     (Priority 3)

Medium Value, Medium Effort:
  🔄 @micromegas/auth           (Priority 4 - wait for plugin OAuth)

Low Value:
  📊 @micromegas/sql-utils      (Skip or deprioritize)
```

## Workspace Structure Recommendation

### Design Rationale

**Mixed Organization Strategy**:
- **Top-level project folders** for multi-language components: `grafana/`, `unreal/`
  - Grafana plugin: TypeScript (frontend) + Go (backend)
  - Unreal integration: C++ + Blueprints
- **Language-based folders** for single-language ecosystems: `rust/`, `python/`, `typescript/`
  - Pure Rust workspace
  - Pure Python packages
  - Pure TypeScript packages (shared libraries, analytics web app)

**Benefits**:
- Grafana plugin keeps TypeScript + Go together (like Unreal keeps C++ together)
- Shared TypeScript packages in `typescript/` for reuse across projects
- Clear separation: `grafana/` is a product/integration, `typescript/` is shared libraries
- Consistent with existing structure (`rust/`, `python/`, `unreal/`)

### Proposed Monorepo Layout

```
micromegas/
├── package.json                    # Root workspace config
├── package-lock.json
├── tsconfig.base.json              # Shared TypeScript config
├── .eslintrc.json                  # Shared ESLint config
├── .prettierrc                     # Shared Prettier config
│
├── grafana/                        # Grafana datasource plugin (TypeScript + Go)
│   ├── package.json
│   ├── tsconfig.json               # Extends ../tsconfig.base.json
│   ├── Magefile.go                 # Go build tool
│   ├── go.mod                      # Go dependencies
│   ├── src/                        # TypeScript frontend
│   │   ├── components/
│   │   ├── datasource.ts
│   │   └── types.ts
│   ├── pkg/                        # Go backend
│   │   ├── flightsql/
│   │   └── main.go
│   └── README.md
│
├── typescript/                     # TypeScript shared packages
│   │
│   ├── analytics-web-app/          # Next.js web application
│   │   ├── package.json
│   │   ├── tsconfig.json           # Extends ../tsconfig.base.json
│   │   ├── src/
│   │   ├── public/
│   │   └── README.md
│   │
│   ├── types/                      # @micromegas/types (shared package)
│   │   ├── package.json
│   │   ├── src/
│   │   │   ├── process.ts
│   │   │   ├── logs.ts
│   │   │   ├── auth.ts
│   │   │   ├── tracing.ts
│   │   │   └── index.ts
│   │   └── tsconfig.json
│   │
│   ├── test/                 # @micromegas/test (shared package)
│   │   ├── package.json
│   │   ├── src/
│   │   │   ├── mock-data.ts
│   │   │   ├── test-helpers.ts
│   │   │   └── index.ts
│   │   └── tsconfig.json
│   │
│   └── time/                 # @micromegas/time (shared package)
│       ├── package.json
│       ├── src/
│       │   ├── rfc3339.ts
│       │   ├── duration.ts
│       │   └── index.ts
│       └── tsconfig.json
│
├── rust/                           # Existing Rust workspace
│   ├── Cargo.toml
│   ├── flight-sql-srv/
│   ├── analytics-web-srv/
│   └── ...
│
├── python/                         # Existing Python packages
│   └── micromegas/
│
├── unreal/                         # Existing Unreal integration (C++ + Blueprints)
│
├── doc/                            # Documentation and presentations
│   ├── high-frequency-observability/  # Reveal.js presentation (in npm workspace)
│   │   ├── package.json
│   │   ├── vite.config.js
│   │   └── src/slides/
│   ├── presentation-template/     # Presentation template (in npm workspace)
│   │   ├── package.json
│   │   └── ...
│   └── ...                         # Other documentation
│
├── build/                          # Build scripts
└── tasks/                          # Planning documents (including this!)
```

### Root package.json

```json
{
  "name": "micromegas-monorepo",
  "version": "1.0.0",
  "private": true,
  "workspaces": [
    "grafana",
    "typescript/*",
    "doc/high-frequency-observability",
    "doc/presentation-template"
  ],
  "scripts": {
    "build": "npm run build --workspaces",
    "build:grafana": "npm run build --workspace=grafana",
    "build:web": "npm run build --workspace=typescript/analytics-web-app",
    "build:shared": "npm run build --workspace=typescript/types --workspace=typescript/test --workspace=typescript/time",
    "build:presentations": "npm run build --workspace=doc/high-frequency-observability --workspace=doc/presentation-template",

    "dev:grafana": "npm run dev --workspace=grafana",
    "dev:web": "npm run dev --workspace=typescript/analytics-web-app",
    "dev:presentation": "npm run dev --workspace=doc/high-frequency-observability",

    "test": "npm test --workspaces",
    "test:grafana": "npm test --workspace=grafana",
    "test:web": "npm test --workspace=typescript/analytics-web-app",

    "lint": "npm run lint --workspaces",
    "typecheck": "npm run typecheck --workspaces",

    "clean": "npm run clean --workspaces && rm -rf node_modules"
  },
  "devDependencies": {
    "@types/node": "^20.0.0",
    "eslint": "^8.57.0",
    "prettier": "^2.5.0",
    "typescript": "^5.4.0"
  }
}
```

### Migration Benefits

**Developer Experience**:
- Single `npm install` for all TypeScript projects
- Consistent tooling versions
- Shared configuration (ESLint, Prettier, TypeScript)
- Easy cross-package imports

**CI/CD Benefits**:
- Single node_modules cache
- Parallel test execution
- Shared build artifacts
- Unified version bumping

**Maintenance Benefits**:
- Single place to update dependencies
- Consistent code style
- Easier refactoring across projects
- Type safety across boundaries

## Dependency Version Alignment Strategy

### Phase 1: Immediate Upgrades

**Grafana Plugin**:
1. Upgrade TypeScript: `4.4.0` → `5.4.0`
   ```bash
   cd grafana
   npm install -D typescript@5.4.0
   npm run typecheck  # Fix any type errors
   ```

2. Update ESLint/Prettier to match web app versions
   ```bash
   npm install -D eslint@8.57.0 prettier@2.5.0
   ```

3. Test build and CI pipeline
   ```bash
   npm run build
   npm run test:ci
   ```

**Analytics Web App**:
- No immediate changes needed (already on latest versions)

### Phase 2: Workspace Migration

1. Create root `package.json` with workspaces
2. Move projects into workspace structure
3. Extract shared types into `typescript/types/`
4. Update imports to use `@micromegas/types`
5. Test all builds and tests
6. Update CI/CD workflows

### Phase 3: Grafana SDK Upgrade (Future)

**Goal**: Align Grafana plugin with latest SDK

**Current**: Grafana SDK 9.4.7 (React 17)
**Target**: Grafana SDK 10.x or 11.x (React 18)

**Steps**:
1. Review Grafana SDK changelog for breaking changes
2. Upgrade Grafana SDK and React together
3. Update component code for React 18 changes
4. Test with newer Grafana versions (10.x, 11.x)
5. Update plugin compatibility in plugin.json

**Benefits**:
- React 18 alignment with analytics-web-app
- Access to newer Grafana features
- Better long-term support

### Phase 4: Ongoing Maintenance

**Dependency Update Strategy**:
1. Use Dependabot or Renovate for automated PRs
2. Group related updates (e.g., all @grafana/* packages)
3. Test in CI before merging
4. Update shared packages first, then consumers

**Version Pinning Strategy**:
- **Exact versions** for critical deps (TypeScript, React)
- **Caret ranges** (^) for dev tools (ESLint, Prettier)
- **Locked versions** via package-lock.json

## Licensing Considerations

### License Compatibility ✅

Both repositories use **Apache License 2.0**, which is fully compatible for merging:

**Current Licenses**:
- **Grafana plugin**: Apache 2.0 (forked from InfluxData FlightSQL plugin)
- **Micromegas repo**: Apache 2.0

**Apache 2.0 Permissions**:
- ✅ Merging with other Apache 2.0 code
- ✅ Redistribution in different repository structures
- ✅ Modification and derivative works
- ✅ Commercial use
- ✅ Private use

### Legal Requirements (Apache 2.0 Section 4)

When merging the Grafana plugin repository, you must:

1. **Retain copyright notices**
   - Keep the LICENSE file from grafana-micromegas-datasource
   - Place at `grafana/LICENSE` in the monorepo

2. **State changes made to the Work**
   - ✅ Already satisfied: README states "forked from Influx's FlightSQL grafana plugin"
   - Maintain this attribution in `grafana/README.md`

3. **Include a copy of the License**
   - Action: Copy LICENSE file during migration
   ```bash
   cp grafana-micromegas-datasource/LICENSE micromegas/grafana/LICENSE
   ```

4. **Include NOTICE file if it exists**
   - ✅ No NOTICE file currently exists in grafana-micromegas-datasource
   - Optional: Create `grafana/NOTICE` file for attribution clarity

### Recommended Attribution File

Create `grafana/NOTICE` (optional but recommended):

```
Grafana Micromegas Datasource Plugin
Copyright 2024 Marc-Antoine Desroches

This plugin is based on the InfluxData FlightSQL Grafana plugin:
https://github.com/influxdata/grafana-flightsql-datasource
Originally Copyright 2023 InfluxData, Inc.
Licensed under Apache License 2.0

Forked November 2024 for Micromegas integration with the following modifications:
- OAuth 2.0 client credentials authentication support
- Micromegas-specific branding and documentation
- FlightSQL protocol compatibility maintained
```

### No Legal Blockers

**Conclusion**: ✅ **No legal issues with merging repositories**

The Apache 2.0 license is designed to be permissive and allows exactly this type of repository reorganization. The only requirement is proper attribution, which is easily satisfied by:
1. Keeping the LICENSE file in the `grafana/` folder
2. Maintaining fork attribution in the README
3. Optionally creating a NOTICE file for clarity

This is standard practice for Apache 2.0 forks and poses no legal risk.

## Potential Challenges and Mitigations

### Challenge 1: Grafana Plugin Special Build Process

**Issue**: Grafana plugin requires:
- Go backend compilation (Mage)
- Plugin signing (Grafana API key)
- Special packaging format (ZIP with specific structure)

**Mitigation**:
- Keep plugin's `build-plugin.sh` script
- Document Grafana-specific build steps
- CI/CD handles signing separately from general builds
- Don't try to "normalize" Grafana plugin build

**Example CI workflow**:
```yaml
# .github/workflows/release-plugin.yml
name: Release Grafana Plugin

on:
  push:
    tags:
      - 'plugin-v*'

jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: actions/setup-node@v3
      - uses: actions/setup-go@v3

      - name: Install dependencies
        run: npm ci

      - name: Build shared packages
        run: npm run build --workspace=grafana --workspace=typescript/*

      - name: Build plugin
        run: npm run build-plugin --workspace=grafana

      - name: Sign plugin
        env:
          GRAFANA_API_KEY: ${{ secrets.GRAFANA_API_KEY }}
        run: npm run sign --workspace=grafana

      - name: Create release
        uses: softprops/action-gh-release@v1
        with:
          files: grafana-plugin/dist/*.zip
```

### Challenge 2: Coordinated Release Management

**Issue**: Need to release Grafana plugin with every Micromegas release, even when unchanged

**Mitigation**:
- Follow existing Rust/Python coordinated release pattern
- All components use synchronized version numbers (see "Release Strategy" section)
- Automated release workflows for all components
- Per-component CHANGELOGs note "No changes" when applicable

**Example versioning** (coordinated):
```json
// grafana/package.json
{
  "name": "micromegas-datasource",
  "version": "0.5.0"  // Synced with main Micromegas version
}

// typescript/analytics-web-app/package.json
{
  "name": "@micromegas/analytics-web-app",
  "version": "0.5.0"  // Synced with main Micromegas version
}

// typescript/types/package.json
{
  "name": "@micromegas/types",
  "version": "0.5.0"  // Synced with main Micromegas version
}
```

### Challenge 3: Node.js + Go + Rust Multi-Language Build

**Issue**: Three language ecosystems in one repo

**Mitigation**:
- Clear separation of concerns:
  - npm workspaces: TypeScript projects only
  - Cargo workspace: Rust projects only (unchanged)
  - Go: Isolated in Grafana plugin (unchanged)
- Root-level scripts for common operations:
  ```json
  {
    "scripts": {
      "build:all": "npm run build && cd rust && cargo build --workspace",
      "test:all": "npm test && cd rust && cargo test --workspace"
    }
  }
  ```
- Document language-specific prerequisites clearly

### Challenge 4: Developer Onboarding Complexity

**Issue**: New contributors need to understand monorepo structure

**Mitigation**:
- Clear CONTRIBUTING.md with monorepo section
- Separate README per project
- Scripts for common workflows:
  ```bash
  # scripts/setup.sh
  #!/bin/bash
  echo "Installing Node.js dependencies..."
  npm install

  echo "Building shared packages..."
  npm run build --workspace=grafana --workspace=typescript/*

  echo "Setup Rust environment..."
  cd rust && cargo check

  echo "Ready to develop!"
  ```
- Document common pitfalls and solutions

## Release Strategy

### Coordinated Releases

Following the existing Micromegas pattern where Python and Rust components are released together, the Grafana plugin should follow a **coordinated release strategy**:

**Current Practice**:
- Python client and Rust services released together with matching version numbers
- Ensures compatibility across the entire stack
- Simplifies version management for users

**Proposed Strategy for Grafana Plugin**:
- Include Grafana plugin in coordinated releases
- Release with Rust/Python even if Grafana plugin has no changes
- Use consistent version numbers across all components

**Example Release Flow**:
```bash
# Micromegas v0.5.0 release includes:
- Rust services: v0.5.0
- Python client: v0.5.0
- Grafana plugin: v0.5.0 (even if no changes since v0.4.9)

# Tag strategy in monorepo:
git tag v0.5.0                    # Overall release
git tag rust/v0.5.0              # Rust-specific (optional)
git tag python/v0.5.0            # Python-specific (optional)
git tag grafana/v0.5.0           # Grafana-specific (for GitHub releases)
```

### Version Synchronization

**Benefits of Coordinated Releases**:
1. **User Clarity**: "Use Micromegas v0.5.0" means all components at v0.5.0
2. **Compatibility Guarantees**: Plugin v0.5.0 guaranteed to work with FlightSQL server v0.5.0
3. **Simplified Documentation**: One version to document and support
4. **Reduced Support Burden**: No version matrix complexity

**When to Release Grafana Plugin**:
- ✅ **Include in every Micromegas release** (synchronized version)
- ✅ Even if plugin has no code changes
- ✅ Changelog notes: "No changes in this release" if applicable
- ❌ **Avoid hotfix-only plugin releases** (creates version drift)

**Exception: Plugin-Specific Hotfixes**:
If a critical Grafana plugin bug needs urgent fix:
```bash
# Hotfix release: v0.5.1
git tag grafana/v0.5.1
# Create GitHub release for plugin only
# Update changelog: "Hotfix for [issue]"
# Next coordinated release: v0.6.0 includes this fix
```

### GitHub Release Artifacts

**Per-Component Releases**:
Even with synchronized versions, maintain separate GitHub releases for each component:

```
GitHub Releases:
├── v0.5.0 (main release)
│   ├── Description: "Micromegas v0.5.0 release notes"
│   └── Assets: (none - see component releases)
│
├── grafana/v0.5.0
│   ├── Description: "Grafana plugin v0.5.0"
│   └── Assets: micromegas-datasource-0.5.0.zip
│
├── rust/v0.5.0
│   ├── Description: "Rust services v0.5.0"
│   └── Assets: binaries for various platforms
│
└── python/v0.5.0
    ├── Description: "Python client v0.5.0"
    └── Assets: wheels (if not using PyPI exclusively)
```

**Automation**:
```yaml
# .github/workflows/release.yml
name: Coordinated Release

on:
  push:
    tags:
      - 'v*.*.*'

jobs:
  release-grafana:
    if: startsWith(github.ref, 'refs/tags/v')
    runs-on: ubuntu-latest
    steps:
      - name: Build Grafana plugin
        run: |
          cd grafana
          npm ci
          npm run build
          npm run sign
          zip -r ../micromegas-datasource-${VERSION}.zip .

      - name: Create Grafana Release
        uses: ncipollo/release-action@v1
        with:
          tag: grafana/${VERSION}
          name: "Grafana Plugin ${VERSION}"
          artifacts: "micromegas-datasource-*.zip"
          body: "See main release v${VERSION} for details"

  release-rust:
    # ... similar for Rust

  release-python:
    # ... similar for Python
```

### CHANGELOG Management

**Per-Component CHANGELOGs**:
```
micromegas/
├── CHANGELOG.md                    # Overall project changelog
├── grafana/CHANGELOG.md           # Grafana plugin specific
├── rust/CHANGELOG.md              # Rust services
└── python/micromegas/CHANGELOG.md # Python client
```

**Example grafana/CHANGELOG.md**:
```markdown
# Grafana Plugin Changelog

## [0.5.0] - 2025-01-15

### Added
- OAuth 2.0 client credentials authentication support

### Changed
- Updated TypeScript to 5.4.0

### Fixed
- (none)

## [0.4.9] - 2025-01-01

### Changed
- No changes (version sync with Micromegas v0.4.9)
```

### Key Principle

**"Release together, version together"** - The Grafana plugin is a first-class component of Micromegas and should be treated as part of the unified release process, just like Python and Rust components.

## Documentation Strategy

### MkDocs Integration

With the Grafana plugin becoming part of the monorepo, comprehensive documentation should be added to the existing MkDocs site.

**Proposed MkDocs Structure**:

```yaml
# mkdocs/mkdocs.yml additions
nav:
  - Home: index.md
  - Getting Started: getting-started.md
  - ... (existing sections)

  - Grafana Plugin:
    - Overview: grafana/overview.md
    - Installation: grafana/installation.md
    - Configuration:
      - Connection Setup: grafana/config/connection.md
      - Authentication: grafana/config/authentication.md
      - OAuth 2.0 Setup: grafana/config/oauth.md
    - Usage:
      - Creating Dashboards: grafana/usage/dashboards.md
      - Query Builder: grafana/usage/query-builder.md
      - SQL Editor: grafana/usage/sql-editor.md
      - Variables and Templating: grafana/usage/variables.md
    - Data Sources:
      - Logs: grafana/datasources/logs.md
      - Metrics: grafana/datasources/metrics.md
      - Traces: grafana/datasources/traces.md
    - Examples:
      - Dashboard Templates: grafana/examples/templates.md
      - Common Queries: grafana/examples/queries.md
      - Alerting Rules: grafana/examples/alerting.md
    - Troubleshooting: grafana/troubleshooting.md
    - Development: grafana/development.md
```

**Documentation Content**:

1. **Installation Guide**
   - Download and install from GitHub releases
   - Grafana version compatibility
   - Plugin verification and signing
   - Update procedures

2. **Configuration Guide**
   - Connection to FlightSQL server
   - Authentication methods (none, username/password, token, OAuth 2.0)
   - TLS/SSL setup
   - Network troubleshooting

3. **Usage Guides**
   - Query builder interface walkthrough
   - Raw SQL editor usage
   - Time range filters and macros
   - Variable substitution
   - Dashboard creation best practices

4. **Data Source Guides**
   - Querying logs with structured fields
   - Metrics aggregation patterns
   - Trace visualization
   - Correlation between data types

5. **Example Dashboards**
   - System monitoring dashboard
   - Application performance monitoring
   - Error tracking and alerting
   - Custom metrics visualization
   - JSON templates for import

6. **Development Guide**
   - Building from source
   - Local development setup
   - Testing plugin changes
   - Contributing guidelines

**Benefits of MkDocs Documentation**:

- **Centralized**: All Micromegas documentation in one place
- **Versioned**: Documentation tied to releases
- **Searchable**: Unified search across all components
- **Consistent**: Same style and navigation as other docs
- **Discoverable**: Users don't need to visit separate repo for plugin docs

**Documentation Source Location**:

```
micromegas/
├── mkdocs/
│   ├── docs/
│   │   ├── grafana/              # Grafana plugin documentation
│   │   │   ├── overview.md
│   │   │   ├── installation.md
│   │   │   ├── config/
│   │   │   ├── usage/
│   │   │   ├── datasources/
│   │   │   ├── examples/
│   │   │   ├── troubleshooting.md
│   │   │   └── development.md
│   │   ├── api/                  # Existing API docs
│   │   └── ...                   # Other existing docs
│   └── mkdocs.yml
```

**Cross-References**:

- Link Grafana docs to FlightSQL server documentation
- Reference authentication setup from admin guide
- Link example queries to SQL syntax reference
- Connect troubleshooting to FlightSQL server logs

**Screenshots and Diagrams**:

- Configuration screenshots
- Dashboard examples
- Query builder UI
- Authentication flow diagrams
- Architecture diagrams showing plugin → FlightSQL → data lake

**Migration Note**: Current Grafana plugin README should remain for GitHub visitors, but point to comprehensive MkDocs documentation for full details.

## Recommendations Summary

### Immediate Actions

1. ✅ **Upgrade Grafana plugin TypeScript** to 5.4.0
2. ✅ **Create shared types package** (`@micromegas/types`)
3. ✅ **Set up npm workspaces** with root package.json
4. ✅ **Align ESLint/Prettier configs**

### Short-term

5. ✅ **Migrate Grafana plugin** into monorepo structure
6. ✅ **Update imports** to use shared types
7. ✅ **Create shared test** package
8. ✅ **Update CI/CD** for monorepo builds

### Medium-term

9. 🔄 **Add tests to analytics-web-app** (Vitest)
10. 🔄 **Upgrade Grafana SDK** to 10.x or 11.x (React 18)
11. 🔄 **Create time package** (if needed)
12. 🔄 **Evaluate Turborepo** for build caching (optional)

### Long-term

13. 🔄 **Add OAuth support to Grafana plugin**
14. 🔄 **Create shared auth package** (`@micromegas/auth`)
15. 🔄 **Code generation** from Rust types to TypeScript
16. 🔄 **Integration tests** spanning plugin and server
17. 🔄 **Comprehensive Grafana documentation** in MkDocs

## Success Metrics

**Technical Metrics**:
- ✅ All builds pass in monorepo CI
- ✅ Zero type errors across all packages
- ✅ Shared types used in at least 2 projects
- ✅ Test coverage maintained or improved
- ✅ Build time <5min for full monorepo

**Developer Experience Metrics**:
- ✅ Single command to set up development environment
- ✅ Hot reload works in both plugin and web app
- ✅ Type-safe imports between packages
- ✅ Onboarding time reduced (documentation quality)

**Maintenance Metrics**:
- ✅ Dependency updates happen across all projects simultaneously
- ✅ Breaking changes coordinated across packages
- ✅ Code reuse demonstrated (shared packages used)

## Conclusion

Phase 2 analysis confirms that **monorepo integration is feasible and beneficial** for the Grafana plugin and Micromegas ecosystem. The key findings:

1. **npm workspaces** provide sufficient tooling for current scale
2. **Shared type definitions** offer immediate value with low effort
3. **Dependency alignment** is manageable (TypeScript upgrade, future SDK upgrade)
4. **Coordinated release strategy** aligns with existing Rust/Python release process
5. **Code reuse opportunities** are significant (types, test utils, future auth)

**Overall Recommendation**: **Proceed with monorepo integration** using the phased approach outlined above, starting with TypeScript upgrade and workspace setup.

**Risk Level**: **Low to Medium** - technical feasibility is high, primary risks are organizational (developer onboarding, CI/CD coordination).

**Next Phase**: Phase 3 should focus on organizational impact analysis and detailed migration planning.
