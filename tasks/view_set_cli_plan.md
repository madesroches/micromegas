# View Sets as Code CLI (Issue #1603) Plan

## Overview

A `micromegas-views` console script that treats a directory of `.sql` files as the desired state of
the deployment's DDL-defined materialized view sets, and the server as the current state, with a
terraform-shaped `plan` / `apply` / `pull` / `list` / `show` workflow. Nothing new is stored: the
desired state is the files (in git), the current state is `lakehouse_view_set_definitions` read
through the existing admin UDTF `list_view_definitions()`, and the write path is the existing
`CREATE [OR REPLACE] MATERIALIZED VIEW` / `DROP MATERIALIZED VIEW` DDL executed over FlightSQL. This
is a client-side diff/apply layer and no server change at all.

`micromegas-screens` already implements this exact workflow for screens
(`python/micromegas/micromegas/cli/screens.py`). So the second deliverable here is the extraction of
the parts that are not screen-specific — the colorized unified diff, the apply confirmation gate,
the `--color` flag wiring — into `cli/state_sync.py`, which both tools then use. The new tool is not
a copy of the old one with the nouns changed.

## Current State

**The DDL surface.** `parse_view_ddl` (`rust/public/src/servers/view_ddl.rs:68-142`) hand-parses
`CREATE [OR REPLACE] MATERIALIZED VIEW <name> WITH (...)` and `DROP MATERIALIZED VIEW [IF EXISTS]
<name>` off a single `sqlparser` pass, and `execute_view_ddl`
(`rust/public/src/servers/flight_sql_service_impl.rs:991-1140`) validates and applies it inside one
transaction under an advisory lock. Every statement returns a one-row result set of `(view_set_name,
status)`, where status is `created`, `replaced`, `dropped`, or `not_found` (the last only from a
`DROP ... IF EXISTS` that matched nothing). A plain `CREATE` against an existing name is an
`already_exists` error (`:1020-1025`); a `DROP` of a view another definition reads is refused by
`check_dependents_survive` (`:1118-1128`) with no `CASCADE`.

**The current-state read.** `list_view_definitions()`
(`rust/analytics/src/lakehouse/list_view_definitions_table_function.rs`) returns one row per stored
definition with `(view_set_name, definition_sql, update_group, updated_at, updated_by)`, read
straight from Postgres and ordered by name, so it also shows a definition that failed to load.
`definition_sql` is **the verbatim statement text as submitted** — `ViewDdl::Create::sql` is
`sql.to_string()` of the whole statement (`view_ddl.rs:34`, `:113`), stored unmodified by
`upsert_tx` (`view_definition_store.rs:127-160`). The normalized columns
(`extract_query`, `count_src_query`, `view_options`, ...) exist in the table but are not exposed by
the UDTF.

**`log_stats` is one of these rows.** The lakehouse migration seeds it into
`lakehouse_view_set_definitions` and `default_view_factory` no longer builds it
(`rust/analytics/src/lakehouse/view_factory.rs:57`, `log_stats_view.rs:63`). So a fresh deployment
already has exactly one stored definition that nobody checked into a views directory.

**The sibling tool.** `micromegas-screens` (`python/micromegas/micromegas/cli/screens.py`, 744
lines) is the precedent for everything structural here: `compute_plan` returning
`(creates, updates, deletes, unchanged, untracked)` (`:360-450`), `format_screen_diff` producing a
4-space-indented colorized `difflib.unified_diff` (`:438-463`), `format_plan` (`:466-505`),
`cmd_apply`'s `[y/N]` gate plus `--auto-approve` (`:508-598`), and a `client_args` parent parser
holding `--profile`/`--no-auth` defined exactly once to dodge the argparse shared-Namespace trap
(`:665-676`). Its unit tests (`python/micromegas/tests/test_screen_files.py`) drive the `cmd_*`
functions directly against a fake client.

