# Hoist `IsolationConfig` Resolution Out of the Auth Branch Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1561

## Overview

`MICROMEGAS_PUBLIC_VIEW_SETS` is read on only one of `FlightSqlServerBuilder`'s three auth
branches. A deployment that injects its own auth provider sets the variable, restarts, and gets
no allowlist, no startup error, and no log line — the knob is inert, and a malformed value is not
even detected. Hoist the resolution out of the auth branch so there is exactly one site,
unconditional and resolved before anything expensive runs. `with_isolation_config` still wins;
the per-branch value disappears entirely rather than gaining a third correct copy.

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
`IsolationConfig` has no such dependency — it is one `Vec<String>` parsed from the environment,
per-service deployment config with nothing to do with which auth provider is configured.

`IsolationConfig::from_env` (`rust/analytics/src/lakehouse/read_scope.rs:179`) returns `Err` on a
malformed `PUBLIC_VIEW_SETS` entry (an empty entry, or `[`/`]`/`"` from someone assuming the
`MICROMEGAS_API_KEYS` JSON shape) and on the removed `UNSTAMPED_AUDIENCE` knob being set at all,
so a startup `?` turns a typo into a fail-fast rather than a silently-inert knob. Two of the
three branches never call it, so they get neither the value nor the fail-fast.

The only in-repo caller on the injected-provider path is the monolith
(`rust/monolith/src/main.rs:344`), and it does call `with_isolation_config` — but only when
`roles.flightsql && !args.disable_auth` (`:299`). **So no in-repo binary reproduces the
visibility half of the bug**: with auth enabled the monolith resolves
`IsolationConfig::from_env("MICROMEGAS_ANALYTICS")` itself (which falls back to the unprefixed
variable) and passes it in; with `--disable-auth` it falls through to the builder default, where
the allowlist is a no-op anyway. The inert knob is reachable only by an out-of-repo embedder
calling `with_auth_provider` without `with_isolation_config` — which is exactly the shape the
builder's API invites, since nothing in its signature or its logs says the pairing is required.
The fail-fast half *is* reachable in-repo, on both `--disable-auth` paths.

Existing coverage, all of which stays valid: `rust/analytics/tests/ownership_rewrite_config_tests.rs`
(`from_env` parsing and the removed-knob refusal), `rust/analytics/tests/ownership_rewrite_public_view_set_tests.rs`
(a listed view set gets no predicate), `rust/analytics/tests/audience_guard_tests.rs::global_rows_visible_via_public_view_sets`.

## Design

### One resolution site

A private free function beside the builder, so the resolution is reachable from a unit test
without standing up `build_and_serve`'s lakehouse:

```rust
/// The builder's single `IsolationConfig` resolution. Independent of which auth provider is
/// configured -- unlike the `ReadPolicy` default beside it, this is per-service deployment
/// config with no store to build, so resolving it per auth branch could only produce three
/// copies of one value.
///
/// The env is not read at all when the caller supplied a config: `with_isolation_config` wins
/// regardless, and reading anyway would invent a startup failure mode on a malformed
/// *unprefixed* variable that the caller deliberately overrode -- the monolith resolves
/// `MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS` itself and passes the result in.
fn resolve_isolation_config(
    explicit: Option<Arc<IsolationConfig>>,
) -> Result<Arc<IsolationConfig>> {
    match explicit {
        Some(config) => Ok(config),
        None => Ok(Arc::new(IsolationConfig::from_env("")?)),
    }
}
```

Called at the very top of `build_and_serve`, before `LakehouseContext::from_env()`. Placing it
there rather than merely before the auth branch buys the earliest possible fail-fast: a typo in
the variable fails startup before any Postgres connection or object-store handshake.
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
  states the single default: `IsolationConfig::from_env("")`, on every branch.

### Monolith: drop the `!args.disable_auth` gate

`rust/monolith/src/main.rs:299` currently resolves the config only when auth is enabled. With the
hoist in place, leaving that gate means a `--disable-auth` monolith silently swaps the prefixed
variable for the unprefixed one — the builder's `from_env("")` never sees
`MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS`. The allowlist is a no-op on that path either way, so
what this actually buys is a uniform fail-fast: a typo in the prefixed variable is caught during
local `--disable-auth` development, which is where an operator is most likely to be editing it,
instead of surviving until the first authenticated deploy. Gate on `roles.flightsql` alone:

```rust
let analytics_isolation_config = if roles.flightsql {
    Some(Arc::new(IsolationConfig::from_env("MICROMEGAS_ANALYTICS")?))
} else {
    None
};
```

`analytics_read_policy`'s own `!args.disable_auth` gate is untouched — that one genuinely needs
the DB grant store and matches the builder's per-branch `ReadPolicy` default.

### Behaviour changes

1. **Visibility widens on upgrade** for a deployment on the injected-provider path that already
   has `MICROMEGAS_PUBLIC_VIEW_SETS` set: the listed view sets stop being audience-filtered. No
   in-repo binary is affected — only an out-of-repo embedder that never called
   `with_isolation_config`.
   Normally something this project avoids, but the variable's only documented meaning is "honour
   this" — nobody can be relying on it being ignored without also believing it is in effect — and
   the alternative is leaving the bug in place. Call it out explicitly so an operator can audit
   the value before upgrading.
2. **A malformed value now fails startup** on the injected-provider and auth-disabled paths
   instead of being ignored. This includes the removed-knob refusal: a `--disable-auth`
   `flight-sql-srv` or monolith with `MICROMEGAS_UNSTAMPED_AUDIENCE` still set now refuses to
   start where it previously came up.

On the auth-disabled branch the allowlist itself is a no-op either way — no `AuthContext`
extension is ever inserted, so the absent-extension convention supplies `ReadScope::All`, which
makes `OwnershipRewrite` a true no-op. Only the injected-provider path changes visibility in
practice; the auth-disabled path changes only in (2).

## Implementation Steps

1. `rust/public/src/servers/flight_sql_server.rs`: add `resolve_isolation_config`, with the doc
   comment above.
2. Same file: call it at the top of `build_and_serve`, before the injected-lakehouse branch.
3. Same file: shrink `AuthAndDefaults` to two elements, update its doc comment, drop the
   `IsolationConfig` from each of the three branch tails, and delete the `unwrap_or` line at
   `:365`.
4. Same file: rewrite `with_isolation_config`'s doc comment to name the one default.
5. Same file: add `#[cfg(test)] mod tests` covering the resolution (see Testing Strategy).
6. `rust/monolith/src/main.rs`: gate `analytics_isolation_config` on `roles.flightsql` alone.
7. `CHANGELOG.md`: an **Analytics** (or **Auth**) bullet under `## Unreleased` covering the fix
   and both behaviour changes from the Design section.
8. `cargo fmt`, `cargo clippy --workspace --all-targets`, `cargo test -p micromegas --features server`.

## Files to Modify

- `rust/public/src/servers/flight_sql_server.rs` — the hoist, the doc comments, the unit tests
- `rust/monolith/src/main.rs` — drop the `!args.disable_auth` gate
- `CHANGELOG.md` — behaviour-change entry

## Trade-offs

**Hoist vs. adding the missing `from_env` call to the two branches.** Three copies of one value
is what produced the bug; a third correct copy leaves the next branch (or the next embedder) free
to reintroduce it. Removing the dimension is the open/closed answer — there is nothing left to
get wrong per branch.

**Free function vs. inlining the `match` at the call site.** Two lines inlined would be
marginally simpler to read, but the issue's required coverage (env honoured, unset, malformed,
explicit-wins) is not reachable from an integration test — `build_and_serve` needs a live lake —
and is not worth a live-DB test under the project's testing rule. A private function makes all
four cases a plain unit test.

## Decisions

- Resolve at the top of `build_and_serve` rather than just before the auth branch: fail-fast on a
  typo before the Postgres/object-store setup, at no cost.
- Monolith's `analytics_isolation_config` loses `!args.disable_auth` but `analytics_read_policy`
  keeps it — the read policy has a real per-branch dependency (the DB grant store), the isolation
  config does not.
- Accept the visibility widening in change (1) as the price of fixing the bug, documented in
  `CHANGELOG.md` so an operator can audit the value pre-upgrade.
- Skip the env read when `with_isolation_config` was set — same reasoning as the injected-provider
  `ReadPolicy` default at `:293`.

