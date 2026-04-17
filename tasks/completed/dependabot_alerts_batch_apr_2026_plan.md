# Dependabot Alerts Batch (Apr 2026) Plan

## Overview
Address the 6 open Dependabot alerts on https://github.com/madesroches/micromegas/security/dependabot. Four are straightforward lockfile/dependency bumps; one (`rand 0.8.5`, #201) is blocked on upstream crates and must be left open per project policy.

## Current State (alerts as of 2026-04-17)

| # | Severity | Package | Ecosystem | Manifest | Vulnerable | Patched | GHSA |
|---|----------|---------|-----------|----------|-----------|---------|------|
| 209 | **critical** | `protobufjs` | npm | `yarn.lock` | `< 7.5.5` (present: `7.5.4`) | `7.5.5` | GHSA-xq3m-2v4x-88gg |
| 208 | medium | `authlib` | pip | `python/micromegas/poetry.lock` | `< 1.6.11` (present: `1.6.9`) | `1.6.11` | GHSA-jj8c-mmj3-mmgv |
| 207 | low | `rustls-webpki` | rust | `rust/datafusion-wasm/Cargo.lock` | `>= 0.101.0, < 0.103.12` (present: `0.103.10`) | `0.103.12` | GHSA-xgp8-3hg3-c2mh |
| 206 | low | `rustls-webpki` | rust | `rust/datafusion-wasm/Cargo.lock` | same | same | GHSA-965h-392x-2mh5 |
| 205 | medium | `protocol-buffers-schema` | npm | `yarn.lock` | `< 3.6.1` (present: `3.6.0`) | `3.6.1` | GHSA-j452-xhg8-qg39 |
| 201 | low | `rand` | rust | `rust/Cargo.lock` | `>= 0.7.0, < 0.9.3` (present: `0.8.5` *and* `0.9.4`) | `0.9.3` | GHSA-cq8v-f236-94qc |

### Dependency-graph notes

**#209 protobufjs** — transitive via:
- `@opentelemetry/otlp-transformer@^0.202.0` → `@grafana/faro-core@1.19.0` → `@grafana/faro-web-sdk@1.19.0` (pulled in by `grafana/` workspace).
- Direct dep is `^7.3.0`; `7.5.5` satisfies the range, so a top-level `resolutions` override works.

**#205 protocol-buffers-schema** — transitive via:
- `resolve-protobuf-schema@2.1.0` → `pbf@3.2.1` → `ol@7.4.0` (OpenLayers, pulled in by `grafana/` workspace).
- Direct dep is `^3.3.1`; `3.6.1` satisfies, so a `resolutions` override works.

**#208 authlib** — direct dep in `python/micromegas/pyproject.toml` as `authlib = "^1.6.7"`. `1.6.11` satisfies the caret, a `poetry update authlib` bumps it.

**#206 / #207 rustls-webpki** — transitive via `rustls@0.23.37 → reqwest@0.12.28 → micromegas-telemetry-sink → datafusion-wasm`. `cargo update -p rustls-webpki` within `rust/datafusion-wasm/` moves it from `0.103.10` → `≥0.103.12` (same minor series; no API change).

**#201 rand** — the workspace dep was already bumped to `0.9` in commit `7b1915cfd` (PR #1010), which is why `rand 0.9.4` appears in `rust/Cargo.lock`. The still-vulnerable `rand 0.8.5` is pulled in *only* transitively via:
- `jsonwebtoken 10.3.0`
- `oauth2 5.0.0` / `openidconnect 4.0.1`
- `rsa 0.9.10` (through `jsonwebtoken`, `openidconnect`)
- `sqlx-postgres 0.8.6`
- `rust-multipart-rfc7578_2 0.6.1` (dev-dep of `axum-test`)

The latest **stable** releases of all of these still depend on `rand 0.8`. Only pre-release tracks have moved (`sqlx 0.9.0-alpha.1`, `rsa 0.10.0-rc.17`). Consequently this alert is **not currently fixable**; it must remain open per the project rule "NEVER dismiss Dependabot alerts — leave them open until fixed by code/dependency changes" (CLAUDE.md).

## Design

### JS resolutions (alerts #209, #205)
Add pins to the root `package.json` `resolutions` block (yarn v1 honors root-level resolutions across workspaces). Existing entries already use this pattern (`dompurify`, `qs`, `serialize-javascript`, etc.), so this matches the established convention.

```jsonc
"resolutions": {
  // ...existing...
  "protobufjs": "^7.5.5",
  "protocol-buffers-schema": "^3.6.1"
}
```

Then run `yarn install` at the repo root to regenerate `yarn.lock`.

### Python bump (alert #208)
From `python/micromegas/`:
```
poetry update authlib
```
The caret constraint `^1.6.7` already permits `1.6.11`; only the lockfile needs to change. No code changes expected — the GHSA (CSRF when using cache) is a server-side fix in authlib's CSRF token validation path, API-compatible.

### Rust wasm crate (alerts #206, #207)
From `rust/datafusion-wasm/`:
```
cargo update -p rustls-webpki
```
`0.103.10` → `0.103.12` is a patch bump in the 0.103.x series; no code change expected. Verify with `cargo build --target wasm32-unknown-unknown` (or whatever target the crate normally builds for) and `cargo test -p micromegas-datafusion-wasm`.

