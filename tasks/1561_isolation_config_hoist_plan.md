# One `MICROMEGAS_PUBLIC_VIEW_SETS`, Resolved Once Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1561

## Overview

`MICROMEGAS_PUBLIC_VIEW_SETS` is read on only one of `FlightSqlServerBuilder`'s three auth
branches. A deployment that injects its own auth provider sets the variable, restarts, and gets
no allowlist and no startup error — the knob is inert, and a malformed value is not even
detected. Two changes, both removals: resolve the config at exactly one site in the builder,
unconditional and ahead of anything expensive; and collapse the knob to a single unprefixed
variable, dropping the per-service `MICROMEGAS_ANALYTICS_` form. Nothing about the allowlist
varies by service or by auth branch, so neither dimension should exist.

## Current State

`rust/public/src/servers/flight_sql_server.rs`. The auth branch at `:281` produces a 3-tuple
(`AuthAndDefaults`, `:44`) whose third element is the default `IsolationConfig`:

| Branch | Default `IsolationConfig` |
|---|---|
| `with_auth_provider(..)` (injected provider) | `IsolationConfig::default()` — env never read (`:312`) |
| `with_default_auth()` | `IsolationConfig::from_env("")?` (`:346`) |
| auth disabled | `IsolationConfig::default()` (`:361`) |

`self.isolation_config` overrides whichever default the branch produced (`:365`), so the only way
to get the allowlist on the injected-provider path today is an explicit `with_isolation_config`
call — and nothing tells an operator that.

The per-branch shape is presumably an analogy to `read_policy`, whose default genuinely does
differ per branch: it needs a DB-backed grant store, and the injected-provider branch
deliberately skips building one when `with_read_policy` was set (the comment at `:283`).
`IsolationConfig` has no such dependency — it is one `Vec<String>` parsed from the environment.

`IsolationConfig::from_env(prefix)` (`rust/analytics/src/lakehouse/read_scope.rs:179`) returns
`Err` on a malformed `PUBLIC_VIEW_SETS` entry (an empty entry, or `[`/`]`/`"` from someone
assuming the `MICROMEGAS_API_KEYS` JSON shape) and on the removed `UNSTAMPED_AUDIENCE` knob being
set at all, so a startup `?` turns a typo into a fail-fast rather than a silently-inert knob. Two
of the three branches never call it, so they get neither the value nor the fail-fast.

It resolves both names through `resolved_var` (`:153`), the `{prefix}_X`-then-`MICROMEGAS_X`
fallback helper — private to `read_scope.rs` and used by nothing else. The only caller passing a
non-empty prefix is the monolith (`rust/monolith/src/main.rs:300`, `"MICROMEGAS_ANALYTICS"`),
which resolves the config itself and passes it in via `with_isolation_config` (`:350`) — but only
when `roles.flightsql && !args.disable_auth` (`:299`).

**So no in-repo binary reproduces the visibility half of the bug**: with auth enabled the monolith
resolves the value itself; with `--disable-auth` it falls through to the builder default, where
the allowlist is a no-op anyway. The inert knob is reachable only by an out-of-repo embedder
calling `with_auth_provider` without `with_isolation_config` — exactly the shape the builder's API
invites, since nothing in its signature says the pairing is required. The fail-fast half *is*
reachable in-repo, on both `--disable-auth` paths.

Existing coverage: `rust/analytics/tests/ownership_rewrite_config_tests.rs` (`from_env` parsing
and the removed-knob refusal — rewritten by this plan, see Testing Strategy),
`rust/analytics/tests/ownership_rewrite_public_view_set_tests.rs` (a listed view set gets no
predicate) and `rust/analytics/tests/audience_guard_tests.rs::global_rows_visible_via_public_view_sets`,
both of which construct `IsolationConfig` directly and stay valid untouched.

## Design

### One variable

`IsolationConfig::from_env` loses its `prefix` parameter and reads `MICROMEGAS_PUBLIC_VIEW_SETS`
only. `resolved_var` has no remaining caller and is deleted with it.

The three prefixed spellings that resolve today must be **refused at startup**, not silently
ignored — the same posture `from_env` already takes for `UNSTAMPED_AUDIENCE`, and the same class
of failure this plan exists to remove. `MICROMEGAS_ANALYTICS_UNSTAMPED_AUDIENCE` is refused today
and must keep being refused; `MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS` is honoured today and becomes
a refusal naming the unprefixed variable as its replacement:

```rust
pub fn from_env() -> anyhow::Result<Self> {
    // Refused rather than ignored: this knob resolved under the prefix until the per-service
    // form was collapsed, so an operator's existing value must fail loudly instead of
    // becoming an inert setting they cannot tell from an unset one.
    if std::env::var("MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS").is_ok() {
        anyhow::bail!(
            "MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS is no longer supported: the public-view-set \
             allowlist is one deployment-wide setting with no per-service form. Rename it to \
             MICROMEGAS_PUBLIC_VIEW_SETS."
        );
    }
    for var in ["MICROMEGAS_UNSTAMPED_AUDIENCE", "MICROMEGAS_ANALYTICS_UNSTAMPED_AUDIENCE"] {
        if std::env::var(var).is_ok() {
            anyhow::bail!(/* existing MICROMEGAS_DEFAULT_AUDIENCE text, {var} interpolated */);
        }
    }
    let public_view_sets = parse_comma_separated_list("MICROMEGAS_PUBLIC_VIEW_SETS")?;
    Ok(Self { public_view_sets })
}
```

### One resolution site

A private free function beside the builder:

```rust
/// The env is not read at all when the caller supplied a config: `with_isolation_config` wins
/// regardless, so reading anyway would invent a startup failure mode on a variable the caller
/// deliberately overrode.
fn resolve_isolation_config(
    explicit: Option<Arc<IsolationConfig>>,
) -> Result<Arc<IsolationConfig>> {
    match explicit {
        Some(config) => Ok(config),
        None => Ok(Arc::new(IsolationConfig::from_env()?)),
    }
}
```

Called at the very top of `build_and_serve`, before `LakehouseContext::from_env()`.
`FlightSqlServerBuilder` has no `Drop` impl and `build_and_serve` already moves fields out of
`self` one at a time, so taking `self.isolation_config` first is a normal partial move.

### What comes out

- `AuthAndDefaults` (`:44`) drops its third element, becoming
  `(Option<Arc<dyn AuthProvider>>, Arc<dyn ReadPolicy>)`; its doc comment drops the
  `IsolationConfig` half. The name still fits — it is auth plus the one default that genuinely is
  per-branch.
- Each of the three branch tails stops returning an `IsolationConfig` (`:312`, `:346`, `:361`),
  and the `let isolation_config = self.isolation_config.unwrap_or(...)` line at `:365` goes away.
- `with_isolation_config`'s doc comment (`:158-165`) loses the per-branch default sentence and
  states the single default: `IsolationConfig::from_env()`, on every branch.
- `rust/monolith/src/main.rs`: `analytics_isolation_config` (`:294-303`) and the
  `with_isolation_config` call (`:349-351`) are deleted outright, along with the
  `IsolationConfig` import (`:26`). The monolith now takes the builder's resolution like any
  other embedder — there is no longer a per-service value for it to compute.

### Behaviour changes

1. **`MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS` is refused at startup.** A monolith deployment
   setting it does not start until the operator renames it to `MICROMEGAS_PUBLIC_VIEW_SETS`. This
   is the one change that breaks a running deployment, and it is deliberate: silently ignoring it
   would leave exactly the inert knob this plan removes.
2. **Visibility widens on upgrade** for a deployment on the injected-provider path that already
   has `MICROMEGAS_PUBLIC_VIEW_SETS` set: the listed view sets stop being audience-filtered. No
   in-repo binary is affected — only an out-of-repo embedder that never called
   `with_isolation_config`.
3. **A malformed value now fails startup** on the injected-provider and auth-disabled paths
   instead of being ignored, including the `UNSTAMPED_AUDIENCE` refusal: a `--disable-auth`
   `flight-sql-srv` or monolith with a stale value set now refuses to start where it previously
   came up.

On the auth-disabled branch the allowlist itself is a no-op either way — no `AuthContext`
extension is ever inserted, so the absent-extension convention supplies `ReadScope::All`, which
makes `OwnershipRewrite` a true no-op. Only the injected-provider path changes visibility in
practice.

## Implementation Steps

1. `rust/analytics/src/lakehouse/read_scope.rs`: drop `from_env`'s `prefix` parameter, delete
   `resolved_var`, add the `MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS` refusal, and refuse both
   `UNSTAMPED_AUDIENCE` spellings explicitly. Update the `IsolationConfig` and `from_env` doc
   comments to describe one unprefixed variable.
2. `rust/public/src/servers/flight_sql_server.rs`: add `resolve_isolation_config` and call it at
   the top of `build_and_serve`, before the injected-lakehouse branch.
3. Same file: shrink `AuthAndDefaults` to two elements, update its doc comment, drop the
   `IsolationConfig` from each of the three branch tails, and delete the `unwrap_or` line at
   `:365`.
