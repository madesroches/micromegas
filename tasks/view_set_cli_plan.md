# View Sets as Code CLI (Issue #1603) Plan

## Overview

A `micromegas-views` console script that treats a directory of `.sql` files as the desired state of
the deployment's DDL-defined materialized view sets, and the server as the current state, with a
terraform-shaped `plan` / `apply` / `pull` / `list` / `show` workflow. Nothing new is stored: the
desired state is the files (in git), the current state is `lakehouse_view_set_definitions` read
through the existing admin UDTF `list_view_set_definitions()`, and the write path is the existing
`CREATE [OR REPLACE] MATERIALIZED VIEW` / `DROP MATERIALIZED VIEW` DDL executed over FlightSQL. This
is a client-side diff/apply layer and no server change at all.

`micromegas-screens` already implements this exact workflow for screens
(`python/micromegas/micromegas/cli/screens.py`). So the second deliverable here is the extraction of
the parts that are not screen-specific — the colorized unified diff, the apply confirmation gate,
the `--color` flag wiring — into `cli/state_sync.py`, which both tools then use.

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

**The current-state read.** `list_view_set_definitions()`
(`rust/analytics/src/lakehouse/list_view_set_definitions_table_function.rs`) returns one row per stored
definition with `(view_set_name, definition_sql, update_group, updated_at, updated_by)`, read
straight from Postgres and ordered by name, so it also shows a definition that failed to load.
`definition_sql` is **the verbatim statement text as submitted** — `ViewDdl::Create::sql` is
`sql.to_string()` of the whole statement (`view_ddl.rs:34`, `:117`), stored unmodified by
`upsert_tx` (`view_definition_store.rs:127-160`). The normalized columns
(`extract_query`, `count_src_query`, `view_options`, ...) exist in the table but are not exposed by
the UDTF.

**`log_stats` is one of these rows.** The lakehouse migration seeds it into
`lakehouse_view_set_definitions` and `default_view_factory` no longer builds it
(`rust/analytics/src/lakehouse/view_factory.rs:305,346`, `log_stats_view.rs:65`). So a fresh deployment
already has exactly one stored definition that nobody checked into a views directory.

**The sibling tool.** `micromegas-screens` (`python/micromegas/micromegas/cli/screens.py`, 744
lines) is the precedent for everything structural here: `compute_plan` returning
`(creates, updates, deletes, unchanged, untracked)` (`:360-435`), `format_screen_diff` producing a
4-space-indented colorized `difflib.unified_diff` (`:438-463`), `format_plan` (`:466-492`),
`cmd_apply`'s `[y/N]` gate plus `--auto-approve` (`:508-594`), and a `client_args` parent parser
holding `--profile`/`--no-auth` defined exactly once to dodge the argparse shared-Namespace trap
(`:665-676`). Its unit tests (`python/micromegas/tests/test_screen_files.py`) drive the `cmd_*`
functions directly against a fake client.

**Auth.** `micromegas-screens` and its `WebClient` siblings resolve auth through
`web_auth.resolve_web_auth()`, which branches only on OIDC fields. A FlightSQL tool instead uses
`micromegas.connection.connect_with_profile(profile=..., client_entrypoint="cli-views")`
(`python/micromegas/micromegas/connection.py:14`), which honors `api_key_file`, then OIDC, then
falls back to unauthenticated — so no `--no-auth` flag is needed, and a static API key works in CI
with no browser. `mkdocs/docs/query-guide/python-api.md:843-850` currently states that
`micromegas-query` and `connect_with_profile()` are the only `api_key_file` honorers; the new tool
joins that list. Every subcommand needs an admin identity: `list_view_set_definitions` is
registered only inside the `lakehouse_admin` block
(`rust/analytics/src/lakehouse/query.rs:237-255`), so even read-only `list`/`show`/`plan` fail as
an unknown-function planner error for a non-admin caller, and `authorize_view_ddl`
(`view_ddl.rs:49-55`) gates the write path the same way.

**Today's workflow.** Hand-write the statement and pipe it through `micromegas-query --file`. There
is no preview, no desired-state file, and no way to tell whether the server matches the repo.

## Design

### 1. Command surface

