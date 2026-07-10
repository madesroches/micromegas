# Retire `telemetry-admin` Subcommands & Rename to a Daemon Service Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1266

## Overview
`telemetry-admin` is, in practice, the maintenance daemon: it is deployed as
`telemetry-admin crond` alongside the ingestion and FlightSQL services and runs the scheduled
materialization + retention loop. But it is still packaged and named like an interactive admin
CLI, and it still carries four one-off subcommands whose capabilities now live either in SQL
table functions (`materialize_partitions()`, `retire_partitions()`) or in the daemon's own
automatic hourly task (retention, temp-file cleanup).

This change does two things:

1. **Remove the four deprecated subcommands** (`materialize-partitions`, `retire-partitions`,
   `delete-old-data`, `delete-expired-temp`), leaving the cron daemon as the binary's *only*
   behavior. Because that leaves a single mode, the `crond` subcommand layer is dropped as well —
   the binary just runs the daemon when invoked.
2. **Rename the crate/binary** from `telemetry-admin` to a name that reflects the daemon role
   (`telemetry-maintenance-srv`), matching the repo's existing `*-srv` service-binary convention.

Because the manual `delete-old-data` horizon disappears, this change also makes the daemon's
currently hardcoded 90-day retention horizon configurable via an env var / flag (default 90),
so an operator who needs a tighter or looser horizon still has a knob — now a *policy* setting on
the running service rather than a one-off manual purge.

## Current State

### The binary (`rust/telemetry-admin-cli/`)
- `Cargo.toml`: package `name = "telemetry-admin"`, `[[bin]] name = "telemetry-admin"`,
  `path = "src/telemetry_admin.rs"`. Directory is `rust/telemetry-admin-cli/`.
- `src/telemetry_admin.rs` (152 lines) defines a clap `Cli` with a `Commands` enum:
  - `DeleteOldData { min_days_old }` → `delete_old_data(&data_lake, min_days_old)` (handler
    lines 84-86)
  - `DeleteExpiredTemp` → `delete_expired_temporary_files(data_lake)` (87-89)
  - `MaterializePartitions { … }` → `materialize_partition_range(…)` (90-116)
  - `RetirePartitions { … }` → `retire_partitions(…)` (117-134)
  - `CronDaemon { shutdown_grace_period_seconds }` → `servers::maintenance::daemon(…)` (136-148)
    — **the only mode to keep**.
  - `#[clap(arg_required_else_help(true))]` at line 31 forces a subcommand today.
- The imports on lines 10, 11, 14, 16 (`delete_old_data`, `materialize_partition_range`,
  `delete_expired_temporary_files`, `retire_partitions`) plus `Context`, `PartitionCache`,
  `TimeRange`, `TimeDelta`, `DateTime`, `Utc` exist **only** to serve the four subcommands and
  become dead once they are removed.
- `src/queries.sql`: an unreferenced scratch file of ad-hoc SQL (process-by-size, size-by-date).
  Not compiled or `include_str!`'d anywhere. It is CLI-era operator reference material, not daemon
  code.
- `README.md`: titled "Micromegas Telemetry Admin CLI Crate".

### The daemon and retention (`rust/public/src/servers/maintenance.rs`)
- `daemon(lakehouse, views_to_update, shutdown, grace)` (lines 291-365) spawns four cron loops
  (`every_day`/`every_hour`/`every_minute`/`every_second`).
- `EveryHourTask::run` (lines 98-116) is where retention happens:
  ```rust
  delete_old_data(self.lakehouse.lake(), 90).await?;        // <-- hardcoded 90-day horizon
  delete_expired_temporary_files(self.lakehouse.lake().clone()).await?;
  ```
  The `90` is a bare literal — no constant, config field, or env var.
- `delete_old_data(lake: &DataLakeConnection, min_days_old: i32)`
  (`rust/analytics/src/delete.rs:152`) cascades block/stream/process deletion + partition
  retirement at `now - min_days_old days`.
