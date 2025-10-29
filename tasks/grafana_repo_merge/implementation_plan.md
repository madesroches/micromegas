# Grafana Repository Merge - Implementation Plan

**Status**: Phase 2 Complete
**Last Updated**: 2025-10-29
**Current Phase**: Phase 2 Complete - Repository Merged, Ready for Phase 3

## Overview

This document provides a detailed, step-by-step plan for merging the Grafana datasource plugin repository into the main Micromegas monorepo. Based on the comprehensive study in `repository_merge_study.md`, this plan uses the **npm workspaces monorepo** approach.

**Related Documents**:
- Study: `repository_merge_study.md`
- OAuth Implementation: `../auth/grafana_oauth_plan.md`

## Current State Summary

### Completed
- ✅ Repository merge study completed (4 implementation approaches analyzed)
- ✅ OAuth 2.0 authentication plan created
- ✅ Detailed implementation plan created with 7 phases
- ✅ `grafana` branch created with planning documents
- ✅ **Phase 1.1 Complete**: Current state documented
- ✅ **Phase 2 Complete**: Repository merged with full history preserved
- ✅ `grafana/` directory created with all plugin files
- ✅ Root `package.json` workspace configuration created
- ✅ `typescript/types` shared types package created and built
- ✅ npm workspaces initialized successfully
- ✅ Root README updated with Grafana plugin section

### In Progress
- 🔄 Phase 3: Upgrade Dependencies & Align Versions (ready to start)

### Not Started
- ❌ Dependency upgrades (Phase 3)
- ❌ CI/CD updates (Phase 4)
- ❌ Documentation updates (Phase 5)
- ❌ Testing & validation (Phase 6)
- ❌ Cleanup & migration (Phase 7)

### Next Steps
1. Upgrade Grafana plugin TypeScript to 5.4 (Phase 3.1)
2. Align development dependencies with analytics-web-app (Phase 3.2)
3. Update CI/CD workflows (Phase 4)

## Prerequisites

- [ ] Review and approval of `repository_merge_study.md`
- [ ] Backup both repositories
- [ ] Ensure all pending PRs in grafana-micromegas-datasource are merged or documented
- [ ] Clean working directories in both repos

## Phase 1: Pre-Merge Preparation

### 1.1 Document Current State ✅

**Location**: Both repos
**Status**: COMPLETED 2025-10-29

**Tasks**:
- [x] Note current commit ID of grafana-micromegas-datasource: `git rev-parse HEAD`
- [x] Note current commit ID of micromegas: `git rev-parse HEAD`
- [x] Document any pending work or experimental branches
- [x] Export list of open issues from Grafana repo
- [x] Create migration notes for any environment-specific configs

**Success Criteria**:
- ✅ Commit IDs documented for rollback
- ✅ Rollback plan documented (see bottom of this file)
- ✅ No pending critical work

**Notes**:
- Micromegas current commit on grafana branch: 9ab025c94
- Documentation complete in implementation plan
- Planning phase complete, ready for execution

## Phase 2: Repository Merge

### 2.1 Merge Grafana Plugin with History ✅

**Location**: `micromegas` repo, new branch `grafana-merge`
**Status**: COMPLETED 2025-10-29

**Tasks**:
- [x] Create branch: `git checkout -b grafana-merge`
- [x] Add Grafana repo as remote:
  ```bash
  git remote add grafana-plugin ../grafana-micromegas-datasource
  git fetch grafana-plugin
  ```
- [x] Merge with subtree preserving history:
  ```bash
  git subtree add --prefix=grafana grafana-plugin main
  ```
  Note: Full commit history preserved via merge commit (4bd08dcbd)
- [x] Verify history: `git log --graph --all` shows full history
- [x] Verify all files present: `ls grafana/`

**Success Criteria**:
- ✅ All Grafana files in `grafana/` directory
- ✅ Commit history preserved (accessible via merge commit parent)
- ✅ No conflicts

**Notes**:
- Merge commit: 4bd08dcbd
- Full history accessible via `git log --graph --all`

### 2.2 Create Root Workspace Configuration ✅

**Location**: `micromegas` repo root
**Status**: COMPLETED 2025-10-29

**Tasks**:
- [x] Create root `package.json` with workspaces configuration
- [x] Create `typescript/` directory: `mkdir -p typescript`
- [x] Run `npm install --legacy-peer-deps` to initialize workspaces
- [x] Verify workspace setup: `npm ls --workspaces`
- [x] Update `.gitignore` for Node.js/npm/TypeScript artifacts