```
micromegas-views plan  [names...] [--dir D] [--profile P] [--prune] [--color/--no-color]
micromegas-views apply [names...] [--dir D] [--profile P] [--prune] [--auto-approve] [--color/--no-color]
micromegas-views pull  [names...] [--dir D] [--profile P]
micromegas-views list         [--dir D] [--profile P] [--format table|json]
micromegas-views show  <name> [--profile P]
```

There is no `init` subcommand and no config file. `micromegas-screens` needs
`micromegas-screens.json` for a server URL and a `managed_by` ownership marker; this tool gets its
endpoint from the standard `--profile` resolution and has no ownership marker to record (see §5).
`--dir` defaults to `.`. Before dispatching, `main` checks — only for a subcommand whose parsed
`Namespace` carries a `dir` attribute, i.e. every subcommand but `show` — that `--dir` exists and
is a directory; if not, it reports the error and exits non-zero rather than letting a mistyped path
scan as an empty desired state.

One desired-state file per view set: `<view_set_name>.sql`, holding exactly one
`CREATE [OR REPLACE] MATERIALIZED VIEW` statement. Nothing else in the directory is read.

Bare `pull` (no names) refreshes only the files already present in `--dir` — the
`screens.py:cmd_pull` default. A local name absent from the server (the normal state between
authoring a new `.sql` file and running `apply`) is warned about and skipped, counted neither
`updated` nor `unchanged`, with no effect on the exit code — mirroring `screens.py:cmd_pull`
(`:329-333`). A named `pull` also adopts a server-only name into a new file, which
is the merged pull/import behavior §5 relies on. A named `pull` (and `show <name>`) for a name
present on neither side reports an error and exits non-zero, matching `screens.py:cmd_pull`
(`:308-317`). `pull` skips, with a warning, any target file that
fails to decode or whose header does not parse, rather than overwriting it — mirroring
`screens.py:cmd_pull`'s (`:337-352`) guard against clobbering a file that can't be safely read.
`pull` writes `canonical_ddl(definition_sql)` plus a trailing newline, not the server's raw stored
text: a definition last touched by `apply` is stored as `CREATE OR REPLACE` (§4), and writing that
raw would rewrite the file's `CREATE` to `CREATE OR REPLACE` on every pull, drifting the file away
from the canonical form the comparison in §3 is built on. Before writing, `pull` compares that
rendered text against the target file's current contents when the file already exists; if they
match, the file is left byte-identical (no write, no mtime change) and the name is counted
`unchanged` rather than `updated` — mirroring `screens.py:cmd_pull`'s (`:341-343`) no-op check.
`pull` finishes with a summary line, `Pull complete: N updated, M unchanged.`, matching
`screens.py:cmd_pull`'s report.

### 2. Local file model and parsing

A `.sql` file is read as text (`encoding="utf-8-sig"`, matching `screens.py:38,57`) and reduced to a
`LocalDefinition`:

```python
LocalDefinition = namedtuple("LocalDefinition", "name text path")
```

The only thing read out of the statement is its `name`, and it is read with a regex anchored at the
head of the text: leading `--` and `/* */` comments are stripped, then
`CREATE [OR REPLACE] MATERIALIZED VIEW <ident>` is matched. This needs no SQL lexer and adds no dependency: the view name always
precedes the `WITH (...)` options, so nothing but a comment can sit ahead of it and no string
literal — dollar-quoted or otherwise — can reach the matched region. The three queries are never
parsed, and neither are the options.

`name` must equal the filename stem, or the file is rejected: the diff, the plan output, and `pull`
all key on the filename, and a mismatch would silently apply a view under a name the repo doesn't
show.

`update_group` is deliberately **not** read locally — see the apply-ordering note in §4.

A file that fails to decode, whose header does not parse at all, or whose name disagrees with its
filename, is reported and skipped, and every skipped file contributes to a single `protected_names`
set that `--prune` refuses to drop: its filename stem always, plus the name the header declares when
that is what disagreed.

A skipped file also makes `plan` and `apply` exit non-zero — `apply` after applying the files that
did parse: the repo means to manage that view set and silently isn't, and an unattended
`apply --auto-approve`, or a CI job gating only on `plan`'s exit code, would otherwise treat the
skip as success even though the file's view set went unmanaged. Pending creates/updates/drops do
not by themselves move `plan`'s exit code — see §4's exit-code contract; a CI drift check reads
`plan`'s output for those.

