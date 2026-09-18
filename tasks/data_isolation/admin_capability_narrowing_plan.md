# Narrowing `is_admin` to Grant Administration Plan

## Overview

`is_admin` today carries two unrelated authorities bundled together: the ability to *administer*
the grant/membership store, and an implicit, unrecorded grant on *every* audience for data writes
and for the key-management surface. This plan separates them. After it, `is_admin` means exactly
"may administer audience grants and group memberships"; obtaining or exercising data access —
minting a write credential for an audience, or importing one — requires an explicit grant row
naming the caller, admin or not. An admin who needs
that access still grants it to themselves in one call, but the grant is now a durable,
attributable, revocable row rather than an invisible property of their group membership.

This is an **audit** control, not a reduction of admin power: `create_grant` lets an admin grant
themselves anything unconditionally (unchanged), so nothing here defends against a malicious
admin. What changes is that every admin data access leaves a trace in `audience_grants`
(`created_by`, `created_at`), visible through `GET .../audience-grants/visible` and
`list_audience_grants()`, and removable by anyone administering the store. Resolves #1598.

## Current State

### The decided part

The ordinary query path has no admin bypass (`audience_based_access_control_plan.md` §5, "No
human-admin query-path bypass"): `is_admin` never maps to `ReadScope::All`, and an admin's
FlightSQL session is audience-filtered like any other caller's. `CallerContext.is_admin`
(`rust/analytics/src/lakehouse/read_scope.rs:54`) is threaded at four call sites, not one: the
mutating-function registration gate (an integrity control), `list_audience_grants`'s
`GrantVisibility::All` branch, `AudienceGuard::global_rows_visible`'s `lakehouse_admin` arm
(`rust/analytics/src/lakehouse/audience_guard.rs:463-469`), and `authorize_view_ddl`'s gate on
`CREATE/DROP MATERIALIZED VIEW` (`rust/public/src/servers/view_ddl.rs:49-56`).

### The two bundles

**Control plane** — administering who may access what:

| Site | Admin behavior today |
|---|---|
| `audience_grants.rs:267` (`GrantGate`) | exempt from `MICROMEGAS_SELF_SERVICE_MINT` |
| `audience_grants.rs:416` (`create_grant`) | bypasses all four non-admin checks, `*` selectors included |
| `audience_grants.rs:589,638` (`delete_grant`) | deletes any row |
| `audience_grants.rs:715` (`visible_grants`) | sees every row |
| `groups.rs` (six handlers) | `AdminUser`-gated group/membership CRUD |
| `list_audience_grants_table_function.rs` | sees every row (`GrantVisibility`) |

**Data plane** — obtaining or exercising access to telemetry:

| Site | Admin behavior today |
|---|---|
| `rust/auth/src/policy.rs:553-561` (`AudienceMintPolicy`) | may mint **any** valid audience, `public` and the deployment default included, with no grant row |
| `ingestion_keys.rs:411-436` (`mint_key`) | server-side claim pre-check; the policy's admin arm always resolves `Ok`, so an admin never reaches the shared claim path |
| `ingestion_keys.rs:614,643,683,710` (`try_claim_and_mint`) | four `if caller.is_admin() { insert_key(…) }` fallbacks: lock contention, over-long selector, audience already owned, reserved name — each mints anyway instead of erroring |
| `ingestion_keys.rs:820` (`list_keys`) | every `ingestion_api_keys` row, every audience — left as-is, see Decisions |
| `ingestion_keys.rs:886` (`revoke_key`) | revokes any key in any audience — left as-is, see Decisions |
| `ingestion_keys.rs:964` (`import_key`) | imports a key bound to any audience |
| `audience_grants.rs:881` (`my_audiences`) | `held_pairs` forced empty, because the client uses `isAdmin` as a blanket "you may mint/share here" |
| `rust/analytics/src/lakehouse/audience_guard.rs:463-469` (`AudienceGuard::global_rows_visible`) | an audience-scoped admin with no grant sees `'global'` partition rows via `list_partitions()`, purely on `lakehouse_admin` |
| `rust/public/src/servers/view_ddl.rs:49` (`authorize_view_ddl`) | gates `CREATE/DROP MATERIALIZED VIEW`; the resulting view's queries then run under `CallerContext::maintenance()` (`ReadScope::All`) — a read bypass, strictly stronger than any row above |

Quota exemptions (`ingestion_keys.rs:350` `max_keys_per_caller`, `:733` `max_claims_per_caller`,
`audience_grants.rs` `max_grants_per_caller`) are a third, separate thing: anti-abuse bounds on
self-service, not access.

### Relevant invariants

- Schema v9 seeds `('public','read','*')` and `('public','mint','*')`
  (`rust/ingestion/src/sql_migration.rs:302`). A deployment with a custom
  `MICROMEGAS_DEFAULT_AUDIENCE` gets **only** the literal `public` rows.
- `caller_selectors` (`policy.rs:121`) returns `["*", "user:<email>", "group:<g>"…]`.
  `selector_matches` honors `*`; `caller_holds_pair` (`audience_grants.rs:362`) strips it, because
  *delegation* must not flow from a wildcard.
- `analytics_api_keys` has no `audience` column (`sql_migration.rs:148`) — analytics keys
  authenticate a principal and inherit that principal's `ReadScope`. There is no audience
  authority to scope, so `analytics_keys.rs` is untouched by this plan.
- `ingestion_api_keys.audience` is immutable once written (migration at `sql_migration.rs:171`).
- `bulk_ingest` (`flight_sql_service_impl.rs:1511`) writes the `audience` column verbatim under an
  `is_admin` gate — see Decisions.

## Design

### The rule

> A route's authorization is the authority that route's own effect requires.
> Administering the grant/membership store requires `is_admin`. Anything that yields, exercises,
> or exposes access to an audience's data requires a grant on that audience — `is_admin` confers
> none.

### 1. `AudienceMintPolicy`: delete the admin arm

`rust/auth/src/policy.rs`. Remove the `if caller.is_admin() { … }` block from
`resolve_audience`, and hoist its format validation out of it so it runs for **every** caller:

```rust
let Some(aud) = requested else {
    return Err(anyhow!("no audience requested and none can be defaulted"));
};
if !is_valid_audience(aud) {
    return Err(anyhow!("malformed audience {aud:?}: must match [A-Za-z0-9_-]{{1,255}}"));
}
// …unchanged static-map + store selector check…
```

Every caller now reaches the same selector check. The type doc's asymmetry paragraph
("mint is an integrity decision… the two axes are allowed to disagree") is deleted and replaced
with the rule above — read and mint no longer disagree. The type doc's opening sentence
(`:499`, "The shipped `MintPolicy`. **Non-admin callers** may mint only an audience in their
**mint** set") also goes stale — once the admin arm is gone, every caller, admin included, is
bound by that sentence — so rewrite it to drop "Non-admin callers" in favor of describing every
caller.

### 2. One mint-authorization helper, shared by mint and import

`ingestion_keys.rs` currently inlines the point query → `AudienceGrants::from_rows` →
`AudienceMintPolicy::new(…).resolve_audience(…)` sequence inside `mint_key`. Extract it, so
`import_key` authorizes by the same code rather than a second, drifting copy:

```rust
/// Resolves whether `caller` may mint into `audience`, by a fresh point query against
/// `audience_grants` (never a cached snapshot). The single mint-authority seam: `mint_key` and
/// `import_key` both go through it.
async fn authorize_mint(
    pool: &PgPool,
    caller: &AuthContext,
    audience: &str,
) -> Result<(), IngestionKeyError>
```

Error mapping is today's: a failed query maps to `Unavailable` (a DB outage must never render as a
denial); a policy denial maps to `Forbidden`.

### 3. `mint_key`: one claim path for every caller

Delete the admin pre-check block (`:411-436`) and the `Err(e) if caller.is_admin()` arm. What
remains is the path non-admins already take. Only `Forbidden` falls through to the claim path;
every other variant (e.g. `Unavailable` on a DB outage) propagates unchanged — a store outage must
never be misattributed as "you have no grant":

```
authorize_mint(pool, caller, candidate)
├─ Ok                    → ordinary INSERT (insert_key, claimed: false)
├─ Err(Forbidden)
│  ├─ explicit audience + caller has an email → try_claim_and_mint  (writes mint+read
│  │                                             user:<email> rows, claimed: true)
│  └─ otherwise → 403
└─ Err(_)                → propagate unchanged
```

An admin naming a brand-new audience still claims it, by the same transaction and with the same
`created_by` audit trail — the behavior the docs already describe for an admin, now produced by
the shared path instead of a parallel one. An admin naming an *existing* audience they hold no
grant on is now denied instead of minting silently.

### 4. `try_claim_and_mint`: delete the four admin fallbacks

`:614`, `:643`, `:683`, `:710`. Each currently swallows a claim failure and mints anyway for an
admin. All four become the non-admin error unconditionally: `409 CLAIM_CONTENDED` on lock
contention, `403` on an over-long selector, `403` on an already-owned audience, `403` on a
reserved name. `insert_key`'s doc comment loses its "every 'the in-lock recheck disagreed'
branch falls through to this for an admin" paragraph; it becomes the ordinary-path insert only.
`try_claim_and_mint`'s own doc comment loses two paragraphs that go false once §3 routes every
caller through the one shared claim path, leaving a single call site that is not non-admin-only:
its lead paragraph (`:556-559`, "Reached only from `mint_key`, only for a **non-admin** caller
who explicitly named `audience`…") and its "**Admin mode never turns a claim attempt into a
mint failure.**" paragraph (`:572-580`). Four further inline/doc comments that assume an admin
pre-check ran moments earlier are deleted or rewritten, since none of the claims they make hold
once §3 removes the pre-check: the "Both the admin and non-admin call sites only ever call this
function when `caller.email` is `Some`" comment at `:591-592` (there is now one call site, not
two), the "an admin minted straight through `AudienceMintPolicy`'s `is_admin` arm" comment at
`:662-667` loses that clause, the `EXISTS`-was-already-run note at `:675-681` (its "for an admin,
`mint_key`'s own pre-check ran the identical `EXISTS` query" branch) is deleted, and the
reserved-name-is-unreachable-for-an-admin note at `:701-707` is deleted.

Also update the two `IngestionKeyError` doc comments that go stale once `import_key` narrows
(§7): the enum doc (`:99-106`) drops `import_key` from its "`list_keys`/`revoke_key`/`import_key`
stay `AdminUser`-gated and never construct any of the four" claim — `import_key` now constructs
`Forbidden`, and `Unavailable` via `authorize_mint`, while `list_keys` and `revoke_key` still
construct none — and the `Forbidden` variant doc
(`:118-122`) drops "a malformed audience from an admin" from its enumerated causes, since §1
deletes the admin arm that produced it. `MintResponse::claimed`'s doc comment (`:276-281`) also
goes stale here: it says `claimed` is `true` only for "an admin caller" minting into a brand-new
audience, but once §3 removes the admin pre-check, admin and non-admin claim through the one
shared path, so rewrite it to describe the shared claim path instead of singling out an admin.

The two quota exemptions (`:733` `max_claims_per_caller`, `:350` `max_keys_per_caller`) stay —
they bound self-service abuse, not access, and an administrator bulk-provisioning credentials is
the case they were written to exempt.

### 5. `list_keys` and `revoke_key`: unchanged

Both stay `AdminUser`-gated and unconditional — see `## Decisions`.

### 7. `import_key`: same authority as minting

Additionally extract `AuthenticatedUser(caller): AuthenticatedUser` alongside `AdminUser`:
`authorize_mint` takes `&AuthContext`, which `AdminUser`'s `ValidatedUser` does not carry. Call
`authorize_mint` on the resolved audience before the `INSERT … ON CONFLICT`; `403` on denial. No
lazy-claim path — see Decisions.

**Pre-import grants.** This also binds `micromegas-import-keys`, the bulk env-keyring migration
tool: it needs a `mint` grant per distinct audience it is about to write, not just an admin OIDC
identity. Before §7 ships, enumerate every distinct `"audience"` value in the keyring being
imported, plus any `--audience AUD` passed to the tool and the deployment default audience for
entries carrying none, and create a `mint`-axis row for the importing principal on each — or the
import fails per entry.

### 8. `my_audiences`: populate `held_pairs` for admins

`audience_grants.rs:881`. Drop the `if caller.is_admin() { Vec::new() }` shortcut and run the
held-pairs query for every caller. `is_admin` stays on the response — the client still needs it
for the grant-administration affordances (Share anywhere, delete any row), which are unchanged.
`held_pairs` has two client consumers once it is populated for admins: the CLI's `personal`
filter (step 10), and `MintIngestionKeyDialog`'s default-audience preselect
(`analytics-web-app/src/components/MintIngestionKeyDialog.tsx:57`), which already reads
`held_pairs` to prefer an audience the caller personally holds a mint grant on over
`audiences[0]` — an admin's preselected audience changes once this step ships.

### 9. Startup warning for a custom default audience

`web_server.rs`, at `IngestionKeysState` construction: when `state.default_audience` is neither
`public` nor covered by a `mint` grant row, log a `warn!`. A custom-default deployment previously
relied on the admin bypass for its very first mint; without the warning the resulting `403`
("audience {candidate:?} is not in the caller's mintable set", since a mint with no explicit
audience never reaches the claim path) reads as a bug rather than a missing one-time grant.

The check is skipped entirely — same as the `public` short-circuit — when `analytics_keys_pool` is
`None` (`MICROMEGAS_SQL_CONNECTION_STRING` unset); the service already starts in that case with
the key routes degraded to 503. Any query error (the telemetry DB owning `audience_grants` is
never migrated by this service's own startup) is logged and ignored, not propagated: this is a
best-effort diagnostic, not a startup precondition.

## Implementation Steps

**Phase 1 — the policy (no DB, self-contained)**

1. `rust/auth/src/policy.rs`: delete `AudienceMintPolicy::resolve_audience`'s admin arm, hoist
   `is_valid_audience` to run for every caller, rewrite the type doc's asymmetry paragraph and its
   opening "Non-admin callers" sentence (`:499`).
2. `rust/auth/tests/policy_tests.rs`: admin-with-no-grant is denied; admin-with-a-`mint`-grant is
   allowed; admin-via-`*` is allowed; malformed audience is rejected for admin and non-admin
   alike. Fold `mint_policy_admin_may_mint_any_valid_audience_including_public` (`:279-289`) into
   the new "admin, no grant → `Err`" case, deleting it — it asserts exactly the admin arm this
   step removes and would otherwise fail. Rename `mint_policy_admin_arm_rejects_a_malformed_audience`
   (`:291-299`) to drop "admin arm" from its name, since it keeps passing but the arm it names is
   gone.

**Phase 2 — ingestion keys**

3. `ingestion_keys.rs`: extract `authorize_mint`; rewrite `mint_key` to call it and delete the
   admin pre-check and the `Err(e) if caller.is_admin()` arm.
4. `ingestion_keys.rs`: delete the four `try_claim_and_mint` admin fallbacks; trim `insert_key`'s
   doc comment; rewrite `try_claim_and_mint`'s own doc comment — its lead paragraph (`:556-559`)
   and its "Admin mode never turns…" paragraph (`:572-580`) — and its four inline admin-pre-check
   references (`:591-592`, `:662-667`, `:675-681`, `:696-701`); rewrite `IngestionKeyError`'s enum
   doc (`:99-106`) and its `Forbidden` variant doc (`:118-122`).
5. `ingestion_keys.rs`: add `AuthenticatedUser(caller): AuthenticatedUser` alongside `AdminUser`
   in `import_key`; call `authorize_mint` from `import_key`.
6. `web_server.rs`: the missing-`mint`-grant startup `warn!` for a non-`public` default audience.

**Phase 3 — grants surface**

7. `audience_grants.rs`: populate `held_pairs` for admins; update `MyAudiencesResponse`'s field
   doc and `my_audiences`'s own doc comment. `analytics-web-app/src/lib/audience-grants-api.ts`:
   rewrite `MyAudiences.held_pairs`'s JSDoc (`:131-137`), which says `held_pairs` is "always empty
   for an admin" — false once this step populates it, and relied on by
   `MintIngestionKeyDialog:57`'s admin preselect.

**Phase 4 — clients**

8. `analytics-web-app/src/components/MintIngestionKeyDialog.tsx`: drop the `isAdmin` guard
    around the public-readability help line (`:158`, "`public` is readable by every authenticated
    user…") and the `isAdmin` guard around the claim hint (`:179`) — every caller now takes the
    same server claim path, so both apply to admins too. Keep the `!isAdmin` `mint_prefix` guard
    at `:70-71`: the prefix is a client naming convention, not a server-enforced authority, and
    this plan does not change it.
9. `analytics-web-app/src/routes/AudienceAccessPage.tsx`: the Mint button (`:793`) switches from
    `!isAdmin && showMintButton` to `(me?.audiences ?? []).includes(group.audience) &&
    showMintButton` — honoring `*` the same way the server's mint rule does; the Share/delete
    checks (`:424`, `:430`, `:498`) keep `isAdmin`.
10. `python/micromegas/micromegas/cli/setup_telemetry.py`: delete the admin special-case
    `parser.error` — `:200` and `:203-208`, keeping `:201`'s `audiences = my_audiences["audiences"]`
    (still used below) — so admins resolve through the shared `held_pairs` path; update
    `resolve_audience`'s docstring (`:154-162`).

**Phase 5 — tests and docs**

11. Rust unit tests (no DB) — see Testing Strategy.
12. Update the existing live suites' admin expectations.
13. Frontend and Python test updates.
14. Documentation and `CHANGELOG.md`.

## Files to Modify

- `rust/auth/src/policy.rs`
- `rust/auth/tests/policy_tests.rs`
- `rust/analytics-web-srv/src/ingestion_keys.rs`
- `rust/analytics-web-srv/src/audience_grants.rs`
- `rust/analytics-web-srv/src/web_server.rs`
- `rust/analytics-web-srv/tests/ingestion_keys_tests.rs`
- `rust/analytics-web-srv/tests/audience_grants_tests.rs`
- `analytics-web-app/src/components/MintIngestionKeyDialog.tsx`
- `analytics-web-app/src/routes/AudienceAccessPage.tsx`
- `analytics-web-app/src/lib/audience-grants-api.ts`
- `analytics-web-app/src/components/__tests__/ApiKeysAdminPage.test.tsx`,
  `src/routes/__tests__/IngestionApiKeysPage.test.tsx`,
  `src/routes/__tests__/AudienceAccessPage.test.tsx` (as affected)
- `python/micromegas/micromegas/cli/setup_telemetry.py` + its tests
- `python/micromegas/micromegas/web_client.py`
- `mkdocs/docs/admin/authentication.md`, `mkdocs/docs/admin/authorization.md`,
  `mkdocs/docs/admin/api-keys.md`, `mkdocs/docs/query-guide/python-api.md`
- `tasks/data_isolation/audience_based_access_control_plan.md`
- `CHANGELOG.md`

## Trade-offs

**Audit-by-grant-row vs. audit-by-log-line.** The cheapest alternative is to keep every bypass and
emit a log line whenever an admin uses one. Rejected: a log line is not queryable alongside the
grants it shadows, not revocable, and not visible on the Audience Access page. The grant row is
all three, and it needs no new storage or surface.

**Delegated per-audience ownership** (an `audience-admins`-style role, two-sided authorization).
This is the ABAC plan's own "Admin surface" follow-up and the real long-term answer. Deferred: it
needs an ownership model and an approval surface, where this plan needs neither and is a
prerequisite for it either way (a delegated owner is meaningless while `is_admin` implies
everything).

**A two-person rule on admin self-grants.** The only thing that would make this a boundary rather
than an audit trail. Out of scope: it needs an approval workflow, a pending-grant state, and a
notification path.

## Decisions

- Grant listing, group/membership CRUD, and the `GrantGate`/`MintGate` knob exemptions stay
  admin-unconditional; only data-plane sites narrow.
- `*` is honored for mint/import authority, and kept stripped for
  grant delegation. The two planes are allowed to answer the wildcard question differently
  because delegation and exercise are different effects.
- `list_keys` and `revoke_key` are not narrowed: both stay `AdminUser`-gated and unconditional.
  Seeing and revoking credentials is the key-management surface an administrator is expected to
  operate; neither confers access, and scoping them to grant rows would strand pre-existing keys
  in audiences nobody holds a grant on. Only the two routes that *grant* write access, `mint_key`
  and `import_key`, narrow. This is what keeps the plan free of a visibility audit, a composed SQL
  authority predicate, and a `created_by` escape hatch.
- `import_key` gets no lazy-claim path. Import exists to migrate an *existing* operator-chosen key,
  which by definition already has a home; duplicating `try_claim_and_mint`'s advisory-lock
  transaction for it buys one convenience at the cost of a second claim implementation. An
  operator importing into a fresh audience creates the grant row first, in one call.
- Quota exemptions (`max_keys_per_caller`, `max_claims_per_caller`, `max_grants_per_caller`) stay
  admin-exempt. They bound self-service abuse, not access.
- `bulk_ingest`'s `is_admin` gate (`flight_sql_service_impl.rs:1511`) is an accepted carve-out. It
  writes the `audience` column verbatim, which by this plan's rule should need a grant — but its
  whole purpose is cross-audience replication of a lake already stamped at origin, so per-audience
  grants cannot express it, and it grants no *read*. Left as-is.
- `authorize_view_ddl`'s `is_admin` gate (`rust/public/src/servers/view_ddl.rs:49-56`) is a second
  accepted carve-out, and the stronger of the two: a DDL view's queries are planned and
  materialized under `CallerContext::maintenance()` (`ReadScope::All`), so an author sees every
  audience regardless of their own read scope. Narrowing it to a per-audience grant is a separate
  change; left as-is here.
- `AudienceGuard::global_rows_visible`'s `lakehouse_admin` arm
  (`rust/analytics/src/lakehouse/audience_guard.rs:463-469`) is a third accepted carve-out: it
  rides on the same `lakehouse_admin` boolean as the mutating-function registration gate, so a
  caller who can already `retire_partitions`/`regenerate_partitions` a global file can also see
  it — no new authority, no new knob. Left as-is here.
- `analytics_keys.rs` is out of scope: analytics keys carry no audience binding, so there is no
  audience authority to scope them by.
- Accepted behavior break: an admin can no longer mint into an existing audience they hold no
  grant on, nor into a custom `MICROMEGAS_DEFAULT_AUDIENCE` with no `mint` row. Both are one
  `create_grant` call away, and the second gets a startup warning. No pre-existing key changes
  visibility or revocability, since §5 leaves both routes alone.
- A client-credentials caller with no email cannot form a `user:` selector and so cannot claim an
  unclaimed audience; such a caller gets a `group:` mint row instead. There is no `group:`-selector
  claim path.
- Step 9's new Mint-button condition also hides the per-audience Mint button for a non-admin on
  audiences they hold no `mint` grant on (today's `!isAdmin && showMintButton` shows it on every
  visible audience group regardless); such a mint 403s server-side today, so this is accepted as
  a non-admin UI change, not just an admin one.
- A missing `mint` grant on a custom default audience warns at startup, never fails startup: it is
  a runtime-fixable DB condition (one `create_grant` call) the service may not even be able to
  observe at boot, since `analytics-web-srv` starts with `analytics_keys_pool: None` when
  `MICROMEGAS_SQL_CONNECTION_STRING` is unset and migrates only the app DB, never the telemetry DB
  that owns `audience_grants`.
- `import_key` authorizes the *requested* audience via `authorize_mint`, even on the
  already-present-key path where the write itself keeps the original binding and discards the
  request's audience: a repeat import can now 403 on an audience it will never write.

## Documentation

- **`mkdocs/docs/admin/authentication.md`** §Admin Privileges (`:561-582`): add what admin is
  *not* — no implicit `read` or `mint` on any audience; data access is always a grant row, for
  admins too. Keep the existing capability list.
- **`mkdocs/docs/admin/authorization.md`**:
  - §Self-service mint: replace the "An admin's mint claims too" bullet (`:228-230`) with the
    single shared rule; add the custom-default-audience one-time grant step.
  - §Routes table (`:293`): `held_pairs` is no longer "(empty for an admin)".
  - §Write gate (`:307`): unchanged, but state explicitly that it is unchanged *because* it is
    grant administration.
  - §`list_audience_grants()` (`:332`): unchanged; note it is the grant-administration surface,
    not a data read.
  - §Admin-gated lakehouse functions: add `bulk_ingest`, `authorize_view_ddl`, and
    `AudienceGuard::global_rows_visible` as the remaining data-plane carve-outs; correct
    `:150`'s "`list_partitions()` silently omits every row that isn't theirs, `'global'` rows
    included" for an admin caller, since `global_rows_visible`'s `lakehouse_admin` arm makes
    `'global'` rows visible to an audience-scoped admin holding no grant.
- **`mkdocs/docs/admin/api-keys.md`**: `:17-23` (what stays admin-only), `:92-100` (the gate
  description), `:102-158` (the routes table's import row and the Import paragraph: import's new
  403/503 — List and Revoke are unchanged, and those paragraphs should say so explicitly now that
  minting narrowed around them), `:212-230` (the admin-claims story collapses into the shared
  path). Extend the migration runbook (`:444-490`) to state the new authorization: the OIDC
  identity used must hold a `mint` grant on each target audience, not just admin membership — see
  §7's pre-import grants.
- **`mkdocs/docs/query-guide/python-api.md`**'s `micromegas-import-keys` section (`:943-952`) and
  **`python/micromegas/micromegas/web_client.py`**'s `import_ingestion_api_key` docstring: replace
  "admin-only"/"Requires OIDC admin access" with the grant-based authorization above.
  `list_ingestion_api_keys`'s docstring stays as it is — that route did not change. Also `web_client.py`'s `my_audiences` docstring
  (`:191-193`): replace the "meaningless for an admin, whose mint authority never depends on a
  grant row at all" parenthetical, since after §1 an admin's mint authority depends entirely on
  grant rows like everyone else's.
- **`tasks/data_isolation/audience_based_access_control_plan.md`**: generalize §5's "No
  human-admin query-path bypass" into "no human-admin data-plane bypass" and note the three
  accepted carve-outs, `bulk_ingest`, `authorize_view_ddl`, and
  `AudienceGuard::global_rows_visible`; update §"Admin surface" to record that the blanket gate is
  now scoped to the control plane, leaving delegated ownership as the remaining follow-up.
- **`CHANGELOG.md`** Unreleased, with a **Minor breaking change** clause for
  `AudienceMintPolicy::resolve_audience`'s behavior change and `import_key`'s new authorization
  check, including on a repeat import's requested audience.

## Testing Strategy

**Unit, no DB — `rust/auth/tests/policy_tests.rs`.** The core change lives here and needs nothing
else: `AudienceMintPolicy` takes a static `AudienceGrants` map, so every case is a constructed
input.

- admin, no grant on the requested audience → `Err`
- admin, `mint`/`user:<their email>` grant → `Ok`
- admin, `mint`/`*` grant → `Ok`
- admin, `read`-only grant on the audience → `Err` (a read grant still confers no mint)
- admin, malformed audience → `Err` with the malformed message (previously the admin-arm-only
  diagnostic; now reachable for every caller)
- non-admin cases → unchanged, as regression cover on the hoisted validation

**Unit, no DB — route rejections on `lazy_pool()`.** The existing 403/400/503 tests
(`mint_403_for_non_admin`, `list_403_for_non_admin`, …) keep passing unchanged; add none, since
every *new* denial needs a grant lookup and therefore a pool.

**Existing live suites (`#[ignore]`), updated not added.** These already exist and encode the old
admin behavior; they must move with it:

- `live_admin_mint_into_a_brand_new_audience_claims_it` — still claims, now via the shared path;
  assert the two `user:<admin email>` rows and `claimed: true`.
- `live_admin_mint_into_an_existing_audience_does_not_claim` → becomes "is denied with 403",
  renamed accordingly.
- `live_admin_mint_of_the_default_audience_is_never_claimed` → 403 unless a `mint` row exists.
- `live_import_is_idempotent` — the first request resolves to `public` (already covered by the
  seeded `('public','mint','*')` row), but the second request names `"team-alpha"` and now hits
  `authorize_mint("team-alpha")` — see Decisions. Before seeding a `mint` grant on `"team-alpha"`,
  assert that second request 403s with no such grant; then seed the grant and assert it succeeds,
  covering the import denial inside this test rather than adding a new one.
- `live_my_audiences_admin_gets_a_normal_response_regardless_of_knob` — delete the "always empty
  for an admin" comment and the `held_pairs.is_empty()` assertion (`:1259-1265`); this test's
  fixture (`admin_user()`, no seeded grant rows) leaves `held_pairs` at `[]` for this caller even
  after §8, so drop the `held_pairs` assertion rather than seeding new rows to make it non-empty.
- `live_visible_admin_sees_every_row` — unchanged; it is the regression guard that the control
  plane did *not* narrow.
- `live_mint_list_revoke_round_trip` — mints into `"team-alpha"` as an admin holding no grant
  there, which after §3 becomes `Forbidden` → `try_claim_and_mint`, writing grant rows its
  cleanup does not remove. Switch it to a per-run unique audience with the same
  `cleanup_audience` helper `:922` already uses, so it stops sharing `"team-alpha"` with
  `live_import_is_idempotent`.

**Frontend (vitest).** `MintIngestionKeyDialog`: an admin now sees the claim hint and the
public-readability help line, and — since `held_pairs` is now populated for admins — the
default-audience preselect now prefers the admin's personally-held mint audience over
`audiences[0]`; assert this in the admin path of `AudienceAccessPage.test.tsx`, the only place an
admin reaches `MintIngestionKeyDialog` (via `showMintButton`). This includes inverting/renaming
`AudienceAccessPage.test.tsx:261`'s existing `it('does not show the public-readability help line
in the Mint dialog for an admin', …)`, since that line is now shown for an admin. `AudienceAccessPage`:
the Mint button follows `me.audiences`, not `isAdmin`, while Share still follows `isAdmin`.
`IngestionApiKeysPage`/`ApiKeysAdminPage`: unchanged behavior.

**Python.** `setup_telemetry`'s `resolve_audience`: the admin branch is gone, so an admin with
exactly one held mint audience resolves it silently, and an admin with none gets the
same "no mintable audience" error as anyone else. Rewrite
`test_omitted_audience_admin_is_always_an_error` (`test_setup_telemetry.py:194-208`) into "an
admin with no held mint audience gets the same zero-match error as anyone else".

## Manual Verification

Each step below exercises the browser/CLI wiring that no test reaches, and each fails loudly and
immediately — a 403 on a visible button, an empty table — rather than silently.

1. Start the monolith with auth enabled and `MICROMEGAS_SELF_SERVICE_MINT` unset:
   `cargo run --bin micromegas-monolith -- --roles all --listen-endpoint-http 127.0.0.1:9000
   --frontend-dir ../analytics-web-app/dist`. Sign in as a member of `admins` who holds no grant
   rows. Expect Admin → Ingestion API Keys to list every key in every audience, exactly as
   today — the key-management views did not narrow.
2. Mint a key naming an existing audience you hold no grant on (Mint dialog → type the existing
   name into `New audience…`; the dialog's picker only lists audiences you hold, so it can never
   be selected there). Expect `403`.
3. Mint naming a brand-new audience. Expect `201` with `claimed: true`, and two new
   `user:<you>`/`mint`+`read` rows on Audience Access.
4. `micromegas-grants --url http://127.0.0.1:9000 create team-alpha mint user:<you>` as the
   admin, then mint into `team-alpha`. Expect `201` with `claimed: false`, and the key visible in
   the list.
5. `micromegas-import-keys` (or `POST .../ingestion-api-keys/import`) an existing key into an
   audience you hold no grant on. Expect `403`. Then
   `micromegas-grants --url http://127.0.0.1:9000 create <that audience> mint user:<you>` and
   retry the same import. Expect `201` with `imported: true`.
6. `micromegas-query "SELECT * FROM list_audience_grants()"` as the admin. Expect every row —
   the control plane did not narrow.
7. Restart with `MICROMEGAS_DEFAULT_AUDIENCE=corp` and no grant on `corp`. Expect the startup
   `warn!` in `/tmp/monolith.log`, and a `403` from a mint with no explicit audience.