### Rand alert (#201) — no-op, document and leave open
- Do not dismiss the alert.
- Add a short note in the PR description explaining that `rand 0.8.5` persists only as a transitive dep and that no stable upstream release has migrated to `rand 0.9` yet.
- Track the upstream releases that would let us close it:
  - `sqlx` 0.9.x stable
  - `jsonwebtoken` next release bumping `rand`
  - `rsa` 0.10.x stable
  - `oauth2`/`openidconnect` next releases
- Revisit when Dependabot opens a follow-up PR or when one of those crates ships a stable release.

## Implementation Steps

Group into three independent PRs so each ecosystem's fix can land without waiting on the others. Alternatively, a single batched PR is acceptable — the recent commit history (#1007, #1009, #1010, #1011) uses both styles.

Recommendation: **one batched PR** titled e.g. `Fix Dependabot alerts #205, #206, #207, #208, #209` — matches the cadence of `923d6c61a` and keeps the changelog tidy.

1. **JS (alerts #209, #205)**
   - Edit `package.json`: add `"protobufjs": "^7.5.5"` and `"protocol-buffers-schema": "^3.6.1"` to `resolutions` (keep alphabetical).
   - Run `yarn install` at repo root.
   - Verify `yarn.lock` now has `protobufjs 7.5.5+` and `protocol-buffers-schema 3.6.1+` (no other entries remain at vulnerable versions).
   - Run `yarn lint` in `grafana/` and `analytics-web-app/` (both pull in npm deps); run `yarn build` in `grafana/` to confirm the Faro SDK / OpenLayers chains still resolve.

2. **Python (alert #208)**
   - From `python/micromegas/`: `poetry update authlib`.
   - Verify `poetry.lock` shows `authlib 1.6.11+`.
   - Run `poetry run pytest` to confirm auth flows still work.
   - `poetry run black .` before commit.

3. **Rust wasm (alerts #206, #207)**
   - From `rust/datafusion-wasm/`: `cargo update -p rustls-webpki`.
   - Verify `rust/datafusion-wasm/Cargo.lock` shows `rustls-webpki 0.103.12+`.
   - Run `cargo build` and `cargo test -p micromegas-datafusion-wasm` (from `rust/`).
   - Run `cargo fmt` (no code change expected, but per project rule).

4. **Alert #201 (rand) — no code change**
   - Do not touch. Mention in PR description that it remains open pending upstream.

5. **Verify & PR**
   - `git status` + `git diff` to confirm only lockfiles (+ root `package.json`) changed.
   - `git log --oneline main..HEAD` before `gh pr create` per `CLAUDE.md`.
   - PR body lists the five alerts addressed and explicitly notes #201 is pending upstream, so a reviewer knows it was considered and not overlooked.

## Files to Modify
- `package.json` — add two `resolutions` entries.
- `yarn.lock` — regenerated.
- `python/micromegas/poetry.lock` — regenerated.
- `rust/datafusion-wasm/Cargo.lock` — regenerated.

No source-code files should change.

## Trade-offs

- **One batched PR vs. per-ecosystem PRs.** Batching is faster to review and matches recent precedent (`923d6c61a`, #1007 bundled 3 alerts). Splitting by ecosystem isolates blast radius if one of the upgrades breaks a build. Batching wins here because all four are pure lockfile bumps with no cross-ecosystem coupling; a CI failure in one file is easy to isolate.
- **Yarn `resolutions` vs. upgrading the parent deps (`@grafana/faro-web-sdk`, `ol`).** Bumping the parents would pull the safe versions naturally, but also drags in unrelated changes (Faro and OpenLayers minor bumps touch many files). Resolutions keep the diff minimal and scoped to the security fix.
- **Waiting on upstream for `rand` vs. forking / patching.** Patching via `[patch.crates-io]` entries for 5 transitive crates is invasive and breaks when those crates release. The unsoundness (GHSA-cq8v-f236-94qc) requires a *custom logger calling `rand::rng()`* — we do not use one, so the practical exposure is zero. Leaving it open and tracked is the right call.

## Documentation
No docs change. Commit / PR body is the only written artifact.

## Testing Strategy
- **JS**: `yarn install` at root; then in each touched workspace run `yarn lint` (and `yarn build` in `grafana/`, `yarn type-check` + `yarn build` in `analytics-web-app/`) to confirm nothing broke in the OpenLayers / Faro chains.
- **Python**: `poetry run pytest` from `python/micromegas/`. Smoke-test `micromegas-query` if a live env is handy; CSRF fix is server-side in authlib and does not affect our client usage.
- **Rust (wasm)**: `cargo build` + `cargo test -p micromegas-datafusion-wasm` from `rust/`. Also run `python3 ../build/rust_ci.py` from `rust/` as a final gate.
- **Full CI**: let GitHub Actions run; Dependabot should automatically close #205, #206, #207, #208, #209 once the merged `main` is scanned.

## Open Questions
- Should #201 get a short note added to the repo (e.g., in a `SECURITY.md` or a `tasks/` stub) so future maintainers don't re-investigate? **Recommendation:** no — the PR description is sufficient; Dependabot itself is the source of truth. But flag it for the user in case they want a durable note.
- Batched PR vs. split? **Default to batched** unless the user says otherwise.