**Auth.** `micromegas-screens` and its `WebClient` siblings resolve auth through
`web_auth.resolve_web_auth()`, which branches only on OIDC fields. A FlightSQL tool instead uses
`micromegas.connection.connect_with_profile(profile=..., client_entrypoint=...)`
(`python/micromegas/micromegas/connection.py:14`), which honors `api_key_file`, then OIDC, then
falls back to unauthenticated — so no `--no-auth` flag is needed, and a static API key works in CI
with no browser. `mkdocs/docs/query-guide/python-api.md:842-849` currently states that
`micromegas-query` and `connect_with_profile()` are the only `api_key_file` honorers; the new tool
joins that list. Every subcommand needs an admin identity: `list_view_definitions` is registered
only inside the `lakehouse_admin` block (`rust/analytics/src/lakehouse/query.rs:237-255`), so even
read-only `list`/`show`/`plan` fail as an unknown-function planner error for a non-admin caller,
and `authorize_view_ddl` (`view_ddl.rs:49-55`) gates the write path the same way.

**Today's workflow.** Hand-write the statement and pipe it through `micromegas-query --file`. There
is no preview, no desired-state file, and no way to tell whether the server matches the repo.

## Design

### 1. Command surface

```
micromegas-views plan  [names...] [--dir D] [--profile P] [--prune] [--color/--no-color]
micromegas-views apply [names...] [--dir D] [--profile P] [--prune] [--auto-approve] [--color/--no-color]
micromegas-views pull  [names...] [--dir D] [--profile P]
micromegas-views list         [--dir D] [--profile P] [--format table|json]
micromegas-views show  <name> [--dir D] [--profile P] [--local]
```

There is no `init` subcommand and no config file. `micromegas-screens` needs
`micromegas-screens.json` for a server URL and a `managed_by` ownership marker; this tool gets its
endpoint from the standard `--profile` resolution and has no ownership marker to record (see §5).
`--dir` defaults to `.`.

One desired-state file per view set: `<view_set_name>.sql`, holding exactly one
`CREATE [OR REPLACE] MATERIALIZED VIEW` statement. Nothing else in the directory is read.

Bare `pull` (no names) refreshes only the files already present in `--dir` — the
`screens.py:cmd_pull` default. A named `pull` also adopts a server-only name into a new file, which
is the merged pull/import behavior §5 relies on. `pull` skips, with a warning, any target file that
fails to decode or whose header does not parse, rather than overwriting it — mirroring
`screens.py:cmd_pull`'s (`:337-352`) guard against clobbering a file that can't be safely read.

### 2. Local file model and parsing

A `.sql` file is read as text (`encoding="utf-8-sig"`, matching `screens.py:38,57`) and reduced to a
`LocalDefinition`:

```python
LocalDefinition = namedtuple("LocalDefinition", "name update_group text path")
```

Only the statement *header* is parsed locally — never the three queries. The header scan runs over a
copy of the text with dollar-quoted bodies and SQL comments blanked out, so an `update_group = 1`
sitting inside an `extract_query` body cannot be mistaken for the option:

```python
def _strip_bodies(text):
    """Blank out $$...$$ bodies, single-quoted string literals (handling '' escapes),
    /*...*/ and -- comments, for header scanning only."""
```

From the stripped copy:

- `name` — from `CREATE\s+(OR\s+REPLACE\s+)?MATERIALIZED\s+VIEW\s+(<ident>)`. Must equal the
  filename stem, or the file is rejected: the diff, the plan output, and `pull` all key on the
  filename, and a mismatch would silently apply a view under a name the repo doesn't show.
- `update_group` — from `\bupdate_group\s*=\s*'?(\d+)'?`, matching both the plain-integer and
  single-quoted-integer forms `value_to_i32` accepts (`view_ddl.rs:169-183`). Used **only** for
  apply ordering and plan display; the server remains the authority on whether the value is legal.
  A file with no extractable `update_group` is not rejected — it sorts last, and the resulting
  ordering may be wrong for that file, which keeps this regex from becoming a second, weaker copy
  of the Rust validator.

A file whose header does not parse at all, or whose name disagrees with its filename, is reported
and skipped. Following `screens.py`'s two-tier handling (`:94-157`, `:396-425`): a name/filename
mismatch suppresses pruning for both the filename stem and the name the header actually declares,
since either could be the definition the author meant to manage; a header that yields no name at
all suppresses pruning repo-wide, because there is no name to scope the suppression to.

