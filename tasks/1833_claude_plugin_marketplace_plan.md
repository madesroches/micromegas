# Claude Code Plugin Marketplace Manifest Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1833

## Overview
The repo ships a Claude Code plugin in `claude-plugin/`, but has no marketplace manifest, so it
can't be installed with `/plugin marketplace add madesroches/micromegas` followed by
`/plugin install micromegas@micromegas`. This plan adds `.claude-plugin/marketplace.json` at the
repo root, bumps the plugin version so installed copies see the new skills as an update, and
documents installation. It also
makes the `micromegas-query` skill cover the commonly used table functions (e.g. `process_spans`)
and point to the functions reference for niche ones.

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
installing by the manifest name fails with `Plugin "<manifest-name>" not found in marketplace`.

### Updates: bump the pinned `version`
Claude Code resolves a plugin's version from `plugin.json`'s `version` first, then the
marketplace entry's `version`, then the commit SHA of the installed directory. Keep the pin in
`plugin.json` and bump it from `0.1.0` to `0.2.0`, since this branch changes the
`micromegas-query` skill; don't set a `version` in the marketplace entry. Installed copies only
see a change as an update when this string changes, so add one sentence to the
`build/sync_claude_plugin_skills.py` docstring saying to bump `version` in
`claude-plugin/.claude-plugin/plugin.json` whenever a synced skill changes.

### Consistency test
Add a no-DB test to `rust/public/tests/mcp_skills_tests.rs`, next to
`claude_plugin_copies_match_canonical_skills` and skipped only when
`CARGO_MANIFEST_DIR/../../claude-plugin` is absent (crate packaged outside the repo); otherwise a
missing or unparsable `.claude-plugin/marketplace.json` fails. It parses `.claude-plugin/marketplace.json` and
`claude-plugin/.claude-plugin/plugin.json` with `serde_json` and asserts:
- the marketplace has an entry whose `source` resolves to the plugin directory:
  `canonicalize(repo_root.join(source))` equals `canonicalize(repo_root.join("claude-plugin"))`;
- that entry's `name` equals `plugin.json`'s `name`;
- the marketplace entry carries no `version` field (the version lives only in `plugin.json`).

### Documentation
Add a `### Claude Code Plugin` subsection under `## Interfaces` in
`mkdocs/docs/query-guide/index.md`, after the Grafana plugin, with:
- the two install commands (`/plugin marketplace add madesroches/micromegas`,
  `/plugin install micromegas@micromegas`), and how to update the installed plugin:
  `claude plugin update micromegas@micromegas` (shell) or the update action in the `/plugin`
  UI, or enable auto-update for the marketplace in `/plugin` (off by default for third-party
  marketplaces), then `/reload-plugins` or restart — `/plugin marketplace update` alone only
  refreshes the listing;
- one line naming the two skills it provides (`micromegas-query`, `micromegas-notebooks`) and
  that `micromegas-query` needs the `micromegas` Python package / CLI (its `## Setup` section
  walks through this);
- a note that a deployment shipping its own copy of the plugin under the same `micromegas` name
  clashes with this one (same plugin and skill names): keep only one enabled, e.g.
  `/plugin disable micromegas@micromegas`.

Add a one-line bullet with a link to that section in `README.md` under `## Key Features`. No
plugin README is created.

### Query skill: table function coverage
Edit the canonical `rust/public/src/servers/mcp/skills/micromegas-query/SKILL.md`, then refresh
the plugin copy with `python3 build/sync_claude_plugin_skills.py`
(`claude_plugin_copies_match_canonical_skills` enforces it).
- **Common table functions** (in `## Key functions` > `### Table functions`) get a
  self-sufficient entry. Expand `process_spans`: the three `types` values and what each returns,
  the prepended `stream_id` and `thread_name` columns (async spans: empty `stream_id`,
  `thread_name` `'async'`), key columns including `duration` (ns) and `depth`, the time range
  coming from `--begin`/`--end` rather than arguments, and the existing density warning.
  Optionally add a `'both'` per-thread example to the `### Span analysis with process_spans`
  pattern. Add a `jsonb_array_elements` entry, and reword the `jsonb_each` entry's
  literal/uncorrelated-subquery restriction so it is stated once, covering both functions. Keep `list_view_sets` referenced (covered in `## Available views`).
