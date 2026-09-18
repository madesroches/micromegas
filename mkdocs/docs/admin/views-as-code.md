# View Sets as Code

`micromegas-views` treats a directory of `.sql` files as the desired state of the deployment's
[DDL-defined materialized view sets](materialized-views.md), and the server as the current state,
with a terraform-shaped `plan` / `apply` / `pull` / `list` / `show` workflow. Nothing new is
stored: the desired state is the files (checked into git, typically), the current state is
[`list_view_set_definitions()`](functions-reference.md#list_view_set_definitions), and the write
path is the same `CREATE [OR REPLACE] MATERIALIZED VIEW` / `DROP MATERIALIZED VIEW` DDL described
in [Materialized Views](materialized-views.md), executed over FlightSQL. This is a client-side
diff/apply layer — there is no new server concept here.

## Requirements

Every subcommand needs an admin identity, including the read-only `list`/`show`/`plan`:
`list_view_set_definitions()` and the `CREATE`/`DROP MATERIALIZED VIEW` DDL path are both gated to
`lakehouse_admin` (see [Authorization](authorization.md#admin-gated-lakehouse-functions)). A
non-admin doesn't get `list_view_set_definitions()` registered at all, so the very first round
trip fails with a "function not found" planner error, not a permission-denied status; a
permission-denied error is specific to the `CREATE`/`DROP MATERIALIZED VIEW` DDL path used by
`apply`.

`micromegas-views` is FlightSQL-based (like `micromegas-query`), not the analytics-web-based
`WebClient` `micromegas-screens` and its siblings use — so it honors `api_key_file` in
`~/.micromegas/config.json`, and a static admin API key works in CI with no browser. See
[Python API: Command-Line Interface](../query-guide/python-api.md#command-line-interface) for the
config file shape and the `--profile` flag.

## File layout

One file per view set: `<view_set_name>.sql`, holding exactly one `CREATE [OR REPLACE]
MATERIALIZED VIEW` statement. The filename stem must equal the name in the statement's header, or
the file is rejected. Nothing else in the directory is read — no config file, no `init` step.

## Commands

```
micromegas-views plan  [names...] [--dir D] [--profile P] [--prune] [--color/--no-color]
micromegas-views apply [names...] [--dir D] [--profile P] [--prune] [--auto-approve] [--color/--no-color]
micromegas-views pull  [names...] [--dir D] [--profile P]
micromegas-views list         [--dir D] [--profile P] [--format table|json]
micromegas-views show  <name> [--profile P]
```

`--dir` defaults to `.`. `plan` and `apply` restrict themselves to `names` when given; `pull` with
no names refreshes every locally-tracked (parseable) file, and a named `pull` also adopts a
server-only definition into a new file — see "Adopting an existing definition" below.

### `plan`

Prints the actions `apply` would take: `+ create`, `~ update` (with a unified diff of the
canonical statement text), and, only with `--prune`, `- drop`. Definitions that exist only on the
server are listed separately, since they are never dropped without `--prune`. `plan` always exits
0 for pending changes — read its output (or `list --format json`'s `status` column) to gate CI on
drift, not its exit code.

### `apply`

Runs every pending create/update, then every pending drop, prompting for confirmation first unless
`--auto-approve` is given. Each statement's outcome (`created`/`replaced`/`dropped`) is printed as
it runs; a failed statement is reported and counted, without aborting the rest — the workflow is
idempotent, so re-running `apply` is the remedy once the underlying issue (a validation error, an
order-sensitive dependency) is fixed.

### `pull`

Writes the server's current definition into `<name>.sql` for each name, refreshing what's already
tracked when run with no arguments. The text written is the canonical form (see "Reformatting and
`OR REPLACE`" below), not the server's raw stored text.

### `list` / `show`

`list` shows every name on either side with its status (`create`/`update`/`unchanged`/
`server-only`) and, for names the server already knows about, `update_group`/`updated_at`/
`updated_by`. `show <name>` prints the server's stored definition verbatim.

## Deletes are opt-in

A server-only definition (present on the server, absent from the directory) is never dropped
unless `--prune` is passed, both for `plan` (to preview it) and `apply` (to actually run it).
`--prune` refuses to run at all against a directory with zero readable `.sql` files — the
plausible way to end up there is a wrong `--dir` or a checkout at the wrong commit, and an empty
desired state is exactly what that mistake looks like; treating it as "drop everything" would be
the worst possible failure mode.

Dropping a view set also retires its materialized partitions — pruning is destructive beyond just
the definition row. It does not retire partitions for a content-changing update, though: see
[Materialized Views: What a redefinition means](materialized-views.md#what-a-redefinition-means).

## `log_stats` shows up as server-only

A fresh deployment already has one stored definition nobody checked into a views directory:
`log_stats`, seeded by the lakehouse migration. `micromegas-views list`/`plan` reports it as
`server-only` until it's adopted with `micromegas-views pull log_stats`. Leaving it unadopted means
an `apply --prune` will propose dropping it.

Adopting it is a one-time step, not a permanent guarantee: a later micromegas upgrade can re-seed
`log_stats` with an updated shipped definition, which then shows up as drift (an `~ update`) the
next time `plan` runs against the file pulled before the upgrade. Re-`pull` it after upgrading,
or an unattended `apply --auto-approve` will silently revert the shipped change back to what your
file says.

## Reformatting and `OR REPLACE`

The comparison is a text comparison over a canonical form of both sides: line endings normalized,
a single trailing `;` dropped, and the leading `CREATE [OR REPLACE] MATERIALIZED VIEW` keyword run
collapsed to plain `CREATE MATERIALIZED VIEW` — since `apply` sends `OR REPLACE` for an update
regardless of how the file is written, and a hand-written definition may say either. Interior
whitespace is **not** normalized beyond that, so a reformat-only edit shows up as an `~ update`;
applying it is harmless, since the schema hash used to invalidate partitions comes from the
inferred Arrow schema, not the query text (see [Materialized Views: What a redefinition
means](materialized-views.md#what-a-redefinition-means)).

## What this tool does not manage

- **Built-in view sets** (the ones registered in code, not via DDL) have no stored definition and
  never appear in `list_view_set_definitions()`, so they never appear in `micromegas-views list`
  either — e.g. `log_entries` is not something you'd expect to `pull`.
- **On-demand cached queries** have no name and are not part of this inventory at all.

## Validation happens at `apply`, not at `plan`

`plan` only compares text — it has no way to know whether a new or changed definition will pass
the server's validation (schema, `update_group` ordering, no volatile functions, and the rest of
[What is checked at CREATE time](materialized-views.md#what-is-checked-at-create-time)). A syntax
or validation error only surfaces when `apply` sends the statement, reported per-statement rather
than failing the whole run.

`apply` also cannot resolve dependency ordering on your behalf: creating a view that reads a
not-yet-created view, dropping a view another definition still reads, or narrowing a column a
dependent uses are all refused server-side, naming the view and the reason. Re-running `apply`
after the blocking statement lands resolves one level of a dependency chain per run.

## CI example

```bash
micromegas-views plan --profile ci --prune
```

using a profile with `api_key_file` set to an **admin** key — a non-admin key fails on the first
round trip, since even `plan` reads `list_view_set_definitions()`.