**Success Criteria**:
- ✅ Root package.json created (workspaces: grafana, typescript/*)
- ✅ Workspaces directory structure in place
- ✅ `npm install` succeeds (with --legacy-peer-deps due to Grafana dependencies)
- ✅ Workspace links created (@micromegas/types and micromegas-datasource)

**Notes**:
- Excluded doc/ workspaces due to duplicate package name "pres"
- Used --legacy-peer-deps to resolve Grafana peer dependency conflicts
- .gitignore updated with node_modules/, dist/, etc.

### 2.3 Create Shared Type Package ✅

**Location**: `typescript/types/`
**Status**: COMPLETED 2025-10-29

**Tasks**:
- [x] Create directory: `mkdir -p typescript/types/src`
- [x] Create `typescript/types/package.json` with @micromegas/types package configuration
- [x] Create `typescript/types/tsconfig.json` with TypeScript compiler options
- [x] Create initial types in `typescript/types/src/index.ts`:
  - ProcessInfo
  - StreamInfo
  - LogEntry
  - MetricPoint
  - SpanEvent
  - AuthConfig
  - ConnectionConfig
- [x] Build types package: `cd typescript/types && npm run build`
- [x] Verify build artifacts in `typescript/types/dist/`

**Success Criteria**:
- ✅ Types package builds successfully
- ✅ Type definitions generated in dist/ (index.js, index.d.ts)
- ✅ No TypeScript errors

**Notes**:
- Package name: @micromegas/types v0.1.0
- Successfully built with TypeScript 5.4.5
- Ready for use by other workspace packages

### 2.4 Update Grafana Plugin to Use Shared Types

**Location**: `grafana/`
**Status**: SKIPPED - Not applicable

**Rationale**:
The Grafana plugin types (SQLQuery, FlightSQLDataSourceOptions, etc.) are specific to the Grafana plugin API and do not overlap with the general telemetry types created in @micromegas/types (ProcessInfo, StreamInfo, etc.). The shared types package was created for future cross-component type sharing, but the Grafana plugin currently has no types that would benefit from being extracted to the shared package.

**Future Consideration**:
If FlightSQL-specific types need to be shared between the Grafana plugin and other components (e.g., a future web dashboard), this step can be revisited.

### 2.5 Update Root README ✅

**Location**: `micromegas` repo root
**Status**: COMPLETED 2025-10-29

**Tasks**:
- [x] Add Grafana plugin section to root README with quick start
- [x] Update Grafana plugin link in header navigation to point to internal section
- [x] Add documentation links to plugin README

**Success Criteria**:
- ✅ README includes Grafana plugin section
- ✅ Setup instructions clear and concise
- ✅ Links to plugin documentation

**Notes**:
- Added new "Grafana Plugin" section between "Getting Started" and "Current Status & Roadmap"
- Header link updated from external repo to internal anchor
- Quick start instructions included

## Phase 3: Upgrade Dependencies & Align Versions

### 3.1 Upgrade Grafana Plugin TypeScript

**Location**: `grafana/` in monorepo

**Tasks**:
- [ ] Update TypeScript to 5.4 to match analytics-web-app:
  ```bash
  cd grafana
  npm install -D typescript@5.4
  ```
- [ ] Fix any type errors introduced by upgrade
- [ ] Update `tsconfig.json` if needed for TS 5.4 features
- [ ] Run full build to verify: `npm run build`
- [ ] Run tests: `npm run test`

**Success Criteria**:
- All builds pass
- All tests pass
- No TypeScript errors
- Version matches analytics-web-app

### 3.2 Align Development Dependencies

**Location**: `grafana/` in monorepo

**Tasks**:
- [ ] Update ESLint to match analytics-web-app version:
  ```bash
  cd grafana
  npm install -D eslint@^8.57.0 @typescript-eslint/eslint-plugin@latest @typescript-eslint/parser@latest
  ```
- [ ] Update Prettier (if used) to match analytics-web-app
- [ ] Update Jest (if applicable) to match analytics-web-app
- [ ] Run linting: `npm run lint`
- [ ] Run formatting: `npm run format` (or prettier)

**Success Criteria**:
- No linting errors
- Consistent formatting with analytics-web-app
- All builds pass

## Phase 4: CI/CD Update

### 4.1 Update GitHub Actions Workflow

**Location**: `.github/workflows/`

**Tasks**:
- [ ] Create new workflow `.github/workflows/grafana-plugin.yml`:
  ```yaml
  name: Grafana Plugin CI

  on:
    push:
      paths:
        - 'grafana/**'
        - 'typescript/types/**'
        - '.github/workflows/grafana-plugin.yml'
    pull_request:
      paths:
        - 'grafana/**'
        - 'typescript/types/**'

  jobs:
    build:
      runs-on: ubuntu-latest
      steps:
        - uses: actions/checkout@v4
        - uses: actions/setup-node@v4
          with:
            node-version: '20'
            cache: 'npm'

        - name: Install dependencies
          run: npm ci

        - name: Build types
          run: cd typescript/types && npm run build

        - name: Build plugin
          run: cd grafana && npm run build

        - name: Run tests
          run: cd grafana && npm run test

        - name: Lint
          run: cd grafana && npm run lint
  ```
- [ ] Update main CI workflow to skip plugin if not changed
- [ ] Add path filters to existing workflows
- [ ] Test workflow by pushing to branch

**Success Criteria**:
- CI runs only for changed components
- All checks pass
- Build times improved (~50% reduction)

### 4.2 Update Release Workflow

**Location**: `.github/workflows/release.yml` (if exists)

**Tasks**:
- [ ] Add Grafana plugin release step
- [ ] Configure version bumping for plugin
- [ ] Add plugin artifact publishing (if applicable)
- [ ] Update release notes template to include plugin

**Success Criteria**:
- Release workflow includes plugin
- Version management works
- Artifacts published correctly

### 4.3 Add Selective Build Script

**Location**: `scripts/`

**Tasks**:
- [ ] Create `scripts/check-changes.sh`:
  ```bash
  #!/bin/bash
  # Check which components changed

  if git diff --name-only HEAD~1 | grep -q "^grafana/"; then
    echo "grafana=true" >> $GITHUB_OUTPUT
  fi

  if git diff --name-only HEAD~1 | grep -q "^rust/"; then
    echo "rust=true" >> $GITHUB_OUTPUT
  fi

  if git diff --name-only HEAD~1 | grep -q "^typescript/"; then
    echo "typescript=true" >> $GITHUB_OUTPUT
  fi
  ```
- [ ] Make executable: `chmod +x scripts/check-changes.sh`
- [ ] Integrate into CI workflows
- [ ] Test with various change scenarios

**Success Criteria**:
- Script correctly identifies changed components
- CI uses script for selective builds
- Build times reduced

## Phase 5: Documentation

### 5.1 Create Monorepo Development Guide

**Location**: `docs/` or `mkdocs/docs/`

**Tasks**:
- [ ] Create `MONOREPO_GUIDE.md`:
  - Workspace structure explanation
  - Development workflow
  - Building specific components
  - Running tests
  - Common issues and solutions
- [ ] Add cross-component development examples
- [ ] Document shared package usage
- [ ] Add troubleshooting section

**Success Criteria**:
- Guide covers all common scenarios
- Examples are clear and tested
- Troubleshooting comprehensive

### 5.2 Update CONTRIBUTING.md

**Location**: Root `CONTRIBUTING.md`

**Tasks**:
- [ ] Add monorepo-specific guidelines:
  - Where to add new packages
  - How to update shared types
  - PR etiquette for cross-component changes
  - Testing requirements
- [ ] Update setup instructions for new contributors
- [ ] Add workspace command reference
- [ ] Document commit message conventions for monorepo

**Success Criteria**:
- Guidelines clear for monorepo workflow
- New contributor onboarding covered
- Examples provided

### 5.3 Consolidate Grafana Documentation

**Location**: `mkdocs/docs/grafana/` (if using MkDocs)

**Tasks**:
- [ ] Create Grafana documentation section:
  - Installation
  - Configuration
  - Authentication setup
  - Usage examples
  - Troubleshooting
- [ ] Migrate content from grafana/README.md
- [ ] Add cross-references to FlightSQL docs
- [ ] Update mkdocs.yml navigation
- [ ] Simplify grafana/README.md (link to full docs)

**Success Criteria**:
- Comprehensive Grafana docs in MkDocs
- Cross-references work
- Navigation intuitive

### 5.4 Create Setup Script

**Location**: `scripts/setup-dev.sh`

**Tasks**:
- [ ] Create comprehensive setup script:
  ```bash
  #!/bin/bash
  set -e

  echo "Setting up Micromegas development environment..."

  # Check prerequisites
  command -v node >/dev/null 2>&1 || { echo "Node.js required"; exit 1; }
  command -v cargo >/dev/null 2>&1 || { echo "Rust required"; exit 1; }
  command -v python3 >/dev/null 2>&1 || { echo "Python 3 required"; exit 1; }

  # Install npm dependencies
  echo "Installing npm dependencies..."
  npm install

  # Build shared packages
  echo "Building shared packages..."
  cd typescript/types && npm run build && cd ../..

  # Setup Rust
  echo "Setting up Rust workspace..."
  cd rust && cargo build && cd ..

  # Setup Python
  echo "Setting up Python..."
  cd python/micromegas && poetry install && cd ../..

  echo "Setup complete! See MONOREPO_GUIDE.md for next steps."
  ```
- [ ] Make executable: `chmod +x scripts/setup-dev.sh`
- [ ] Test on clean environment
- [ ] Add error handling and helpful messages

**Success Criteria**:
- Script sets up full dev environment
- Error messages helpful
- Works on Linux/macOS

## Phase 6: Testing & Validation (Continuous)

### 6.1 Component Testing

**Tasks**:
- [ ] Test Grafana plugin builds independently
- [ ] Test Rust workspace builds independently
- [ ] Test Python package independently
- [ ] Test shared types package
- [ ] Verify workspace dependencies resolve correctly

**Success Criteria**:
- All components build successfully
- No broken dependencies
- Workspace links working

### 6.2 Integration Testing

**Tasks**:
- [ ] Test cross-component changes (types update → plugin rebuild)
- [ ] Test development workflow (make change, see result)
- [ ] Test CI/CD pipeline end-to-end
- [ ] Test release workflow (if applicable)
- [ ] Test new developer onboarding (setup script + docs)

**Success Criteria**:
- Cross-component updates work smoothly
- CI/CD pipeline robust
- New developer can get started in < 30 minutes

### 6.3 Performance Validation

**Tasks**:
- [ ] Measure repository clone time
- [ ] Measure full build time
- [ ] Measure incremental build time
- [ ] Measure CI pipeline duration
- [ ] Compare to baseline (separate repos)

**Success Criteria**:
- Clone time < 5 seconds
- CI time reduced by ~50%
- Build times acceptable

## Phase 7: Cleanup & Migration

### 7.1 Archive Old Repository

**Location**: `grafana-micromegas-datasource` repo

**Tasks**:
- [ ] Add README to old repo pointing to new location:
  ```markdown
  # ARCHIVED: Grafana Micromegas Datasource

  This repository has been merged into the main Micromegas monorepo.

  **New Location**: https://github.com/madesroches/micromegas/tree/main/grafana

  Please open issues and PRs in the main repository.

  **Last Standalone Commit**: [commit-hash]
  **Merge Date**: 2025-10-29
  ```
- [ ] Archive repository on GitHub
- [ ] Update all external links
- [ ] Notify users/contributors

**Success Criteria**:
- Old repo archived
- Clear migration notice
- All links updated

### 7.2 Update Documentation Links

**Location**: Various

**Tasks**:
- [ ] Update links in main README
- [ ] Update links in blog posts/announcements (if any)
- [ ] Update links in issue templates
- [ ] Search codebase for old repo URLs: `grep -r "grafana-micromegas-datasource"`
- [ ] Update package.json repository URLs

**Success Criteria**:
- No broken links
- All references point to monorepo

### 7.3 Migrate Open Issues

**Location**: GitHub

**Tasks**:
- [ ] Review open issues in old Grafana repo
- [ ] Create corresponding issues in main repo
- [ ] Add "grafana-plugin" label
- [ ] Close issues in old repo with reference to new issues
- [ ] Update issue references in code/docs

**Success Criteria**:
- All open issues migrated
- References updated
- Old issues closed with pointers

## Rollback Plan

If critical issues arise during migration:

### Immediate Rollback (During Phase 2-3)

1. Checkout previous commit: `git checkout <pre-merge-commit-id>`
2. Force push to revert: `git push origin main --force` (only if not merged!)
3. Restore Grafana repo from backup (use noted commit ID)

### Post-Merge Rollback (After Phase 7)

1. Restore Grafana repo from archive
2. Cherry-pick any commits made since merge
3. Update documentation to reflect rollback
4. Investigate issues before re-attempting merge

## Success Metrics

- [ ] All builds pass in monorepo
- [ ] CI/CD pipeline 50% faster
- [ ] No duplicate type definitions
- [ ] Setup script works for new developers
- [ ] Documentation complete and accurate
- [ ] Old repository archived properly

## Communication Plan

### Before Merge
- [ ] Announce merge plan to team/contributors
- [ ] Share study and implementation plan
- [ ] Set merge date
- [ ] Notify external users (if applicable)

### During Merge
- [ ] Post progress updates
- [ ] Notify of any delays
- [ ] Document issues encountered

### After Merge
- [ ] Announce completion
- [ ] Share migration guide
- [ ] Highlight new workflows
- [ ] Offer support for questions

## Resources

- Study Document: `repository_merge_study.md`
- npm workspaces docs: https://docs.npmjs.com/cli/v8/using-npm/workspaces
- TypeScript Project References: https://www.typescriptlang.org/docs/handbook/project-references.html
- GitHub Actions path filters: https://docs.github.com/en/actions/using-workflows/workflow-syntax-for-github-actions#onpushpull_requestpaths

## Notes

- Keep branch `grafana-merge` until fully validated
- Tag major milestones for easy rollback
- Test setup script on clean VM before final merge
- Consider beta period with selected users before full rollout