`list_local_definitions(dir)` returns the scan as one tuple, `(definitions, unparseable_stems,
mismatched_names)`, mirroring `screens.py`'s `local, unreadable, invalid_names = local_scan or
list_local_screens()`: `definitions` maps name to `LocalDefinition`, `unparseable_stems` is the
repo-wide prune suppression set, and `mismatched_names` is the per-name prune suppression set. This
is the tuple `compute_plan` (§4) takes as its `local_scan` argument.

### 3. Comparison: canonical statement text

The server stores verbatim text, so the comparison is a text comparison over one canonical form
applied to both sides:

```python
def canonical_ddl(text):
    # 1. CRLF/CR -> LF
    # 2. strip leading/trailing whitespace
    # 3. drop a single trailing ';', then re-strip
    # 4. rewrite the leading 'CREATE [OR REPLACE] MATERIALIZED VIEW' keyword run
    #    (after any leading comments) to exactly 'CREATE MATERIALIZED VIEW'
```

Step 4 is what makes the round trip stable: `apply` may send `OR REPLACE` where the file says plain
`CREATE` (§4), and a definition created by hand before this tool existed may say either. Collapsing
the keyword run also absorbs interior whitespace inside it.

Interior whitespace is **not** normalized beyond that. Collapsing whitespace runs inside the
statement would silence reformat-only diffs, but it would also equate two definitions whose string
literals genuinely differ (`'a  b'` vs `'a b'`) — reporting `unchanged` for a server that does not
match the repo. A reformat-only edit therefore shows up as an update, and applying it is harmless:
the schema hash is derived from the inferred Arrow schema, not the query text
(`mkdocs/docs/admin/materialized-views.md`, "What a redefinition means"), so existing partitions
stay valid and the only effect is a `CREATE OR REPLACE` round trip and a registry reload.

`plan`'s per-view diff is a `difflib.unified_diff` over the canonical text of both sides
(`fromfile="server"`, `tofile="local"`), so the `OR REPLACE` bookkeeping never appears in output.

### 4. The plan model

```python
def compute_plan(client, local_scan, names=None, prune=False):
    """-> (creates, updates, deletes, unchanged, unmanaged)"""
```

`local_scan` is the `(definitions, unparseable_stems, mismatched_names)` tuple `list_local_definitions`
returns (§2), unpacked exactly as `screens.py:compute_plan` unpacks `local_scan` — `unparseable_stems`
suppresses pruning repo-wide, `mismatched_names` suppresses it for those two names, and an empty
`definitions` with `prune=True` is the "zero readable `.sql` files" case §5 refuses to run.

`client` is anything with `.query(sql)` returning a DataFrame — the real FlightSQL client in
production, a canned-DataFrame fake in tests. Current state is one call:

```sql
SELECT view_set_name, definition_sql, update_group, updated_at, updated_by
  FROM list_view_definitions()