- The monolith runs the *same* `daemon()` directly from `rust/monolith/src/main.rs:287` under its
  own `maintenance` role; it does **not** shell out to the `telemetry-admin` binary. So both the
  standalone binary and the monolith call `daemon()`, and the retention horizon must be threaded to
  both if it becomes configurable.

### SQL / Python replacements that already exist
- `materialize_partitions` (UDTF), `retire_partitions` (UDTF), `retire_partition_by_metadata`
  (scalar UDF) are registered in `rust/analytics/src/lakehouse/query.rs`
  (`register_lakehouse_functions`, lines ~119-162) and documented in
  `mkdocs/docs/admin/functions-reference.md`. **These are a separate FlightSQL code path and are
  out of scope — do not touch them.** They cover the `materialize-partitions` and
  `retire-partitions` subcommands.
- **There is no SQL/Python equivalent for `delete_old_data` (the retention cascade) or
  `delete_expired_temporary_files`.** Those are reachable only via (a) the automatic hourly daemon
  task, or (b) the subcommands being removed. This is exactly why the retention horizon needs a
  config knob when `delete-old-data` goes away.

### Shutdown / grace-period pattern to mirror
- `shutdown_grace_period_seconds` is a clap field with `long` + `env` +
  `default_value = "25"`, env `MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS`
  (`telemetry_admin.rs:63-72`; monolith `main.rs:158`). The new retention flag follows this same
  shape.

### All references to update (from a full-tree sweep, excluding `rust/target/` and
`tasks/completed/` archives)

**Rust / Cargo**
- `rust/telemetry-admin-cli/Cargo.toml` — package name, bin name, src path.
- `rust/telemetry-admin-cli/` directory + `src/telemetry_admin.rs` filename.
- `rust/Cargo.toml:2` — `members = ["*", …]`; glob, so **no edit needed** for a directory rename.
- `rust/public/src/lib.rs:30` — doc-comment URL to `telemetry-admin-cli/src/telemetry_admin.rs`.
- `rust/public/src/lib.rs:75` — doc-comment `cargo run -p telemetry-admin -- crond`.
- `rust/public/src/servers/maintenance.rs:101` — hardcoded `90` (retention config work).
- `rust/monolith/src/main.rs` — thread the retention knob to `daemon()` (retention config work).

**Docker**
- `docker/admin.Dockerfile` — comment (l.1), `--bin telemetry-admin` (l.20, 22), cp path (l.26),
  `COPY` (l.35), `ENTRYPOINT` (l.37); plus the **filename** itself.
- `docker/all-in-one.Dockerfile` — `--bin telemetry-admin` (l.82, 89), cp path (l.97),
  `COPY` (l.111).
- `docker/README.md` — image table row `admin.Dockerfile` / `micromegas-admin` / "Telemetry admin
  CLI" (l.13), plus image name `micromegas-admin` and command lines (l.124, l.173).
- `build/build_docker_images.py:35` — `"admin": ("admin.Dockerfile", "Telemetry admin CLI")`.
- `build/run_daemon_container.py:12` — `"telemetry-admin crond"` command.
- `.gitignore:40` — `docker/telemetry-admin` (built binary copied into the docker build context).

**local_test_env / dev scripts**
- `local_test_env/ai_scripts/start_services.py` — service-name list (l.36), binary launch +
  `crond` arg (l.204), build `--bin telemetry-admin` (l.400), hardcoded log filename
  `/tmp/admin.log` (write at l.202, echo at l.231) — rename to `/tmp/daemon.log`.
- `local_test_env/ai_scripts/stop_services.py:53` — service-name list; `stop_services.py:66` —
  `log_files` cleanup list entry `/tmp/admin.log`, rename to `/tmp/daemon.log` so it still
  cleans up the daemon's renamed log file.
- `local_test_env/ai_scripts/start_services_with_oidc.py` — service list (l.70),
  `cargo run -p telemetry-admin -- crond` (l.229), hardcoded log filename `/tmp/admin.log` —
  rename to `/tmp/daemon.log` (write at l.227, echo at l.258).
- `local_test_env/dev.py` — build (l.45), `cargo run … -p telemetry-admin -- crond` (l.211).