- **Niche functions** get a one-line pointer to the functions reference
  (`mkdocs/docs/query-guide/functions-reference.md`, linked from `## Reference documentation`)
  rather than full coverage: the table functions `perfetto_trace_chunks` and `parse_block`, and
  the niche scalar groups (color, binning, math functions,
  `properties_length`/`properties_to_dict`/`properties_to_array`, `get_payload`).
  `list_audience_grants`, `list_query_denials`, `check_view_merge` and
  `list_view_set_definitions` get a separate pointer to the admin functions reference
  (https://micromegas.info/docs/admin/functions-reference/); the last three are admin-only
  (non-admins get not-found). The mutating/admin functions are not named in the skill; it says
  they are documented in the admin reference.

## Implementation Steps
1. Create `.claude-plugin/marketplace.json` as above.
2. Bump `"version"` in `claude-plugin/.claude-plugin/plugin.json` to `0.2.0`, and add the bump
   reminder to the `build/sync_claude_plugin_skills.py` docstring.
3. Add the consistency test to `rust/public/tests/mcp_skills_tests.rs`.
4. Add the docs subsection to `mkdocs/docs/query-guide/index.md` and the README line.
5. Add `claude-plugin/**` and `.claude-plugin/**` to the `paths` lists (both `push` and
   `pull_request`) in `.github/workflows/rust.yml` and to `paths-ignore` in
   `.github/workflows/build-skip.yml`, so manifest-only changes run the consistency test.
6. Update the canonical `micromegas-query` skill as described in "Query skill: table function
   coverage" and run `python3 build/sync_claude_plugin_skills.py`.
7. Create `## Unreleased` above `## v0.34.0 - 2026-11-26` in `CHANGELOG.md` with a
   `**Claude Code Plugin:**` entry citing #1833, mentioning the query skill's table function
   coverage.

## Files to Modify
- `.claude-plugin/marketplace.json` (new)
- `claude-plugin/.claude-plugin/plugin.json`
- `build/sync_claude_plugin_skills.py` (docstring only)
- `rust/public/tests/mcp_skills_tests.rs`
- `.github/workflows/rust.yml`
- `.github/workflows/build-skip.yml`
- `rust/public/src/servers/mcp/skills/micromegas-query/SKILL.md`
- `claude-plugin/skills/micromegas-query/SKILL.md` (regenerated by the sync script)
- `mkdocs/docs/query-guide/index.md`
- `README.md`
- `CHANGELOG.md`

## Documentation
- `mkdocs/docs/query-guide/index.md`: new Claude Code Plugin subsection.
- `README.md`: one-line pointer under `## Key Features`.
- `CHANGELOG.md`: Unreleased entry.

## Testing Strategy
- New unit test in `mcp_skills_tests.rs` (no DB) covering name match, source path, and no
  `version` on the marketplace entry.
- Existing `claude_plugin_copies_match_canonical_skills` keeps guarding skill drift.
- `python3 build/rust_ci.py` (or the equivalent, from `rust/`:
  `cargo test -p micromegas --features server --test mcp_skills_tests`) and
  `python mkdocs/build-docs.py` (strict mkdocs build) passes.
- The query skill change is verified by reading; the copy-sync test guards the plugin copy.

## Manual Verification
Only Claude Code itself can confirm the manifest is accepted; there is no CI install of it.
1. `claude plugin validate .` from the repo root — expect `✔ Validation passed`.
2. `claude plugin marketplace add ./` then `claude plugin install micromegas@micromegas` —
   expect `✔ Installed plugin: micromegas@micromegas`, and `claude plugin details micromegas`
   lists both skills. Remove afterwards with `claude plugin marketplace remove micromegas`.

## Decisions
- The user added the query skill table-function coverage (common functions in full, niche ones
  pointed to the reference) to this branch's scope.
- The user asked to bump the plugin version (keep the `plugin.json` pin, `0.1.0` → `0.2.0`)
  rather than drop it for commit-SHA versioning.
- AFK: Documented update path is `claude plugin update micromegas@micromegas` / the `/plugin` UI
  update / opt-in marketplace auto-update, then `/reload-plugins` or restart, stated once in the
  Documentation bullet; `/plugin marketplace update` was dropped as the update command since it
  only refreshes the listing, and the duplicate list in the Updates section was removed.
