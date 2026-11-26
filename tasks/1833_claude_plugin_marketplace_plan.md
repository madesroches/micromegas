# Claude Code Plugin Marketplace Manifest Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1833

## Overview
The repo ships a Claude Code plugin in `claude-plugin/`, but has no marketplace manifest, so it
can't be installed with `/plugin marketplace add madesroches/micromegas` followed by
`/plugin install micromegas@micromegas`. This plan adds `.claude-plugin/marketplace.json` at the
repo root, makes installed copies pick up skill updates, and documents installation.

## Current State
- `claude-plugin/.claude-plugin/plugin.json` declares `name: "micromegas"`,
  `version: "0.1.0"`, a description, author, homepage and license.
- `claude-plugin/skills/{micromegas-query,micromegas-notebooks}/SKILL.md` are copies of the
  canonical skills in `rust/public/src/servers/mcp/skills/`, synced by
  `build/sync_claude_plugin_skills.py`; `rust/public/tests/mcp_skills_tests.rs`
  (`claude_plugin_copies_match_canonical_skills`) fails when they drift.
- No `.claude-plugin/` directory exists at the repo root. Users either run
  `claude --plugin-dir <clone>/claude-plugin` every session or copy `SKILL.md` by hand.
- The docs never describe how to install the plugin. `mkdocs/docs/query-guide/index.md` lists
  the query interfaces (Python API, Grafana plugin).
- The plugin has no README.

## Design

### Marketplace manifest
New file `.claude-plugin/marketplace.json` at the repo root (the marketplace root is the
directory containing `.claude-plugin/`; relative sources are written from there):

```json
{
  "name": "micromegas",
  "description": "Claude Code plugins for the Micromegas observability platform",
  "owner": { "name": "Micromegas" },
  "plugins": [
    {
      "name": "micromegas",
      "source": "./claude-plugin",
      "description": "Query Micromegas telemetry with SQL and author notebooks"
    }
  ]
}
```

The entry `name` must equal the manifest `name` in `plugin.json` (`micromegas`), otherwise
installing by name fails with `Plugin "<name>" not found in marketplace`.

### Updates: drop the pinned `version`
Claude Code resolves a plugin's version from `plugin.json`'s `version` first, then the
marketplace entry's `version`, and only when neither is set from the commit SHA of the installed
directory (for a relative path in a Git-hosted marketplace). A pinned `"version": "0.1.0"` keeps
every user on their cached copy until someone remembers to bump it. Remove `version` from
`plugin.json` (and don't set one in the marketplace entry) so installed copies update whenever
the plugin directory changes. This replaces the issue's suggestion of bumping the version on
skill changes with a mechanism that can't be forgotten.

### Consistency test
Add a no-DB test to `rust/public/tests/mcp_skills_tests.rs`, next to
`claude_plugin_copies_match_canonical_skills` and skipped the same way when the plugin directory
is absent (crate packaged outside the repo). It parses `.claude-plugin/marketplace.json` and
`claude-plugin/.claude-plugin/plugin.json` with `serde_json` and asserts:
- the marketplace has an entry whose `source` resolves (from the repo root) to the
  `claude-plugin/` directory, and that directory exists;
- that entry's `name` equals `plugin.json`'s `name`;
- neither `plugin.json` nor the entry carries a `version` field (a pin would silently freeze
  users on an old copy).

These failures are silent for maintainers (installs break or stop updating only for users), so
they warrant a test. If `serde_json` is not available to this test target with the features the
test file already requires, add it as a dev-dependency.

### Documentation
Add a `### Claude Code Plugin` subsection under `## Interfaces` in
`mkdocs/docs/query-guide/index.md`, after the Grafana plugin, with:
- the two install commands (`/plugin marketplace add madesroches/micromegas`,
  `/plugin install micromegas@micromegas`) and `/plugin marketplace update micromegas` to pull
  updates;
- one line naming the two skills it provides (`micromegas-query`, `micromegas-notebooks`) and
  that `micromegas-query` needs the `micromegas` Python package / CLI (its `## Setup` section
  walks through this);
- a note that a deployment shipping its own copy of the plugin under the same `micromegas` name
  clashes with this one (same plugin and skill names): keep only one enabled, e.g.
  `/plugin disable micromegas@micromegas`.

Add a one-line mention with a link to that section in `README.md` next to the existing MCP
server bullet. No plugin README is created.

## Implementation Steps
1. Create `.claude-plugin/marketplace.json` as above.
2. Remove `"version"` from `claude-plugin/.claude-plugin/plugin.json`.
3. Add the consistency test to `rust/public/tests/mcp_skills_tests.rs`.
4. Add the docs subsection to `mkdocs/docs/query-guide/index.md` and the README line.
5. Add a `CHANGELOG.md` entry under `## Unreleased` (`**Claude Code Plugin:**`), citing #1833.

## Files to Modify
- `.claude-plugin/marketplace.json` (new)
- `claude-plugin/.claude-plugin/plugin.json`
- `rust/public/tests/mcp_skills_tests.rs`
- `rust/public/Cargo.toml` (only if a `serde_json` dev-dependency is needed)
- `mkdocs/docs/query-guide/index.md`
- `README.md`
- `CHANGELOG.md`

## Trade-offs
- **Commit-SHA versioning vs. bumping `version`.** Bumping on every skill change is a manual
  step that is easy to miss, and a missed bump leaves users stale with no error. Dropping the
  field makes every change to the plugin directory an update. Cost: there is no human-readable
  plugin version; acceptable since the plugin ships from the repo's main branch anyway.
- **Marketplace at the repo root vs. a separate repo.** The issue's install command
  (`madesroches/micromegas`) requires the manifest at this repo's root, and a separate repo would
  need its own sync of the skills.
- **Docs location.** The query guide's Interfaces list is where users look for ways to query;
  `admin/mcp.md` covers the server-side MCP route, which is a different integration.

## Documentation
- `mkdocs/docs/query-guide/index.md`: new Claude Code Plugin subsection.
- `README.md`: one-line pointer.
- `CHANGELOG.md`: Unreleased entry.

## Testing Strategy
- New unit test in `mcp_skills_tests.rs` (no DB) covering name match, source path, and absence
  of a pinned version.
- Existing `claude_plugin_copies_match_canonical_skills` keeps guarding skill drift.
- `python3 build/rust_ci.py` (or the equivalent `cargo test -p micromegas --test
  mcp_skills_tests`) and the mkdocs build/link check (`build/check_docs_site.py` if applicable).

## Manual Verification
Only Claude Code itself can confirm the manifest is accepted; there is no CI install of it.
1. `claude plugin validate .` from the repo root — expect `✔ Validation passed`.
2. `claude plugin marketplace add ./` then `claude plugin install micromegas@micromegas` —
   expect `✔ Installed plugin: micromegas@micromegas`, and `claude plugin details micromegas`
   lists both skills. Remove afterwards with `claude plugin marketplace remove micromegas`.
