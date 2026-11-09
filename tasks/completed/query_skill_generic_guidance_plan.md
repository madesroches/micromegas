# micromegas-query Skill: Absorb Generic Guidance Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1772

## Overview
Now that `list_view_sets()` serves a markdown `description` per view set, deployments keep view-specific guidance there. What remains in a deployment's fork of the `micromegas-query` skill is mostly generic query guidance. This plan moves that guidance upstream into `claude-plugin/skills/micromegas-query/SKILL.md` (eight additions from the issue) so a deployment can shrink to a small overlay of deployment facts.

## Current State
`claude-plugin/skills/micromegas-query/SKILL.md` is the only file involved:
- `## MANDATORY RULES` (lines 12-21): six numbered rules. Rule 4 is "Use `view_instance()`...", rules 5-6 are the `thread_spans` / `async_events` no-global-view rules.
- `## Available views` (lines 211-225): one catalog read, `SELECT view_set_name, description, columns, sort_order FROM list_view_sets() LIMIT 100 ... --format json`.
- `## Performance tips` (lines 349-357) and `## Workflow: narrowing time ranges` (359-374).
- No guidance on binding descriptions, cheap catalog reads, sizing on-demand views, empty-result output, histogram columns, rollups, unbounded `GROUP BY`, or deployment overlays.

The issue's rule numbering ("mandatory rule 4") does not match the file: the time-window rule would be a new rule, not an edit of rule 4. This plan adds a new rule 7 (below).

## Design
Docs-only change to one file; no code, no SQL surface change.

1. **Descriptions are binding.** In `## Available views`, after the catalog read, add a paragraph: windows, mandatory filters and access rules in a view's `description` must be followed like the MANDATORY RULES. Add MANDATORY RULE 7: "For each view, stay within the time window its `description` gives (and apply its mandatory filters)."
2. **Cheap catalog read.** Make the first step `SELECT view_set_name FROM list_view_sets() LIMIT 100`, then `WHERE view_set_name IN (...)` with `description, columns, sort_order` for only the views needed. Note the full read is tens of KB on a deployment with a few dozen views; keep `--format json` because table output truncates cells. Keep the "do not run DESCRIBE" sentence.
3. **Sizing on-demand (JIT) views.** New `### Sizing views` subsection under Performance tips. `thread_spans`, `async_events`, `net_spans`, `otel_spans` and `images` are materialized on demand per `view_instance()` id (`thread_spans` is keyed per stream, the others per `process_id`), so `list_partitions()` row counts reflect past queries, not data volume. Estimate density from source events per stream tag, over a few days, divided by the number of days:
   `SELECT array_to_string("streams.tags", '+') AS tags, sum(nb_objects) AS events FROM blocks GROUP BY 1 ORDER BY events DESC LIMIT 20`.
   Tag-to-view mapping: `cpu` -> `thread_spans` / `async_events` (the `cpu` total covers both), `net` -> `net_spans`, `image` -> `images`, `trace` -> `otel_spans`.
4. **Empty results.** Note that an empty result prints lowercase `no data` for every `--format` (so `json`/`csv` output is a bare, non-parseable string), including a `GROUP BY` matching no row; it is not an error or missing access. For an explicit 0 use a plain `COUNT(*)` without `GROUP BY`.
5. **Histogram columns.** Performance tip: project only the histogram columns needed; they are large and re-aggregated on every read, so `SELECT *` on a view with several histograms multiplies scan cost.
6. **Pre-aggregated views first.** Performance subsection: when the deployment defines per-minute rollups of `measures` (min/max/sum/count per bin), compute MIN/MAX/SUM/COUNT/AVG and find outliers there, then drill into raw `measures` with a tight window only for exact sample times. Include the translation table (`MIN(value)`→`MIN(min_value)`, `MAX(value)`→`MAX(max_value)`, `SUM(value)`→`SUM(sum_values)`, `COUNT(value)`→`SUM(nb_values)`, `AVG(value)`→`SUM(sum_values)/SUM(nb_values)`), stating column names follow the rollup's own `description`.
7. **Unbounded `GROUP BY`.** Performance tip: on a large aggregated view, a `GROUP BY` without a selective filter can exhaust server memory; the group set is the problem and `LIMIT` does not help. Filter on a selective column (typically the leading `sort_order` column) before grouping broadly. Also append to SKILL.md line 21 (complementing rule 2): "...as can a `GROUP BY` without a selective filter on a large view (see Performance tips) — `LIMIT` caps output, not the group set".
8. **Deployment overlay hook.** New `## Deployment notes` section, placed before `## Available views`: if `~/.micromegas/query-notes.md` exists, read it before the catalog read (the "First step" of `## Available views`); where its notes conflict with a view `description`, the notes win only by being stricter (narrower windows, extra filters), never by loosening a `description` or the MANDATORY RULES. The same applies to deployment notes given in context. Typical content: approximate row counts, stricter windows for the largest views, preferred subset views over `log_entries`, environment conventions. The path sits beside `config.json`, and `Read` is already in `allowed-tools` so no tool allowlist change is needed.

## Implementation Steps
1. Edit `claude-plugin/skills/micromegas-query/SKILL.md`: add MANDATORY RULE 7; rewrite the first step of `## Available views` (items 1, 2, 4); add `## Deployment notes` (item 8); add performance additions and `### Sizing views` (items 3, 5, 6, 7).
2. Add a `CHANGELOG.md` entry under Unreleased with the bold prefix `**Claude Code Plugin:**` describing the skill's new generic guidance, citing (#1772). State "No SQL changes."

## Files to Modify
- `claude-plugin/skills/micromegas-query/SKILL.md`
- `CHANGELOG.md`

## Trade-offs
- **Overlay as a notes file vs. plugin**: a plain file read by the skill needs no plugin machinery and works with the existing `Read` permission; a second plugin would need a new loading convention. The conflict-resolution rule is stated in the skill so both a file and in-context notes behave the same.
## Decisions
- Notes file location is `~/.micromegas/query-notes.md`, next to `config.json`.

## Documentation
No mkdocs page duplicates the skill's guidance; no doc update needed.

## Testing Strategy
No automated tests apply to a prompt file. Verification is review that every item 1-8 of the issue is present and the examples are syntactically valid SQL for the stated views.

## Manual Verification
1. Run `micromegas-query "SELECT array_to_string(\"streams.tags\", '+') AS tags, sum(nb_objects) AS events FROM blocks GROUP BY 1 ORDER BY events DESC LIMIT 20" --begin 3d` against a local environment (`python3 local_test_env/ai_scripts/start_services.py`); expect rows or `no data` on an empty lake. Not automated because it needs a live service and the change is documentation only.

## Open Questions
None.