```

No name is ever interpolated into SQL. `show <name>` and a named-subset `plan` fetch every row and
filter in pandas; the table holds one row per view set, so there is nothing to optimize and no
quoting to get wrong.

Classification, per local file:

| Local | Server | Outcome |
|---|---|---|
| present | absent | `create` |
| present | present, canonical text differs | `update` |
| present | present, canonical text equal | `unchanged` |
| absent | present | `unmanaged`, or `delete` when `--prune` **and** `names` is empty |

Deletes are computed only in whole-repo mode: `--prune` combined with an explicit `names` subset
drops nothing (matching `screens.py:compute_plan`'s `if not names` fence around its delete loop,
`screens.py:396-425`) — a named-subset run has no way to know whether a view set outside the
subset is still managed elsewhere in the directory, so treating "not mentioned" as "delete" would
be wrong.

**Apply order.** All creates and updates run first, then all drops — matching `screens.py:cmd_apply`
(`:549-586`) — so that an update which removes a view's dependency on a to-be-pruned view lands
before the drop, rather than having `check_dependents_survive` refuse it. Within the create/update
phase, statements run in ascending `update_group` (ties broken by name); within the drop phase, in
descending server `update_group`. `update_group` must be strictly greater than that of every view a
definition reads, so that ordering is a valid topological order in both directions — which matters
because the server validates a `CREATE` against the definitions already stored
(`flight_sql_service_impl.rs:1032-1046`) and refuses a `DROP` whose dependent survives.

**Statement sent.** For an update, the file text with `OR REPLACE` injected into the header when
absent (the inverse of `canonical_ddl`'s step 4). For a create, the file text verbatim: if the file
says plain `CREATE` and someone created that view between `plan` and `apply`, the server's
`already_exists` error surfaces the race instead of silently clobbering it. For a drop, `DROP
MATERIALIZED VIEW <name>` without `IF EXISTS` — the plan just established it exists, and a
`not_found` status is worth an error rather than a shrug.

`apply` reports each statement's returned `(view_set_name, status)` row, continues past a failure
(counting it, as `screens.py:cmd_apply` does), and exits non-zero if any failed. The exceptions
caught per statement are `pyarrow.flight.FlightError` and `pyarrow.lib.ArrowInvalid` — what
`FlightSQLClient.query` actually raises (`flightsql/client.py:355-419`,
`tests/test_ddl_materialized_view.py:263`), not the `RuntimeError` `screens.py`'s `WebClient`
raises. `connect_with_profile`'s `ProfileError` (a `ValueError`) is caught once at connect time,
as the other CLIs do (`query.py:141-144`). Each DDL statement is its own server-side transaction,
so a partial apply is a real outcome; the workflow is idempotent, so the remedy is re-running
`apply`.

### 5. Deletes are opt-in

`lakehouse_view_set_definitions` has no `managed_by` column, so — unlike `micromegas-screens`, which
can tell its own screens from someone else's — this tool cannot distinguish "a view set this repo
used to manage and that was removed from the repo" from "a view set this repo never managed."
Server-only definitions are therefore reported as `unmanaged` and **never dropped** unless `--prune`
is passed. Two guards on top:

- `--prune` only ever considers deletes in whole-repo mode: `--prune` alongside an explicit
  `names` list drops nothing (§4). A named-subset run cannot tell a view set outside the subset
  apart from one this repo never managed, so it cannot safely prune it either.
- `--prune` refuses to run when the directory contains zero readable `.sql` files. A wrong `--dir`
  or a checkout at the wrong commit is the plausible way to ask a reconciler to drop production
  views, and an empty desired state is the signature of it.
- `--prune` still routes through the same confirmation gate, and `--auto-approve` still bypasses the
  gate (CI needs it) — the explicitness lives in `--prune` itself.

A fresh deployment lists `log_stats` as unmanaged, since the migration seeds it. `pull log_stats`
adopts it into the directory; leaving it unmanaged is equally fine.

A `DROP` also retires the view's partitions (`flight_sql_service_impl.rs:1092-1108`), so pruning is
destructive beyond the definition row. Worth one line in the docs.

### 6. Shared module: `cli/state_sync.py`

Extracted from `screens.py`, used by both tools:

- `colorize_diff(diff_lines, use_color)` — the `---`/`+++`/`@@`/`-`/`+` coloring and 4-space indent,
  currently the back half of `format_screen_diff` (`screens.py:445-463`).
- `unified_diff(before_lines, after_lines, from_label, to_label, use_color)` — `difflib` plus the
  above; returns `""` when there is no difference.
- `confirm_apply(auto_approve)` — the `[y/N]` prompt, the `Apply cancelled.` message, and
  `sys.exit(1)` (`screens.py:531-535`).
- `add_color_arg(parser)` — the `--color` `BooleanOptionalAction` default-`True` flag.
- `use_color(args)` — `sys.stdout.isatty() and args.color`.

`screens.py` keeps `format_screen_diff` as a thin wrapper (JSON-serialize both sides, call
`unified_diff`), so `tests/test_screen_files.py`'s existing imports and assertions keep passing
unchanged. Nothing screen-specific moves: `managed_by`, the untracked/ownership model, the
`compute_plan` tuple shape, and `WebClient` construction all stay in `screens.py`. The shared module
is the presentation and prompt layer only — that is the part that is genuinely identical, and
generalizing the plan model itself across a `managed_by`-having and a `managed_by`-lacking resource
would cost more than it saves.

### 7. Output

```
$ micromegas-views plan
micromegas-views will perform the following actions:

  + create: request_stats (update_group 4000)
  ~ update: log_stats (update_group 3000)
    --- server
    +++ local
    @@ -3,7 +3,7 @@
    -    WHERE insert_time >= '{begin}' AND insert_time < '{end}'
    +    WHERE insert_time >= '{begin}' AND insert_time < '{end}' AND level <= 4