**Docs (mkdocs + doc/)**
- `mkdocs/docs/admin/maintenance.md` — l.3 intro, l.23 run command, l.26-27 Docker/entrypoint,
  l.39 "single crond instance"; Retention section l.58-66 (document the new horizon knob).
- `mkdocs/docs/admin/service-lifecycle.md` — l.17 table row, l.26 example.
- `mkdocs/docs/architecture/index.md` — l.40 diagram node, l.236 split-mode paragraph.
- `mkdocs/docs/getting-started.md:127` — run command.
- `mkdocs/docs/cost-effectiveness.md:24` — "Maintenance Daemon (`telemetry-admin`)".
- `doc/GETTING_STARTED.md:75` — run command.
- `mkdocs/site/**` is build output — regenerate, do not hand-edit.

**READMEs / meta**
- `rust/telemetry-admin-cli/README.md` — title + description.
- `README.md:50` — "Maintenance Daemon (`telemetry-admin`)".
- `CLAUDE.md:78` — service list mentions `telemetry-admin`.
- `CLAUDE.md:97` — "Admin: `tail -f /tmp/admin.log`"; rename the log path to `/tmp/daemon.log`
  and reword the "Admin:" label to reflect the maintenance daemon.
- `AI_GUIDELINES.md:66` — "`telemetry-admin-cli/`: Administrative CLI tool".
- `.github/copilot-instructions.md:59` — "admin CLI".
- `CHANGELOG.md` — add an Unreleased entry (see Documentation). Leave the historical l.85 entry
  as-is (it records shipped history).

The four hyphenated subcommand names appear in **live** files only in
`rust/telemetry-admin-cli/src/telemetry_admin.rs`; no script, Dockerfile, or current doc invokes
them, so removal is low-risk.

## Design

### 1. Naming
Rename to **`telemetry-maintenance-srv`** (crate, package, and binary). Rationale:
- The repo's long-running binaries all use the `*-srv` suffix: `telemetry-ingestion-srv`,
  `flight-sql-srv`, `analytics-web-srv`, `micromegas-object-cache-srv`. `telemetry-maintenance-srv`
  slots into that family and keeps the `telemetry-` prefix it already had.
- "maintenance" is the word the daemon, the monolith role (`ROLE_MAINTENANCE`), and the docs
  already use for this work.

Directory: `rust/telemetry-admin-cli/` → `rust/telemetry-maintenance-srv/`. Source file:
`src/telemetry_admin.rs` → `src/main.rs` (matches `telemetry-ingestion-srv` and
`analytics-web-srv`, which both use `src/main.rs`). Use `git mv` so history follows.

Docker image: `micromegas-admin` → `micromegas-maintenance`, Dockerfile
`docker/admin.Dockerfile` → `docker/maintenance.Dockerfile`, and the `build_docker_images.py`
image key `admin` → `maintenance`. This is an operator-facing rename of a published image; it is
called out as a breaking deployment change in the CHANGELOG.

### 2. Remove subcommands and collapse the CLI
With `crond` the only mode, the `Commands` enum and the whole subcommand layer go away. `Cli`
becomes a plain `Parser` struct carrying only the daemon's flags:

```rust
#[derive(Parser, Debug)]
#[clap(name = "Micromegas Telemetry Maintenance")]
#[clap(about = "Maintenance daemon for a Micromegas telemetry data lake", version, author)]
struct Cli {
    /// Seconds to wait for in-flight tasks to complete after SIGTERM
    #[clap(long, default_value = "25", env = "MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS")]
    shutdown_grace_period_seconds: u64,

    /// Delete lake data older than this many days (retention horizon)
    #[clap(long, default_value = "90", env = "MICROMEGAS_RETENTION_DAYS")]
    retention_days: i32,
}
```

Notes:
- Drop `#[clap(arg_required_else_help(true))]` — there are no subcommands to require, and the
  daemon should start on a bare invocation.
- `main()` no longer `match`es on a command; it builds the view factory, then calls `daemon(…)`
  directly, passing the grace period and (see below) the retention horizon.
