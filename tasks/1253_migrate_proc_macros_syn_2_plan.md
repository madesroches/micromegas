# Migrate Internal Proc-Macro Crates from syn 1.0 to 2.0 Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1253

## Overview
The workspace pins `syn = { version = "1.0", features = ["extra-traits", "full"] }` at
`rust/Cargo.toml:84`. It is consumed only by the project's own three proc-macro crates, and it is
the sole reason `Cargo.lock` carries a duplicate `syn` (`v1.0.109` **and** `v2.0.118`). Every other
dependency in the tree is already on syn 2.x. Migrating the three crates to the syn 2.0 API lets us
bump the workspace pin to `"2.0"`, which drops `syn v1.0.109` and its transitive `quote 1`/`unicode-ident`
duplication from every build, shrinking the proc-macro compile stack.

This is a dependency/API migration only — no change to macro behavior or generated code.

## Current State
`syn` is declared once, in the workspace root, and pulled in by exactly three crates:

- `rust/Cargo.toml:84` — `syn = { version = "1.0", features = ["extra-traits", "full"] }`
- `rust/tracing/proc-macros/Cargo.toml:16` — `syn.workspace = true`
- `rust/transit/derive/Cargo.toml:17` — `syn.workspace = true`
- `rust/micromegas-proc-macros/Cargo.toml:18` — `syn.workspace = true`

Verified no other crate declares `syn` (`grep -rn '^syn' --include=Cargo.toml rust/` returns only the
four lines above), and `cargo tree -i syn@1.0.109` shows the only reverse-dep chain rooting at
`micromegas-derive-transit`. So these three crates are the complete migration surface.

The three crates use syn to differing depths:

### 1. `rust/micromegas-proc-macros/src/lib.rs` — the `micromegas_main` attribute macro
**This is the only crate with hard breaking changes.** It parses attribute arguments using APIs
that were **removed** in syn 2.0:

- Line 8: `use syn::{ItemFn, Lit, Meta, NestedMeta};` — `NestedMeta` no longer exists in syn 2.0.
- Lines 82-86: parses args as `Punctuated::<NestedMeta, Token![,]>::parse_terminated`.
- Lines 114-213: a `for arg in args` loop matching `NestedMeta::Meta(Meta::NameValue(nv))` and
  reading the literal via `nv.lit` (e.g. `if let Lit::Str(lit_str) = &nv.lit`). In syn 2.0,
  `Meta::NameValue` no longer has a `lit: Lit` field — it has `value: Expr` instead.

All nine supported args (`interop_max_level`, `max_level_override`, `ctrlc_handling`,
`local_sink_enabled`, `local_sink_max_level`, `install_log_capture`, `system_metrics`,
`telemetry_url`, `api_key`) go through this same `NestedMeta` + `nv.lit` pattern, plus the
catch-all `other =>` arm (line 206) that produces the "Unsupported attribute argument" error.

The crate has a substantial unit-test suite (`#[cfg(test)] mod tests`, lines 319-420) that asserts
on **both** the expanded output strings and the exact error-message strings. These tests are the
migration's safety net and must all still pass unchanged.

### 2. `rust/tracing/proc-macros/src/lib.rs` — `span_fn` / `log_fn`
**One small breaking change.** Line 210:

```rust
if stmts.len() == 1
    && let syn::Stmt::Expr(syn::Expr::Call(call_expr)) = &stmts[0]
    && call_expr.args.len() == 1
```

In syn 2.0 the `Stmt::Expr` variant changed shape from `Stmt::Expr(Expr)` to
`Stmt::Expr(Expr, Option<Semi>)` (trailing-semicolon tracking was unified). The pattern must
become `syn::Stmt::Expr(syn::Expr::Call(call_expr), _)`.

Everything else in this file — `ItemFn`, `ReturnType`, `Type`, `TypePath`, `TypeParamBound`,
`ImplTrait`, `parse_macro_input!`, `parse_quote!`, custom `Parse` impl for `TraceArgs` — is
API-compatible with syn 2.0 and needs no change.