4. Same file: rewrite `with_isolation_config`'s doc comment to name the one default.
5. Same file: add `#[cfg(test)] mod tests` covering the resolution (see Testing Strategy).
6. `rust/monolith/src/main.rs`: delete `analytics_isolation_config`, the `with_isolation_config`
   call, and the now-unused `IsolationConfig` import.
7. Rewrite `rust/analytics/tests/ownership_rewrite_config_tests.rs` for the prefix-free signature
   (see Testing Strategy), including its module doc comment.
8. Add `rust/public/tests/isolation_config_fail_fast_tests.rs` and its `[[test]]` entry in
   `rust/public/Cargo.toml` (see Testing Strategy).
9. `mkdocs/docs/admin/monolith.md`: rename the `:51` row to `MICROMEGAS_PUBLIC_VIEW_SETS`, drop
   the "falls back to unprefixed" clause, and add it to the always-unprefixed list in the
   "One prefix asymmetry" note (`:60-68`).
10. `CHANGELOG.md`: an **Analytics** bullet under `## Unreleased` covering the fix and all three
    behaviour changes, with change (1) written as a `**Minor breaking change**:` clause.
11. `cargo fmt`, `cargo clippy --workspace --all-targets`, `cargo test -p micromegas --features server`,
    `cargo test -p micromegas-analytics`.

## Files to Modify

- `rust/analytics/src/lakehouse/read_scope.rs` — prefix-free `from_env`, `resolved_var` deleted,
  the new refusal
- `rust/public/src/servers/flight_sql_server.rs` — the hoist, the doc comments, the unit tests
- `rust/monolith/src/main.rs` — delete the per-service resolution and its import
- `rust/analytics/tests/ownership_rewrite_config_tests.rs` — rewritten for the new signature
- `rust/public/tests/isolation_config_fail_fast_tests.rs` — new integration test
- `rust/public/Cargo.toml` — new `[[test]]` entry, `required-features = ["server"]`
- `mkdocs/docs/admin/monolith.md` — the renamed variable and the prefix-asymmetry note
- `CHANGELOG.md` — behaviour-change entry

## Trade-offs

**Refusing the prefixed variable vs. silently ignoring it.** Ignoring is a one-line change and
breaks no running deployment, but it reproduces the exact defect being fixed: an operator's value
becomes indistinguishable from an unset one. The refusal costs one rename, in a variable already
documented as optional, and the repo has the established mechanism for it.

**Free function vs. inlining the `match` at the call site.** Two lines inlined would be
marginally simpler to read, but three of the four required cases (env honoured, unset,
explicit-wins) are not reachable from an integration test without a live lake, and are not worth
a live-DB test under the project's testing rule. A private function makes all four a plain unit
test.

## Decisions

- Resolve at the top of `build_and_serve` rather than just before the auth branch: fail-fast on a
  typo before the Postgres/object-store setup, at no cost.
- Keep `with_isolation_config` even though no in-repo caller uses it after step 6 — it mirrors
  `with_read_policy` and stays the escape hatch for an out-of-repo embedder.
- Accept the visibility widening in change (2) as the price of fixing the bug, documented in
  `CHANGELOG.md` so an operator can audit the value pre-upgrade.
- Skip the env read when `with_isolation_config` was set — same reasoning as the injected-provider
  `ReadPolicy` default at `:293`.
- No startup log line for the resolved allowlist — declined as out of scope for this fix.

## Documentation

`mkdocs/docs/admin/monolith.md` is the only page that documents the prefixed form (`:51`) and
needs the rename plus a line in its "One prefix asymmetry" note, which already enumerates the
knobs that are always unprefixed under the monolith.

The other four hits need no change — `admin/flight-sql.md:33`, `admin/authorization.md:22` and
`:193`, and `admin/functions-reference.md:75` all document the unprefixed variable with no
auth-branch precondition, which the fix makes true. Verify during implementation that no other
page has since acquired a branch-conditional or prefixed claim
(`grep -rn PUBLIC_VIEW_SETS mkdocs/docs/` — scoped to `docs/`, since `mkdocs/site/` is generated
build output that swamps the real hits).

## Testing Strategy

**`#[cfg(test)] mod tests` in `rust/public/src/servers/flight_sql_server.rs`** over
`resolve_isolation_config` — the function is private to the builder, so no integration test can
reach it. This will be the first unit-test module in `rust/public/src`; `serial_test` is already
a dev-dependency (`rust/public/Cargo.toml:92`). The `servers` module, and this test module with
it, only compiles under the non-default `server` feature, so `cargo test -p micromegas` alone
builds zero of these tests silently — use `cargo test -p micromegas --features server`.