- Delete the now-dead imports (`delete_old_data`, `materialize_partition_range`,
  `delete_expired_temporary_files`, `retire_partitions`, `Context`, `PartitionCache`,
  `TimeRange`, `TimeDelta`, `DateTime`, `Utc`, `std::sync::Arc`). Keep `ResponseWriter`? No — the
  `null_response_writer` construction (line 82) and the `Arc::new(...)` calls it and the removed
  handlers used are deleted along with the handlers; `daemon()` builds its own `Arc`-wrapped
  context. Verify with `cargo build` + `cargo clippy` that no unused-import warnings remain.

### 3. Make the retention horizon configurable
Thread a `retention_days: i32` value into the daemon so the hourly task's hardcoded `90`
becomes the passed-in value. This preserves the *only* capability lost with `delete-old-data`
(choosing the horizon) while keeping retention automatic.

Change the daemon signature in `rust/public/src/servers/maintenance.rs`:
```rust
pub async fn daemon<F>(
    lakehouse: Arc<LakehouseContext>,
    mut views_to_update: Vec<Arc<dyn View>>,
    retention_days: i32,
    shutdown: F,
    grace: Duration,
) -> Result<()>
```
Store `retention_days` on `EveryHourTask` and use it in place of the literal `90`:
```rust
pub struct EveryHourTask {
    pub lakehouse: Arc<LakehouseContext>,
    pub views: Views,
    pub retention_days: i32,
}
// in run():
delete_old_data(self.lakehouse.lake(), self.retention_days).await?;
```
Both callers must pass the value:
- `telemetry-maintenance-srv` `main.rs`: pass `args.retention_days`.
- `rust/monolith/src/main.rs`: add a matching `--retention-days` / `MICROMEGAS_RETENTION_DAYS`
  clap field (default 90) and pass it into `daemon(…)` at line 287. This keeps the monolith's
  maintenance role and the standalone daemon behaving identically and configured the same way.

This is the recommended approach over the alternatives (a bare constant, or leaving 90 hardcoded);
see Trade-offs.

### 4. `queries.sql`
It is unreferenced CLI-era scratch material and does not belong in a daemon crate. Delete it as
part of the move (its two queries — data size by process and by date — are trivial ad-hoc SQL a
user runs through the query CLI, and are not documentation the project relies on). If a reviewer
prefers to preserve them, relocate into a docs snippet instead of carrying them in the service
crate. Recommended: delete.

## Implementation Steps

### Phase 1 — Rename the crate/binary
1. `git mv rust/telemetry-admin-cli rust/telemetry-maintenance-srv`.
2. `git mv rust/telemetry-maintenance-srv/src/telemetry_admin.rs rust/telemetry-maintenance-srv/src/main.rs`.
3. `git rm rust/telemetry-maintenance-srv/src/queries.sql`.
4. Edit `rust/telemetry-maintenance-srv/Cargo.toml`: package `name = "telemetry-maintenance-srv"`,
   `[[bin]] name = "telemetry-maintenance-srv"`, `path = "src/main.rs"`, and update the package
   `description`.
5. Rewrite `rust/telemetry-maintenance-srv/README.md` title/description for the daemon.
6. Build to confirm the workspace glob picks up the new directory: `cargo build` from `rust/`.

### Phase 2 — Collapse the CLI, remove subcommands, and make retention configurable
Note: the `daemon()` signature change and the standalone binary's switch to the new call arity are
done together in this phase, with a single build checkpoint at the end (step 12) — no intermediate
step asserts a passing `cargo build` while the two sides of the call are mismatched.
7. In `rust/public/src/servers/maintenance.rs`: add `retention_days` param to `daemon()`, add the
   `retention_days` field to `EveryHourTask`, populate it when constructing the hourly task, and
   replace the literal `90` at line 101 with `self.retention_days`.
8. In `rust/monolith/src/main.rs`: add `--retention-days` / `MICROMEGAS_RETENTION_DAYS` clap field
   (default 90) and pass it into the `daemon(…)` call (line ~287).