`list_local_definitions(dir)` therefore returns `(definitions, protected_names)`: `definitions` maps
name to `LocalDefinition`, and `protected_names` is the prune suppression set. This is the tuple
`compute_plan` (§4) takes as its `local_scan` argument.

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
the keyword run also absorbs interior whitespace inside it, and it rewrites the same head-anchored
region the name scan reads (§2), so it needs no lexer either.

Interior whitespace is **not** normalized beyond that. A reformat-only edit therefore shows up as an update, and applying it is harmless:
the schema hash is derived from the inferred Arrow schema, not the query text
(`mkdocs/docs/admin/materialized-views.md`, "What a redefinition means"), so existing partitions
stay valid and the only effect is a `CREATE OR REPLACE` round trip and a registry reload.

`plan`'s per-view diff is a `difflib.unified_diff` over the canonical text of both sides
(`fromfile="server"`, `tofile="local"`), so the `OR REPLACE` bookkeeping never appears in output.

### 4. The plan model

```python
def read_server_state(client):
    """-> DataFrame(view_set_name, definition_sql, update_group, updated_at, updated_by)"""

def compute_plan(server_state, local_scan, names=None):
    """-> (creates, updates, unchanged, server_only)"""
```

`read_server_state` issues the current-state `SELECT` below; it does not catch
`pyarrow.flight.FlightError` / `pyarrow.lib.ArrowException` itself — that catch lives in `main`,
around dispatch (see "Current-state read"). `compute_plan` takes the DataFrame `read_server_state`
returns as an argument, the same way it already takes `local_scan`, rather than fetching it itself.
`cmd_plan`, `cmd_apply`, and `cmd_list` each call `read_server_state` exactly once and
pass the result to `compute_plan`. `cmd_pull` also calls `read_server_state` exactly once, but
decides per file on the byte comparison in §1 against the local scan, not on `compute_plan`'s
`updates`/`unchanged` classification. `cmd_show` has no `local_scan` at all (§1 drops `--dir` from
`show`), so it cannot go through `compute_plan`; it calls `read_server_state` directly. `cmd_list` reads
`update_group`/`updated_at`/`updated_by` off the same DataFrame it already fetched, since
`compute_plan`'s return tuple does not carry those columns.

`updates` elements are `(name, local_canonical, server_canonical)` triples — mirroring
`screens.py:397`'s `(name, normalized_local, normalized_server)` — so the per-view diff in §3 and
`list`'s output in §7 have both canonical texts without a second read.

`local_scan` is the `(definitions, protected_names)` tuple `list_local_definitions` returns (§2),
passed in by the caller so a single command invocation scans the directory once and prints each
skipped-file warning once — the reason `screens.py:compute_plan` takes a `local_scan` argument
(`:355-368`). `protected_names` is consulted by `cmd_plan`/`cmd_apply` when `--prune` is set. An
empty `definitions` is the "zero readable `.sql` files" case that `--prune` refuses to run (§5),
checked before drops are rendered.

The `client` argument to `read_server_state` is anything with `.query(sql)` returning a DataFrame —
the real FlightSQL client in production, a canned-DataFrame fake in tests. It reaches `read_server_state`
through a module-level `make_client(args)` factory —

```python
def make_client(args):
    return connect_with_profile(profile=args.profile, client_entrypoint="cli-views")
```

— which each `cmd_*` calls itself, exactly the monkeypatchable seam `screens.py:187`'s
`make_client(config, args)` already is for `micromegas-screens`
(monkeypatched in `tests/test_screen_files.py:673,704,745,771,789,808`). There is no connect-once
call in `main`: `ProfileError` raised inside a `cmd_*`'s call to `make_client` propagates up through
`args.func(args)` and is caught by the same dispatch-level catch described under "Current-state
read" below. Current state is one call:

```sql
SELECT view_set_name, definition_sql, update_group, updated_at, updated_by
  FROM list_view_set_definitions()
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
| absent | present | `server-only` |

`names` narrows only the create/update/unchanged classification, which iterates the named subset;
`server_only` is always computed against the full `definitions` map from `list_local_definitions`,
so a locally-managed view outside the named subset is never misreported as server-only. A name
passed to `plan` or `apply` that is present on neither side is reported as an error and the command
exits non-zero, matching `pull <name>` and `show <name>` (§1).

**`plan`'s exit-code contract.** Non-zero only for a skipped local file (§2), an unknown name
(above), or a dispatch-level error caught by `main` (below) — never for pending changes: a plan
with creates, updates, or drops still queued exits 0, matching `screens.py:cmd_plan`
(`:495-505`), which always exits 0 regardless of what the plan contains. A CI job that wants to
fail on drift reads `plan`'s output — the `Plan: N to create, N to update, N to drop, N
unchanged.` summary line, or `list --format json`'s per-name `status` column (§7) — rather than
the exit status.
A later `--detailed-exitcode` flag could offer terraform's convention (0 none / 1 error / 2
pending) for jobs that would rather gate on status; not in scope here.

Rendering `server_only` as drops is each command's decision, not `compute_plan`'s: both `cmd_plan`
and `cmd_apply` compute `drops = [n for n in server_only if n not in protected_names] if (prune and
not names) else []` — the same fence as
`screens.py:compute_plan`'s `if not names` guard around its delete loop (`screens.py:396-425`), now
applied at the call site instead of inside the plan function: a named-subset run has no way to know
whether a view set outside the subset is still managed elsewhere in the directory, so treating "not
mentioned" as "delete" would be wrong.

**Apply order.** All creates and updates run first, then all drops — matching `screens.py:cmd_apply`
(`:549-586`) — so that an update which removes a view's dependency on a to-be-pruned view lands
before the drop, rather than having `check_dependents_survive` refuse it. Within each phase,
statements run in name order.

**Dependency order is not attempted.** Three statement-level refusals are order-sensitive: creating
a view whose query reads a not-yet-created view (validation builds its factory from the rows stored
at that moment, `flight_sql_service_impl.rs:1038-1067`); dropping a view another definition still
reads; and a `CREATE OR REPLACE` that narrows a column a dependent uses (the last two both
`check_dependents_survive`, `:1118-1128`). Each of them leaves the server unchanged and returns an
error naming the view and the reason, so the tool reports it like any other failed statement and
exits non-zero; running `apply` again — now that the statements it depended on have landed —
resolves one level of a chain per run.

**Statement sent.** For an update, the file text with `OR REPLACE` injected into the header when
absent (the inverse of `canonical_ddl`'s step 4). For a create, `canonical_ddl(text)` — guaranteed
plain `CREATE` regardless of how the file is written — so that if someone created that view between
`plan` and `apply`, the server's `already_exists` error surfaces the race instead of silently
clobbering it. For a drop, `DROP
MATERIALIZED VIEW <name>` without `IF EXISTS` — the plan just established it exists, and a
`not_found` status is worth an error rather than a shrug.

`apply` reports each statement's returned `(view_set_name, status)` row, continues past a failure
(counting it, as `screens.py:cmd_apply` does), and exits non-zero if any statement failed or the
scan skipped any local file (§2). The exceptions caught per statement are
`pyarrow.flight.FlightError` and `pyarrow.lib.ArrowException` — the latter because the gRPC statuses this design leans on, `already_exists` and `not_found`
(`flight_sql_service_impl.rs:1021,1082`), surface from pyarrow as `ArrowException` and
`ArrowKeyError` respectively, neither a `FlightError` nor an `ArrowInvalid`; `ArrowException` is
their common base (along with `ArrowInvalid`, used for `INVALID_ARGUMENT`), so it is caught rather
than the narrower subclasses. This is what
`FlightSQLClient.query` actually raises (`flightsql/client.py:355-419`,
`tests/test_ddl_materialized_view.py:263`), not the `RuntimeError` `screens.py`'s `WebClient`
raises. Each DDL statement is its own server-side transaction,
so a partial apply is a real outcome; the workflow is idempotent, so the remedy is re-running
`apply`.

**Current-state read.** `read_server_state`'s `SELECT ... FROM list_view_set_definitions()` call is
the first FlightSQL round trip every subcommand makes, read-only `list`/`show`/`plan` included, and
the anticipated failure point for a non-admin identity (Current State, "Auth"). `read_server_state`
itself does not catch anything or exit; `main` wraps `args.func(args)` in a
`pyarrow.flight.FlightError` / `pyarrow.lib.ArrowException` / `ProfileError` / `OSError` catch around
dispatch — the same pattern `screens.py:main` uses (`:736-740`), extended with `OSError` for a
`--dir` that stops existing or a target file that can't be written — reporting the error and exiting
non-zero with a pointer to the admin-identity requirement, rather than letting the planner's
unknown-function error, a `make_client` failure, or a filesystem error surface as a raw traceback.

### 5. Deletes are opt-in

`--prune` still gates dropping it, justified by blast radius (a `DROP` also
retires partitions, below) plus the migration-seeded `log_stats` row. Server-only definitions are
therefore reported as
`server-only` and **never dropped** unless `--prune` is passed. Two guards on top:

- `--prune` refuses to run when the directory contains zero readable `.sql` files. A wrong `--dir`
  or a checkout at the wrong commit is the plausible way to ask a reconciler to drop production
  views, and an empty desired state is the signature of it.
- `--prune` still routes through the same confirmation gate, and `--auto-approve` still bypasses the
  gate (CI needs it) — the explicitness lives in `--prune` itself.

A fresh deployment lists `log_stats` as `server-only`, since the migration seeds it. `pull log_stats`
adopts it into the directory; leaving it unadopted means `--prune` will propose dropping it.

A `DROP` also retires the view's partitions (`flight_sql_service_impl.rs:1092-1108`), so pruning is
destructive beyond the definition row. Worth one line in the docs — phrased without promising how
many instances' partitions go with it, since `partition_insert_range` keys on
`view_instance_id = 'global'` today (`view_definition_store.rs:192`) and a per-instance definition
would have to sweep every instance.

### 6. Shared module: `cli/state_sync.py`

Extracted from `screens.py`, used by both tools:

- `unified_diff(before_lines, after_lines, from_label, to_label, use_color)` — `difflib` plus the
  `---`/`+++`/`@@`/`-`/`+` coloring and 4-space indent currently the back half of
  `format_screen_diff` (`screens.py:445-463`); returns `""` when there is no difference.
- `confirm_apply(auto_approve)` — the `[y/N]` prompt and the `Apply cancelled.` message
  (`screens.py:531-535`), returning `True` when approved and `True` immediately when
  `auto_approve`. Each `cmd_apply` writes `if not confirm_apply(args.auto_approve): sys.exit(1)`, which is the
  exact behavior `screens.py` has today — same message, same exit code — so the extraction stays
  behavior-preserving.
- `add_color_arg(parser)` — the `--color` `BooleanOptionalAction` default-`True` flag.
- `use_color(args)` — `sys.stdout.isatty() and args.color`.

`screens.py` keeps `format_screen_diff` as a thin wrapper (JSON-serialize both sides, call
`unified_diff`), so `tests/test_screen_files.py`'s existing imports and assertions keep passing
unchanged. Nothing screen-specific moves: `managed_by`, the untracked/ownership model, the
`compute_plan` tuple shape, and `WebClient` construction all stay in `screens.py`.

### 7. Output

```
$ micromegas-views plan
micromegas-views will perform the following actions:

  + create: request_stats
  ~ update: log_stats
    --- server
    +++ local
    @@ -3,7 +3,7 @@
    -    WHERE insert_time >= '{begin}' AND insert_time < '{end}'
    +    WHERE insert_time >= '{begin}' AND insert_time < '{end}' AND level <= 4