### 3. `rust/transit/derive/src/*.rs` — `TransitReflect` derive + `declare_queue_struct`
**Expected to need no source changes.** `derive_reflect.rs` matches over `syn::Type` (with a
wildcard arm, so the `#[non_exhaustive]` enum in 2.0 is fine), `syn::Data`, and `syn::Fields`;
`declare_queue.rs` uses `parse::<DeriveInput>` and `GenericParam::{Type,Lifetime,Const}`. All of
these are stable across the 1.0→2.0 boundary. The `{unknown_field_type:?}` Debug format in
`derive_reflect.rs:24` relies on the `extra-traits` feature, which still exists in 2.0 and is
retained (see Design). This crate is verified by a build/test pass rather than expected edits.

### Feature flags
`extra-traits` (Debug/Eq/Hash impls on syn AST nodes — needed for the `{:?}` in `derive_reflect.rs`)
and `full` (needed to parse `ItemFn` bodies) both still exist in syn 2.0 and are both still required.
The feature set stays identical.

## Design
Three edits, then a clean-tree verification of the fourth crate:

### A. Bump the workspace pin
`rust/Cargo.toml:84`:
```toml
syn = { version = "2.0", features = ["extra-traits", "full"] }
```
Keep alphabetical ordering (already correct — `syn` sits between `subtle` and `sysinfo`). Features
unchanged.

### B. Rewrite attribute parsing in `micromegas-proc-macros`
Replace the removed `NestedMeta` machinery with the syn 2.0 equivalent while preserving the existing
per-argument `match` structure, all error messages, and all generated output — so the existing unit
tests keep passing verbatim.

- **Import** (line 8): drop `NestedMeta`; add `Expr` and `ExprLit`:
  ```rust
  use syn::{Expr, ExprLit, ItemFn, Lit, Meta};
  ```
- **Parse** (lines 82-86): parse a `Punctuated<Meta, Token![,]>` instead of `Punctuated<NestedMeta, …>`.
  Every argument this macro accepts is a `name = value` pair, i.e. a `Meta::NameValue`, so the
  element type is `Meta` directly:
  ```rust
  let args: Vec<Meta> =
      syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated
          .parse2(args)?
          .into_iter()
          .collect();
  ```
- **Match arms** (lines 114-213): change each arm from `NestedMeta::Meta(Meta::NameValue(nv)) if …`
  to `Meta::NameValue(nv) if …`, and change every literal extraction from reading `nv.lit` to
  reading `nv.value` (an `Expr`) and pattern-matching the wrapped literal. Concretely, each string
  arm goes from:
  ```rust
  if let Lit::Str(lit_str) = &nv.lit { … }
  ```
  to:
  ```rust
  if let Expr::Lit(ExprLit { lit: Lit::Str(lit_str), .. }) = &nv.value { … }
  ```
  and each bool arm likewise matches `Expr::Lit(ExprLit { lit: Lit::Bool(lit_bool), .. })`. The
  `else` branches that emit `"… must be a string/bool literal"` errors, and their spanned target
  (`&nv.lit` → `&nv.value`), stay otherwise identical.
- **Catch-all** (line 206): change `other =>` from matching a `NestedMeta` to matching a `Meta`;
  `syn::Error::new_spanned(&other, "Unsupported attribute argument…")` still compiles (`Meta`
  implements `ToTokens`).

Alternative considered and rejected: `syn::meta::parser` (the syn 2.0 "recommended" attribute-arg
API) — see Trade-offs.

### C. Fix the `Stmt::Expr` pattern in `tracing/proc-macros`
`tracing/proc-macros/src/lib.rs:210`: add the trailing-token binding:
```rust
&& let syn::Stmt::Expr(syn::Expr::Call(call_expr), _) = &stmts[0]
```
The `_` ignores the `Option<Semi>`; the async-trait-generated tail expression `Box::pin(async move {…})`
is a `Stmt::Expr(Expr::Call(_), None)`, so the match still fires exactly as before.

### D. `transit/derive` — verify only
No edit anticipated. Confirm via `cargo build -p micromegas-derive-transit` and the transit test
suite. If the 2.0 compiler surfaces any incompatibility (not expected), address it in this step.

## Implementation Steps
1. **`rust/Cargo.toml`** — bump `syn` to `"2.0"` (Design §A). Do **not** hand-edit `Cargo.lock`; let
   the build regenerate it.
2. **`rust/micromegas-proc-macros/src/lib.rs`** — apply the attribute-parsing rewrite (Design §B):
   fix imports, the `parse2` element type, all nine `match` arms + catch-all, and the literal
   extraction. Leave the doc comments, builder-call generation, and `#[cfg(test)]` module untouched.