9. In `src/main.rs` (telemetry-maintenance-srv): replace the `Cli`/`Commands` definitions with the
   flat `Parser` struct (grace period + retention_days), drop `arg_required_else_help`, remove the
   `Subcommand` import, and update the module doc-comment on line 1 (`//! Telemetry Admin CLI`) to
   reflect the maintenance daemon (e.g. `//! Telemetry maintenance daemon`).
10. Replace the `match args.command { … }` block with a direct daemon launch (build view factory →
    `get_global_views_with_update_group` → `daemon(lakehouse, views, args.retention_days,
    wait_for_sigterm(), grace)`). `daemon()` already takes the matching 5 parameters from step 7,
    so this is the only place the call arity changes.
11. Delete all imports that were only used by the removed handlers (including `std::sync::Arc` —
    see Design §2).
12. `cargo build` + `cargo clippy --workspace -- -D warnings` — zero unused-import/dead-code
    warnings; both `telemetry-maintenance-srv` and `micromegas-monolith` compile.

### Phase 3 — Docker
13. `git mv docker/admin.Dockerfile docker/maintenance.Dockerfile`; update the comment, all
    `--bin telemetry-admin` → `--bin telemetry-maintenance-srv`, cp/COPY paths, and
    `ENTRYPOINT ["telemetry-maintenance-srv"]`.
14. `docker/all-in-one.Dockerfile`: `--bin telemetry-admin` → `--bin telemetry-maintenance-srv`
    (l.82, 89), cp path (l.97), COPY (l.111).
15. `build/build_docker_images.py:35`: key `admin` → `maintenance`, filename
    `maintenance.Dockerfile`, description "Maintenance daemon".
16. `build/run_daemon_container.py:12`: command token `"telemetry-admin crond"` →
    `"telemetry-maintenance-srv"`. The image on line 11 (`marcantoinedesroches/micromegas-all:latest`)
    is the separate all-in-one image and is **not** renamed — leave it as-is.
17. `docker/README.md`: image table row (l.13) — Dockerfile `admin.Dockerfile` →
    `maintenance.Dockerfile`, image `micromegas-admin` → `micromegas-maintenance`, description
    "Telemetry admin CLI" → "Maintenance daemon"; plus image `micromegas-admin` →
    `micromegas-maintenance`. In the "Admin daemon" run block (against the renamed
    `micromegas-admin`/`micromegas-maintenance` image, whose entrypoint is the binary), just drop
    the `crond` arg (l.124). In the All-in-One block (against the un-renamed `micromegas-all`
    image, which takes the binary name as its command), rename the binary token itself:
    `telemetry-admin crond` → `telemetry-maintenance-srv` (l.173) — dropping only `crond` there
    would leave the nonexistent `telemetry-admin` binary name.
18. Update `.gitignore:40`: `docker/telemetry-admin` → `docker/telemetry-maintenance-srv`.

### Phase 4 — Scripts
19. `local_test_env/ai_scripts/start_services.py`: service name `telemetry-admin` →
    `telemetry-maintenance-srv` (l.36), launch `[str(target_dir / "telemetry-maintenance-srv")]`
    dropping the `"crond"` arg (l.204), build `--bin telemetry-maintenance-srv` (l.400), and
    rename the hardcoded log filename `/tmp/admin.log` → `/tmp/daemon.log` (write at l.202,
    echo at l.231).
20. `local_test_env/ai_scripts/stop_services.py`: service name (l.53), and rename the
    `log_files` cleanup list entry `/tmp/admin.log` → `/tmp/daemon.log` (l.66) so it still
    cleans up the daemon's renamed log file.
21. `local_test_env/ai_scripts/start_services_with_oidc.py`: service list (l.70),
    `cargo run -p telemetry-maintenance-srv --` dropping `crond` (l.229), and the hardcoded log
    filename `/tmp/admin.log` → `/tmp/daemon.log` (write at l.227, echo at l.258).
22. `local_test_env/dev.py`: build `-p telemetry-maintenance-srv` (l.45),
    `cargo run … -p telemetry-maintenance-srv` dropping `crond` (l.211).