Plan: 1 to create, 1 to update, 0 to drop, 2 unchanged.

Server-only view sets on server (use 'pull' to adopt, '--prune' to drop):
  ? error_rollup
```

`list` calls `read_server_state` once, passes the result to `compute_plan`, and joins the same
DataFrame's rows for `update_group`/`updated_at`/`updated_by`: name / status (`create`, `update`,
`unchanged`, `server-only`) / `update_group` / `updated_at` / `updated_by`, with `--format json`; a
`create` row has no server row yet, so those last three columns are empty for it. `show <name>` calls
`read_server_state` directly (it has no `local_scan` to give `compute_plan`) and prints the
server's stored `definition_sql` verbatim.
`pull` differs from `show`: it writes `canonical_ddl(definition_sql)`, not the verbatim stored
text (§1).

## Implementation Steps

**Phase 1 — shared module**

1. Create `python/micromegas/micromegas/cli/state_sync.py` with the four helpers in §6.
2. Refactor `python/micromegas/micromegas/cli/screens.py` to use them: `format_screen_diff` becomes
   a wrapper over `unified_diff`; `cmd_apply`'s inline prompt becomes
   `if not confirm_apply(args.auto_approve): sys.exit(1)`; the two
   `--color` definitions and the two `sys.stdout.isatty() and args.color` expressions become
   `add_color_arg` / `use_color`.
3. Run `python/micromegas/tests/test_screen_files.py` and `tests/cli/test_screens_auth.py` — they
   must pass untouched. That is the check that the extraction changed no behavior.

**Phase 2 — the new tool**

4. Create `python/micromegas/micromegas/cli/views.py`: `parse_local_definition`,
   `list_local_definitions(dir)`, `canonical_ddl`, `with_or_replace`, `make_client`,
   `read_server_state`, `compute_plan`, `format_plan`, `cmd_plan`, `cmd_apply`, `cmd_pull`,
   `cmd_list`, `cmd_show`, `main`. Each `cmd_*` calls `make_client(args)` to get its client (§4).
   `cmd_apply` catches
   `pyarrow.flight.FlightError` and `pyarrow.lib.ArrowException` per statement; `main` wraps
   `args.func(args)` in the same catch around dispatch, plus `ProfileError` (raised by `make_client`
   inside a `cmd_*`) and `OSError` (a `--dir` that fails the existence/is-a-directory check, or an
   unwritable target file), reporting the error and exiting non-zero with a pointer to the
   admin-identity requirement when it originates from `read_server_state` (§4).
   `cmd_pull` skips the write and counts the file `unchanged` when `canonical_ddl(definition_sql) +
   "\n"` (the text `pull` writes) already matches the file's current contents; otherwise it writes with `encoding="utf-8"`,
   matching `screens.py:89,256`. `main` calls `sys.stdout.reconfigure(encoding="utf-8",
   errors="backslashreplace")` before dispatching, matching `screens.py:651`, so colorized diff
   output survives a non-UTF-8 locale.
5. Wire the argparse surface of §1 with two parent parsers — `client_args` carrying `--profile`
   (every subcommand) and `dir_args` carrying `--dir` (every subcommand but `show`, which has no
   local side). Each flag is defined exactly once, which is the point — see `screens.py:665-676`
   for the shared-Namespace trap this avoids. Call `add_version_argument(parser)` so `--version`
   matches the other seven console scripts.
6. Add `micromegas-views = "micromegas.cli.views:main"` to `[tool.poetry.scripts]` in
   `python/micromegas/pyproject.toml`.

**Phase 3 — tests and docs**

7. Add `python/micromegas/tests/cli/test_views.py` (§ Testing Strategy).
8. Write `mkdocs/docs/admin/views-as-code.md`, add it to the Administration nav in
   `mkdocs/mkdocs.yml` right after `admin/materialized-views.md`, and cross-link it from
   `mkdocs/docs/admin/materialized-views.md`.
9. Update `mkdocs/docs/query-guide/python-api.md:843-850` to include `micromegas-views` among the
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
a validator that already exists in Rust, or extending `list_view_set_definitions()` to expose the
normalized columns and *still* parsing the local file. Text comparison gives a faithful diff and one
source of truth for what a definition means, at the cost of reporting reformat-only edits as
updates. It also costs nothing as the DDL grows: a new `WITH (...)` option — an instance key, a
retention setting — diffs and round-trips correctly through a tool that never learned what it means,
where a semantic comparison would need a new field and a new local parser branch per option.

**No Terraform provider.** A provider (Go, `terraform-plugin-framework`, reusing the in-tree
FlightSQL client at `grafana/pkg/flightsql/`) would get a real state file, and with it safe deletes
and drift detection driven by that state rather than by diffing the whole table. Against that: a Go release/signing/registry
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
  the two tools. All four helpers move, including the two one-liners: they exist so the two tools
  cannot disagree on the `--color` flag and the isatty rule, which `screens.py` already duplicates
  four times between them. `confirm_apply` returns a bool instead of exiting, so the shared code
  never terminates the process.
- Interior whitespace is not normalized: a false `unchanged` on a differing string literal is worse
  than reformat churn in the plan output.
- Nothing but the view name is read out of the local file, with a head-anchored regex — no SQL
  lexer and no new dependency. `update_group` is not read locally and `apply` does not order by
  dependency; an order-sensitive statement fails with a server-side error naming the view, and
  re-running `apply` is the remedy (§4).
- `apply` continues past a failed statement and exits non-zero, matching `screens.py`, rather than
  stopping at the first error.
- A skipped local file makes `plan` and `apply` exit non-zero, unlike `screens.py`, which warns and
  exits 0: this tool is meant to run unattended in CI, where a warning on stderr is not read.
- `plan` exits 0 when creates/updates/drops are pending, matching `screens.py:cmd_plan`; a CI
  drift check must read `plan`'s output, not its exit status, to detect pending changes.
- A skipped local file protects only the name(s) it could have been, not the whole directory:
  unlike `screens.py`, the filename stem is the key here, so there is no unknowable-identity case
  to justify a repo-wide suppression (§2).
- Deletes require `--prune`, and `--prune` refuses an empty desired-state directory. Its safety also
  depends on `list_view_set_definitions()` exposing the DDL tier alone; if the anonymous tier ever shares
  the table, `compute_plan` must filter on a tier discriminator before classifying `server-only`.
- View sets get no `managed_by` column: creation has no UI path today, so there is no exploratory
  tier to distinguish from; if an admin-console create path lands, opt-in `--prune` plus the
  `server-only` report — not a `managed_by` marker — is what keeps a UI-born definition safe.
- The comparison depends on the server storing `definition_sql` verbatim and never rewriting it
  (§3). Stated as an invariant rather than assumed, because server-side column injection is being
  designed for the adjacent tier.
- `show` reads the server only. A local-file mode would be `cat <name>.sql`, and dropping it makes
  "every subcommand connects and needs an admin identity" a flat rule with no carve-out in `main`.
- Instance fan-out is out of scope, and the file model is keyed on `view_set_name` because the
  definition table is. A `scaffold --from-query` bridge to the anonymous tier is a follow-up,
  blocked on a server-side probe-derivation path.
- No live-DB or live-service test is added, per `CLAUDE.md` — no bug is being pinned here.

## Documentation

- **New:** `mkdocs/docs/admin/views-as-code.md` — the workflow, the file layout, the five
  subcommands, `--prune` and why deletes are opt-in, `log_stats` showing up as server-only on a
  fresh deployment, that adopting it with `pull log_stats` means re-`pull`ing it again after a
  micromegas upgrade — a release can re-seed its shipped definition, and otherwise `plan` reports
  that as drift for an unattended `apply` to silently revert — the fact that a drop also retires
  the view's partitions, the fact that an
  `apply`d content-changing update does **not** retire partitions materialized under the previous
  definition — pointing at `materialized-views.md`'s "What a redefinition means" — reformat-only
  diffs, that
  every subcommand, read-only `list`/`show`/`plan` included, requires an admin identity because
  `list_view_set_definitions` and the DDL path are both gated to `lakehouse_admin`, and a CI
  example
  using a profile with `api_key_file` — where that key must be an admin key. It should also say what
  this tool does *not* manage: built-in view sets, which have no stored definition, and on-demand
  cached queries, which have no name — so a user who misses `log_entries` from `list` knows why.
- **Updated:** `mkdocs/docs/admin/materialized-views.md` — a pointer from "Introspection" to the new
  page.
- **Updated:** `mkdocs/docs/query-guide/python-api.md` — `micromegas-views` is FlightSQL-based, so
  it honors `api_key_file`; the paragraph at `:843-850` currently implies only `micromegas-query`
  and `connect_with_profile()` do. Also add `AlreadyExists` → `pyarrow.lib.ArrowException` and
  `NotFound` → `pyarrow.lib.ArrowKeyError` rows to the "Exception types" table at `:1226-1245`,
  which `apply`'s per-statement catch (§4) relies on but which the table doesn't currently list.
- **Updated:** `mkdocs/mkdocs.yml` — nav entry.

## Testing Strategy

All automated coverage is unit tests in `python/micromegas/tests/cli/test_views.py`, driving the
`cmd_*` functions against a fake client whose `.query(sql)` returns a canned DataFrame, substituted
by monkeypatching module-level `make_client` — the same seam `test_screen_files.py` uses for
`screens.py:make_client`. Nothing here needs a live DB or service: every behavior being added is
reachable by calling code with constructed inputs.

**File parsing**
- Round trip: `pull` writes a file that `parse_local_definition` reads back to the same name and
  text.
- A leading `--` comment, and a leading `/* */` comment, before `CREATE` do not defeat the name
  scan.
- An `update_group = 1` inside a `$$...$$` body does not disturb the name scan (the scan stops at
  the view name, well before any body).
- A server row whose `definition_sql` begins `CREATE OR REPLACE` pulls down as plain `CREATE`
  (pins that `pull` writes `canonical_ddl(definition_sql)`, not the verbatim stored text).
- A `pull` over a file that already holds `canonical_ddl(definition_sql)` for that name leaves the
  file byte-identical and counts it `unchanged` in the summary line, rather than rewriting it.
- Filename/DDL-name mismatch is reported and skipped, and protects both the filename stem and the
  declared name from `--prune`.
- A `.sql` file that fails to decode is reported and skipped, and protects its filename stem from
  `--prune`.
- An `apply` whose scan skipped a file still applies every file that parsed, reports the skip, and
  exits non-zero; `plan` reports the skip and exits non-zero too.
- An unparseable header is reported and skipped, and protects its filename stem from `--prune` —
  and *only* its stem: an unrelated server-only view in the same directory is still proposed for
  drop (pins that there is no repo-wide suppression).
- A named `pull` against an existing `.sql` file that fails to decode or whose header does not
  parse warns and leaves the file byte-identical (mirrors `test_screen_files.py`'s coverage of
  `screens.py`'s no-clobber guard).
- Regression guard, run under `LC_ALL=C PYTHONUTF8=0` (matching
  `test_screen_files.py:876-992`): a non-ASCII `.sql` file round-trips through `pull`/parse, and
  colorized diff output prints without a `UnicodeEncodeError`.

**Canonicalization**
- `CREATE OR REPLACE ...` and `CREATE ...` of otherwise identical text canonicalize equal.
- Trailing `;`, trailing newline, and CRLF line endings canonicalize equal.
- Differing interior whitespace canonicalizes **unequal** (pins the §3 decision).
- `with_or_replace` is idempotent and inverse to step 4.

**Plan**
- Each row of the §4 table.
- A server-only definition lands in `compute_plan`'s `server_only` return value regardless of
  `--prune`.
- `drops` is empty unless the call site sets `prune and not names`, and equals `server_only` when it
  does — covers both the no-`--prune` and the `--prune`-plus-named-subset cases.
- `--prune` against an empty directory errors instead of proposing drops.
- A fake client whose `.query` raises `ArrowException` on the current-state read lets the exception
  propagate out of `read_server_state` through each read-only subcommand's `cmd_*` function
  (`plan`, `list`, `show`, `pull`); `main`'s dispatch-level catch is what reports the error with the
  admin-identity pointer and exits non-zero.

**Apply**
- Every create/update is issued before any drop — asserted over the fake client's recorded
  statement sequence, which is what pins the phase ordering.
- An update sends `OR REPLACE`; a create sends `canonical_ddl(text)`, plain `CREATE` even when the
  file itself says `CREATE OR REPLACE`.
- A statement that raises `pyarrow.lib.ArrowKeyError` (the `not_found` case) and one that raises
  bare `pyarrow.lib.ArrowException` (the `already_exists` case) are each counted, do not abort the
  rest, and produce a non-zero exit — pinning the widened catch, not just `ArrowInvalid`.
- `--auto-approve` skips the prompt; a declined prompt applies nothing and exits 1.
- `confirm_apply` returns `False` on a declined prompt rather than raising `SystemExit` — asserted
  directly, without `pytest.raises`.

**Output**
- The diff shown for an update contains no `OR REPLACE` line.
- `list` in both formats; `show` for a stored name, and its non-zero exit for an unknown one.
- `test_views_version_flag`: `--version` prints the version, Python version, and interpreter path
  (extending `tests/cli/test_version.py`).

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
   → `+ create: request_stats`.
4. `micromegas-views apply --auto-approve`
   → `created`; `micromegas-query "SELECT * FROM list_view_set_definitions()" --all` shows both rows.
5. Widen the `WHERE` clause in `request_stats.sql`; `micromegas-views plan`
   → `~ update` with a diff of just that line; `apply` → `replaced`.
6. Break the file (a column the source doesn't have) and `apply`
   → the server's validation error is reported and the exit code is non-zero. This is the
   `plan`-can't-validate gap from Trade-offs, checked by hand once.
7. `rm request_stats.sql && micromegas-views plan`
   → `? request_stats` under server-only, no drop proposed. Then `micromegas-views apply --prune`
   → `dropped`, and `list_partitions()` shows its partitions retired.

## Open Questions

None.