3. **`rust/tracing/proc-macros/src/lib.rs`** — add `, _` to the `Stmt::Expr` pattern (Design §C).
4. **`transit/derive`** — no expected change; verify it builds against syn 2.0 (Design §D).
5. **Regenerate the lock & confirm the dedup**: run `cargo build`, then
   `cargo tree -d 2>/dev/null | grep -A3 'syn'` (or `cargo tree -i syn@1.0.109`, which should now
   error with "package ID specification … did not match any packages") to confirm `syn v1.0.x` is
   gone and only `syn v2.0.x` remains.
6. **Full verification** — run the test/lint sequence in Testing Strategy.

## Files to Modify
- `rust/Cargo.toml` — bump `syn` pin to `"2.0"`.
- `rust/micromegas-proc-macros/src/lib.rs` — migrate attribute-arg parsing off `NestedMeta`/`nv.lit`.
- `rust/tracing/proc-macros/src/lib.rs` — update the `Stmt::Expr` match pattern.
- `Cargo.lock` — regenerated by cargo (do not hand-edit); expected to lose the `syn v1.0.109` entry.
- (`rust/transit/derive/src/*.rs` — verified, edits only if the build surfaces an incompatibility.)

## Trade-offs
- **`Punctuated<Meta>` + manual `Expr::Lit` extraction vs. `syn::meta::parser`.** syn 2.0 promotes
  `syn::meta::parser` with a closure calling `meta.value()?.parse()?` as the modern idiom for
  attribute args. It's cleaner for new code, but it would restructure the whole
  arg-dispatch loop, change how errors are produced, and risk perturbing the exact error-message
  strings the unit tests assert on. The `Punctuated<Meta>` approach is the minimal, behavior-preserving
  diff — it keeps the existing arm-per-argument structure and every error string intact, so the test
  suite is an exact regression gate. Chosen for lowest risk on a pure-migration task.
- **Adding `darling`.** A derive-based arg parser would be less code long-term but adds a dependency
  to solve a problem that's already solved inline. Out of scope for a dedup migration.
- **Not touching `transit/derive`.** We deliberately avoid speculative edits there; its syn usage is
  already 2.0-compatible, so the right action is to verify, not to rewrite.

## Documentation
None. No public API, CLI, config, env var, or user-facing behavior changes — this is an internal
dependency bump. `CHANGELOG.md` gets an entry under `## Unreleased` noting the syn 2.0 migration /
removal of the duplicate `syn 1.0` (handled by the PR-finalization step, not a docs page).

## Testing Strategy
Behavior-preservation is the whole game here; the bar is "identical macro output, zero new warnings."

1. **Proc-macro unit tests** (the primary gate): `cargo test -p micromegas-proc-macros`. The suite in
   `micromegas-proc-macros/src/lib.rs` asserts on expanded output (`with_auth_from_env`,
   `ApiKeyRequestDecorator`, `LevelFilter :: Info`, …) and on exact error strings
   (`"ctrlc_handling must be a bool literal"`, `"Unsupported attribute argument"`,
   `"Invalid level value"`, non-async / malformed-args errors). All must pass unchanged.
2. **transit derive tests**: `cargo test -p micromegas-derive-transit -p micromegas-transit`
   (covers `TransitReflect` + `declare_queue_struct` via `transit/tests/test_reflect.rs`,
   `test_queue.rs`, etc.).
3. **tracing macro tests**: `cargo test -p micromegas-tracing` (covers `span_fn`/`log_fn`, incl.
   the async-trait path in `tracing/tests/`), plus a normal `cargo build` of the workspace, which
   compiles the 60+ `span_fn` call sites and every `#[micromegas_main]` binary
   (`monolith`, `flight-sql-srv`, `telemetry-ingestion-srv`, `analytics-web-srv`, examples, …) —
   real-world expansion coverage for both attribute macros.
4. **Dedup confirmation** (acceptance criterion): `cargo tree -i syn@1.0.109` must report no match,
   and `cargo tree -d` must not list any `syn v1.0.x`.
5. **Full CI gate**: `python3 build/rust_ci.py` (fmt check + `cargo clippy --workspace -- -D warnings`
   + tests). Must be clean — the acceptance criteria explicitly require clippy with `-D warnings`.

## Open Questions
None — scope, surface, and API deltas are fully determined by the issue and confirmed against the
codebase.