### Phase 5 — Docs & meta
23. Update `mkdocs/docs/admin/maintenance.md`: binary name, run command
    (`cargo run --release --bin telemetry-maintenance-srv`, no `crond`), Docker/entrypoint prose,
    and the Retention section to document `--retention-days` / `MICROMEGAS_RETENTION_DAYS`
    (default 90) instead of describing 90 as fixed.
24. Update `mkdocs/docs/admin/service-lifecycle.md` (table row + example; binary is now
    `telemetry-maintenance-srv` with no `crond`), `mkdocs/docs/architecture/index.md` (diagram node
    + split-mode paragraph), `mkdocs/docs/getting-started.md`, `mkdocs/docs/cost-effectiveness.md`,
    `doc/GETTING_STARTED.md`.
25. Update `rust/public/src/lib.rs` doc comments (l.30 path, l.75 run command).
26. Update `README.md:50`, `CLAUDE.md:78` (service list), `CLAUDE.md:97` (rename the "Admin:"
    log-path line's `/tmp/admin.log` → `/tmp/daemon.log` and reword the "Admin:" label for the
    maintenance daemon), `AI_GUIDELINES.md:66`, `.github/copilot-instructions.md:59`.
27. Add a `CHANGELOG.md` Unreleased entry (removed subcommands; binary + Docker-image rename as a
    breaking deployment change; new `MICROMEGAS_RETENTION_DAYS` knob; note that the public crate's
    `daemon()` signature changed with a new `retention_days` parameter, matching the precedent set
    by the existing l.85 entry for #1037).

### Phase 6 — Verify
28. From `rust/`: `cargo fmt`, `cargo build`, `cargo clippy --workspace -- -D warnings`,
    `cargo test`.
29. Run `local_test_env/ai_scripts/start_services.py` and confirm the daemon starts under its new
    name and materializes partitions (see Testing Strategy).
30. `python build/build_docker_images.py` (or the maintenance image only) to confirm the renamed
    Dockerfile/image build.

## Files to Modify
- `rust/telemetry-admin-cli/` → `rust/telemetry-maintenance-srv/` (dir), `Cargo.toml`,
  `src/telemetry_admin.rs` → `src/main.rs`, `README.md`, remove `src/queries.sql`
- `rust/public/src/servers/maintenance.rs`, `rust/public/src/lib.rs`
- `rust/monolith/src/main.rs`
- `docker/admin.Dockerfile` → `docker/maintenance.Dockerfile`, `docker/all-in-one.Dockerfile`,
  `docker/README.md`
- `build/build_docker_images.py`, `build/run_daemon_container.py`
- `.gitignore` (`docker/telemetry-admin` → `docker/telemetry-maintenance-srv`)
- `local_test_env/ai_scripts/start_services.py`, `stop_services.py`,
  `start_services_with_oidc.py`, `local_test_env/dev.py`
- `mkdocs/docs/admin/maintenance.md`, `mkdocs/docs/admin/service-lifecycle.md`,
  `mkdocs/docs/architecture/index.md`, `mkdocs/docs/getting-started.md`,
  `mkdocs/docs/cost-effectiveness.md`, `doc/GETTING_STARTED.md`
- `README.md`, `CLAUDE.md`, `AI_GUIDELINES.md`, `.github/copilot-instructions.md`, `CHANGELOG.md`

## Trade-offs

- **Binary name `telemetry-maintenance-srv` vs. `maintenance-daemon` / `telemetry-maintenance`.**
  Chosen `telemetry-maintenance-srv` because it matches the repo's uniform `*-srv` binary suffix
  and keeps the `telemetry-` namespace. `maintenance-daemon` breaks the suffix convention;
  bare `telemetry-maintenance` loses the "it's a long-running service" signal the suffix carries.

- **Retention: configurable knob vs. leave 90 hardcoded vs. a named constant.** Removing
  `delete-old-data` deletes the only way to pick a horizon, and there is no SQL replacement for the
  retention cascade. A config knob (default 90) preserves that capability with zero behavior change
  at the default, and turns a one-off manual purge into a proper service policy. Leaving `90`
  hardcoded would silently drop a real capability; a named constant improves readability but still
  can't be changed without a rebuild. The knob is threaded through both the standalone daemon and
  the monolith so the two stay identical.

- **Drop the subcommand layer entirely vs. keep a no-op `crond`.** With one mode, keeping `crond`
  as the sole subcommand is pure ceremony and keeps the misleading "this is a CLI with modes"
  framing. Dropping it makes the invocation `telemetry-maintenance-srv` (matching the other `*-srv`
  binaries, which take flags, not subcommands). All scripts/docs that passed `crond` are updated.

- **Rename the Docker image vs. keep `micromegas-admin`.** Keeping the old image name would leave
  the exact "named like an admin tool" problem the issue is fixing, just in the container layer.
  Renaming is a breaking deployment change, so it is documented in the CHANGELOG; the cost is a
  one-time deploy-manifest update for operators.

- **Delete `queries.sql` vs. keep/relocate.** It is unreferenced scratch SQL that only made sense
  when this was an operator CLI. Deleting keeps the service crate clean; the queries are trivial to
  reconstruct via the query CLI. Relocation to docs is the fallback if a reviewer wants them kept.

## Documentation
- **`mkdocs/docs/admin/maintenance.md`** — primary daemon page: new binary name, run command
  without `crond`, Docker entrypoint prose, and a rewritten Retention section documenting
  `--retention-days` / `MICROMEGAS_RETENTION_DAYS` (default 90).
- **`mkdocs/docs/admin/service-lifecycle.md`** — service table row + example command.
- **`mkdocs/docs/architecture/index.md`**, **`getting-started.md`**, **`cost-effectiveness.md`**,
  **`doc/GETTING_STARTED.md`** — name/command updates.
- **`CHANGELOG.md`** — Unreleased entry covering: removal of the four subcommands; binary rename
  `telemetry-admin` → `telemetry-maintenance-srv` and Docker image `micromegas-admin` →
  `micromegas-maintenance` (breaking for deployments); new `MICROMEGAS_RETENTION_DAYS` knob; the
  public crate's `daemon()` signature change (new `retention_days` parameter), matching the l.85
  precedent that calls out `daemon()`/`run_tasks_forever` signature changes.
- **`rust/telemetry-maintenance-srv/README.md`**, top-level **`README.md`** — daemon framing.
- The deprecated subcommands are not documented in any live page (the docs already steer users to
  SQL functions), so there is no subcommand reference to remove — only naming to update.

## Testing Strategy
- `cargo build` + `cargo clippy --workspace -- -D warnings` — no dead-code/unused-import warnings
  after subcommand removal; both `telemetry-maintenance-srv` and `micromegas-monolith` compile.
- `cargo fmt --check` and `cargo test` from `rust/`.
- `cargo run --bin telemetry-maintenance-srv -- --help` shows only `--shutdown-grace-period-seconds`
  and `--retention-days` (no subcommands); a bare `cargo run --bin telemetry-maintenance-srv`
  starts the daemon.
- End-to-end via `local_test_env/ai_scripts/start_services.py`: services come up, the maintenance
  daemon starts under its new name, and partitions materialize (check `/tmp/daemon.log` /
  the daemon log and query `list_partitions()`).
- Set `MICROMEGAS_RETENTION_DAYS=1` and confirm the hourly task logs deletion at the configured
  horizon (or unit-test `EveryHourTask` wiring if a lighter check is preferred).
- `python build/build_docker_images.py` builds the renamed `maintenance` image; grep the tree for
  residual `telemetry-admin` / `crond` references (outside `tasks/completed/` and `CHANGELOG` l.85)
  to confirm nothing live was missed.

## Resolved Decisions
- **Binary name**: `telemetry-maintenance-srv`, matching the `*-srv` convention (see Naming and
  Trade-offs).
- **Docker image rename**: confirmed — `micromegas-admin` → `micromegas-maintenance`. This is a
  breaking change for deployments pulling the published image; it is called out in the CHANGELOG
  entry (see Trade-offs).
</content>
</invoke>