Plan: 1 to create, 1 to update, 0 to drop, 2 unchanged.

Unmanaged view sets on server (use 'pull' to adopt, '--prune' to drop):
  ? error_rollup
```

`list` prints name / status (`synced`, `modified`, `local-only`, `server-only`) / `update_group` /
`updated_at` / `updated_by`, with `--format json`. `show <name>` prints the server's stored
`definition_sql`; `--local` prints the file instead.

## Implementation Steps

**Phase 1 — shared module**

1. Create `python/micromegas/micromegas/cli/state_sync.py` with the five helpers in §6.
2. Refactor `python/micromegas/micromegas/cli/screens.py` to use them: `format_screen_diff` becomes
   a wrapper over `unified_diff`; `cmd_apply`'s inline prompt becomes `confirm_apply`; the two
   `--color` definitions and the two `sys.stdout.isatty() and args.color` expressions become
   `add_color_arg` / `use_color`.
3. Run `python/micromegas/tests/test_screen_files.py` and `tests/cli/test_screens_auth.py` — they
   must pass untouched. That is the check that the extraction changed no behavior.

**Phase 2 — the new tool**

4. Create `python/micromegas/micromegas/cli/views.py`: `_strip_bodies`, `parse_local_definition`,
   `list_local_definitions(dir)`, `canonical_ddl`, `with_or_replace`, `compute_plan`, `format_plan`,
   `cmd_plan`, `cmd_apply`, `cmd_pull`, `cmd_list`, `cmd_show`, `main`. `cmd_apply` catches
   `pyarrow.flight.FlightError` and `pyarrow.lib.ArrowInvalid` per statement; `main` catches
   `ProfileError` at connect time (§4). `cmd_pull` writes with `encoding="utf-8"`, matching
   `screens.py:89,256`; `main` calls `sys.stdout.reconfigure(encoding="utf-8",
   errors="backslashreplace")` before dispatching, matching `screens.py:651`, so colorized diff
   output survives a non-UTF-8 locale.
5. Wire the argparse surface of §1, with a `client_args` parent parser carrying `--profile` and
   `--dir` (defined exactly once — see `screens.py:665-676` for why), and call
   `add_version_argument(parser)` so `--version` matches the other six console scripts.
6. Add `micromegas-views = "micromegas.cli.views:main"` to `[tool.poetry.scripts]` in
   `python/micromegas/pyproject.toml`.

**Phase 3 — tests and docs**

7. Add `python/micromegas/tests/cli/test_views.py` (§ Testing Strategy).
8. Write `mkdocs/docs/admin/views-as-code.md`, add it to the Administration nav in
   `mkdocs/mkdocs.yml` right after `admin/materialized-views.md`, and cross-link it from
   `mkdocs/docs/admin/materialized-views.md`.
9. Update `mkdocs/docs/query-guide/python-api.md:842-849` to include `micromegas-views` among the
   FlightSQL, `api_key_file`-honoring tools.
10. Add the `CHANGELOG.md` **Unreleased** entry.
11. `black` over the new and changed Python files.

## Files to Modify

| File | Change |
|---|---|
| `python/micromegas/micromegas/cli/state_sync.py` | New — shared diff/prompt/color helpers |
| `python/micromegas/micromegas/cli/views.py` | New — the tool |
| `python/micromegas/micromegas/cli/screens.py` | Refactor onto `state_sync` |
| `python/micromegas/pyproject.toml` | `micromegas-views` console script |
| `python/micromegas/tests/cli/test_views.py` | New — unit tests |
| `python/micromegas/tests/cli/test_version.py` | Add `micromegas-views` `--version` case |
| `mkdocs/docs/admin/views-as-code.md` | New — user documentation |
| `mkdocs/mkdocs.yml` | Administration nav entry |
| `mkdocs/docs/admin/materialized-views.md` | Cross-link to the new page |
| `mkdocs/docs/query-guide/python-api.md` | `api_key_file` honorer list |
| `CHANGELOG.md` | Unreleased entry |

## Trade-offs

**Text comparison vs. semantic comparison.** Comparing canonical statement text needs no local
understanding of the DDL. The alternative — compare parsed definitions — requires either a Python
reimplementation of `view_definition_from_options` (`view_ddl.rs:185`), a second and weaker copy of
a validator that already exists in Rust, or extending `list_view_definitions()` to expose the
normalized columns and *still* parsing the local file. Text comparison gives a faithful diff and one
source of truth for what a definition means, at the cost of reporting reformat-only edits as
updates.

**No Terraform provider.** A provider (Go, `terraform-plugin-framework`, reusing the in-tree
FlightSQL client at `grafana/pkg/flightsql/`) would get a real state file, and with it safe deletes
and drift detection without a `managed_by` column. Against that: a Go release/signing/registry
pipeline, a second auth stack to keep in step with the Python one, and a split from the
`micromegas-screens` pattern that this repo's other as-code surface already established. A Terraform
*module* is not an alternative on its own — modules compose a provider's resources, and the nearest
provider-free shape is a `local-exec` wrapper that needs this CLI anyway.

**No server-side dry run.** `plan` compares text; it cannot tell whether a new definition will pass
`validate_view_definition`. So a syntax or validation error surfaces at `apply`, not at `plan` —
unlike terraform, where `plan` does provider-side validation. Fixing that properly means a
server-side validate-without-writing path, which is a new server concept and out of scope for this
issue. `apply`'s per-statement error reporting is the mitigation.

## Decisions

- Python CLI sharing code with `micromegas-screens`, not a Terraform provider — user's call, after
  the comparison above.
- The shared module carries presentation and prompts only; the plan model is not generalized across
  the two tools.
- Interior whitespace is not normalized: a false `unchanged` on a differing string literal is worse
  than reformat churn in the plan output.
- `update_group` is regex-extracted from the local header for ordering and display only; the server
  stays the authority, and an unextractable value sorts last rather than failing locally, accepting
  that the apply ordering may be wrong for that one file rather than blocking the whole run.
- `apply` continues past a failed statement and exits non-zero, matching `screens.py`, rather than
  stopping at the first error.
- Deletes require `--prune`, and `--prune` refuses an empty desired-state directory.
- No live-DB or live-service test is added, per `CLAUDE.md` — no bug is being pinned here.

## Documentation

- **New:** `mkdocs/docs/admin/views-as-code.md` — the workflow, the file layout, the five
  subcommands, `--prune` and why deletes are opt-in, `log_stats` showing up as unmanaged on a fresh
  deployment, the fact that a drop also retires partitions, reformat-only diffs, that every
  subcommand (including read-only `list`/`show`/`plan`) requires an admin identity because
  `list_view_definitions` and the DDL path are both gated to `lakehouse_admin`, and a CI example
  using a profile with `api_key_file` — where that key must be an admin key.
- **Updated:** `mkdocs/docs/admin/materialized-views.md` — a pointer from "Introspection" to the new
  page.
- **Updated:** `mkdocs/docs/query-guide/python-api.md` — `micromegas-views` is FlightSQL-based, so
  it honors `api_key_file`; the paragraph at `:842-849` currently implies only `micromegas-query`
  and `connect_with_profile()` do.
- **Updated:** `mkdocs/mkdocs.yml` — nav entry.

## Testing Strategy

All automated coverage is unit tests in `python/micromegas/tests/cli/test_views.py`, driving the
`cmd_*` functions against a fake client whose `.query(sql)` returns a canned DataFrame. Nothing here
needs a live DB or service: every behavior being added is reachable by calling code with constructed
inputs.

**File parsing**
- Round trip: `pull` writes a file that `parse_local_definition` reads back to the same name, text,
  and `update_group`.
- `update_group` extracted from the header; **not** taken from an `update_group = 1` occurrence
  inside a `$$...$$` body, a single-quoted body, a `--` comment, or a `/* */` comment.
- Filename/DDL-name mismatch is reported and skipped, and suppresses pruning for both the filename
  stem and the declared name.
- An unparseable header is reported and skipped, and suppresses pruning repo-wide.
- Regression guard, run under `LC_ALL=C PYTHONUTF8=0` (matching
  `test_screen_files.py:880-975`): a non-ASCII `.sql` file round-trips through `pull`/parse, and
  colorized diff output prints without a `UnicodeEncodeError`.

**Canonicalization**
- `CREATE OR REPLACE ...` and `CREATE ...` of otherwise identical text canonicalize equal.
- Trailing `;`, trailing newline, and CRLF line endings canonicalize equal.
- Differing interior whitespace canonicalizes **unequal** (pins the §3 decision).
- `with_or_replace` is idempotent and inverse to step 4.

**Plan**
- Each row of the §4 table.
- Server-only definitions land in `unmanaged` without `--prune` and in `deletes` with it.
- `--prune` combined with an explicit `names` subset drops nothing.
- `--prune` against an empty directory errors instead of proposing drops.

**Apply**
- Creates/updates are issued in ascending `update_group`, drops in descending server
  `update_group` — asserted over the fake client's recorded statement sequence, which is what pins
  the dependency ordering.
- An update sends `OR REPLACE`; a create sends the file text verbatim.
- A statement that raises is counted, does not abort the rest, and produces a non-zero exit.
- `--auto-approve` skips the prompt; a declined prompt applies nothing.

**Output**
- The diff shown for an update contains no `OR REPLACE` line.
- `list` and `show` in both formats.
- `test_views_version_flag`: `--version` prints the version, Python version, and interpreter path
  (extending `tests/cli/test_version.py`, which already carries this test per tool).

**Regression guard for the extraction:** `tests/test_screen_files.py` and
`tests/cli/test_screens_auth.py` must pass with no edits after Phase 1.

## Manual Verification

Against `local_test_env` (`python3 local_test_env/ai_scripts/start_services.py`), which runs with
auth disabled. These steps exercise the one thing the fake client cannot: that the statements this
tool generates are accepted by the real parser, validator, and registry.

1. `mkdir /tmp/views && cd /tmp/views && micromegas-views list`
   → `log_stats` listed as `server-only`.
2. `micromegas-views pull log_stats`
   → `log_stats.sql` written; `micromegas-views plan` reports `No changes. 1 unchanged.` This is the
   round-trip check on real stored text — it confirms `canonical_ddl` absorbs exactly the difference
   between what the seeded row holds and what `pull` writes.
3. Write a `request_stats.sql` (the `CREATE MATERIALIZED VIEW` from
   `mkdocs/docs/admin/materialized-views.md`, `update_group = 4000`), then `micromegas-views plan`
   → `+ create: request_stats (update_group 4000)`.
4. `micromegas-views apply --auto-approve`
   → `created`; `micromegas-query "SELECT * FROM list_view_definitions()" --all` shows both rows.
5. Widen the `WHERE` clause in `request_stats.sql`; `micromegas-views plan`
   → `~ update` with a diff of just that line; `apply` → `replaced`.
6. Break the file (a column the source doesn't have) and `apply`
   → the server's validation error is reported and the exit code is non-zero. This is the
   `plan`-can't-validate gap from Trade-offs, checked by hand once.
7. `rm request_stats.sql && micromegas-views plan`
   → `? request_stats` under unmanaged, no drop proposed. Then `micromegas-views apply --prune`
   → `dropped`, and `list_partitions()` shows its partitions retired.

## Open Questions

None blocking. One deferred: whether a server-side validate-without-writing path is worth adding so
`plan` can catch a bad definition before `apply` (see Trade-offs). It is a separate issue if wanted.