## Documentation

No `mkdocs/` change is required: `mkdocs/docs/admin/flight-sql.md:33`,
`mkdocs/docs/admin/monolith.md:51`, and `mkdocs/docs/admin/authorization.md:22` all document the
variable with no auth-branch precondition, which the fix makes true. Verify during
implementation that no other page has since acquired a branch-conditional claim
(`grep -rn PUBLIC_VIEW_SETS mkdocs/`).

## Testing Strategy

`#[cfg(test)] mod tests` in `rust/public/src/servers/flight_sql_server.rs` over
`resolve_isolation_config` — the function is private to the builder, so no integration test can
reach it. This will be the first unit-test module in `rust/public/src`; `serial_test` is already
a dev-dependency (`rust/public/Cargo.toml:92`). The `servers` module, and this test module with
it, only compiles under the non-default `server` feature, so `cargo test -p micromegas` alone
builds zero of these tests silently — use `cargo test -p micromegas --features server`.

Every test mutates process-wide env, so each is `#[serial]` with an `EnvGuard` that clears both
`MICROMEGAS_PUBLIC_VIEW_SETS` and `MICROMEGAS_UNSTAMPED_AUDIENCE` on drop — the same pattern as
`rust/analytics/tests/ownership_rewrite_config_tests.rs`. `UNSTAMPED_AUDIENCE` needs clearing
too: `from_env` refuses it outright, so an ambient value would fail every case for the wrong
reason. Cargo runs each test binary as its own process, so the analytics test file's use of the
same unprefixed variables cannot leak in.

Cases, mirroring the issue:

1. env set, `with_isolation_config` never called → the allowlist is parsed from the environment.
2. env unset → empty `public_view_sets`.
3. env malformed (e.g. `["a"]`) → `Err`.
4. explicit config supplied → it wins; with the env simultaneously set to a *different*
   value, and separately to a *malformed* value, to pin that the env is not read at all on that
   path (the property the monolith depends on).

The hoist removes the auth-branch dimension, so no case needs repeating per branch — that is
precisely what makes these four sufficient.

No new live-DB test: nothing here reproduces a bug witnessed in the wild against a real database,
and the wiring change is covered by the manual step below.

## Manual Verification

The visibility change is not reachable from any in-repo binary (see Current State), so there is
nothing to check by hand for it — an out-of-repo embedder is the only caller that observes it.
What is worth one pass by hand is the fail-fast that this change newly turns on for the
`--disable-auth` paths, plus a smoke check that the two servers still start once the resolution
moved. Not automated because both require standing up a real lake and object store; breakage
would be immediately obvious on the next run rather than silent.

1. Malformed value now refuses to start, where it previously came up:
   ```
   cd rust && MICROMEGAS_PUBLIC_VIEW_SETS='["log_entries"]' cargo run --bin micromegas-monolith -- \
     --roles all --listen-endpoint-http 127.0.0.1:9000 --disable-auth
   ```
   Expected: startup fails with the "comma-separated, not a JSON array like MICROMEGAS_API_KEYS"
   error.
2. Same check on the split binary, which takes the builder's own auth-disabled branch and does go
   through `build_and_serve`:
   `MICROMEGAS_PUBLIC_VIEW_SETS='["log_entries"]' cargo run --bin flight-sql-srv -- --disable-auth`.
   Expected: the same failure, before any lake connection is attempted (no Postgres/object-store
   log lines first).
3. Smoke test both binaries with auth disabled — `python3 local_test_env/ai_scripts/start_services.py`
   and `--monolith` (both default to `--disable-auth`) — with a valid
   `MICROMEGAS_PUBLIC_VIEW_SETS=log_entries`. Expected: both come up, and
   `micromegas-query "SELECT count(*) FROM log_entries" --begin 1h` returns rows. This does not
   reach `with_default_auth` or the injected-provider branch; reaching those needs extra setup:
   `flight-sql-srv` without `--disable-auth` plus `MICROMEGAS_API_KEYS` set (for
   `with_default_auth`), or `--monolith` with `MICROMEGAS_OIDC_CONFIG` /
   `MICROMEGAS_ANALYTICS_OIDC_CONFIG` set (for the injected-provider branch).