Every test mutates process-wide env, so each is `#[serial]` with an `EnvGuard` that clears
`MICROMEGAS_PUBLIC_VIEW_SETS` and both `UNSTAMPED_AUDIENCE` spellings on drop — the same pattern
as `ownership_rewrite_config_tests.rs`. The refused variables need clearing too: `from_env` bails
on them outright, so an ambient value would fail every case for the wrong reason. Cargo runs each
test binary as its own process, so the analytics test file's use of the same variables cannot
leak in.

Cases:

1. env set, `with_isolation_config` never called → the allowlist is parsed from the environment.
2. env unset → empty `public_view_sets`.
3. env malformed (e.g. `["a"]`) → `Err`.
4. explicit config supplied → it wins; with the env simultaneously set to a *different* value,
   and separately to a *malformed* value, to pin that the env is not read at all on that path.

The hoist removes the auth-branch dimension, so no case needs repeating per branch — that is
precisely what makes these four sufficient.

**`rust/analytics/tests/ownership_rewrite_config_tests.rs`**, rewritten for the prefix-free
signature. Its four `PUBLIC_VIEW_SETS` parsing tests (comma-separated list, JSON-array-shaped
entries rejected, empty entries rejected, all-whitespace → empty) and `unset_vars_resolve_to_default`
carry over verbatim against `from_env()`. Its three prefix-resolution tests collapse: the two
prefixed-`UNSTAMPED_AUDIENCE` cases become one test naming
`MICROMEGAS_ANALYTICS_UNSTAMPED_AUDIENCE` explicitly, and `unprefixed_unstamped_audience_var_is_also_a_startup_error`
loses its "also" framing. Add one new test: `MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS` set to a
*well-formed* value is a startup error, which is what pins the refusal rather than a silent
ignore. Its module doc comment must be updated too — the existing text claims these vars are
touched by no other test in the repo, which the new `rust/public` tests falsify (harmlessly:
separate processes).

**`rust/public/tests/isolation_config_fail_fast_tests.rs`**: one integration test asserting
`FlightSqlServer::builder().build_and_serve().await` returns the "comma-separated, not a JSON
array like MICROMEGAS_API_KEYS" error with `MICROMEGAS_PUBLIC_VIEW_SETS` malformed. Its
`EnvGuard` must also remove `MICROMEGAS_SQL_CONNECTION_STRING` and `MICROMEGAS_OBJECT_STORE_URI`
(restoring them on drop): those are normally exported in a developer's shell, and without
clearing them the test still passes after connecting to a real lake, which would quietly reduce
it to a duplicate of unit case 3. Clearing them is what makes reaching the error *without* a lake
the assertion that the resolution runs ahead of `LakehouseContext::from_env()` — the silent
regression no other test covers, since a well-formed deployment starts normally either way. No
`#[serial]`: cargo gives this `[[test]]` its own single-test binary, so the attribute would
serialize nothing. Registered in `rust/public/Cargo.toml` with
`required-features = ["server"]`, matching the existing entries.

No new live-DB test: nothing here reproduces a bug witnessed in the wild against a real database.

## Manual Verification

The visibility change is not reachable from any in-repo binary (see Current State), so there is
nothing to check by hand for it. Both fail-fast paths are covered above — the builder's by the
integration test, `from_env`'s refusals by unit tests. What remains is one smoke check that the
servers still start once the resolution moved out of the monolith, which is not automated because
it needs a real lake and object store, and whose breakage would be immediately obvious on the next
run rather than silent.

1. `python3 local_test_env/ai_scripts/start_services.py` and again with `--monolith`, with a valid
   `MICROMEGAS_PUBLIC_VIEW_SETS=log_entries` exported. Expected: both come up, and
   `micromegas-query "SELECT count(*) FROM log_entries" --begin 1h` returns rows against each.
   Both default to `--disable-auth`, so this does not reach `with_default_auth` or the
   injected-provider branch; reaching those needs `flight-sql-srv` without `--disable-auth` plus
   `MICROMEGAS_API_KEYS` set, or `--monolith` with `MICROMEGAS_OIDC_CONFIG` /
   `MICROMEGAS_ANALYTICS_OIDC_CONFIG` set.
2. With `MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS=log_entries` exported instead, the monolith must
   refuse to start, naming `MICROMEGAS_PUBLIC_VIEW_SETS`. This is the upgrade path an existing
   deployment hits, and the unit test for the refusal cannot show that the monolith actually
   reaches `from_env` after step 6 deletes its own resolution.
