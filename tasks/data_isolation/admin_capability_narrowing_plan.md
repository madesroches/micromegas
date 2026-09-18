# Narrowing `is_admin` to Grant Administration Plan

## Overview

`is_admin` today carries two unrelated authorities bundled together: the ability to *administer*
the grant/membership store, and an implicit, unrecorded grant on *every* audience for data writes
and for the key-management surface. This plan separates them. After it, `is_admin` means exactly
"may administer audience grants and group memberships"; obtaining or exercising data access —
minting a write credential for an audience, importing one, seeing or revoking the credentials of
an audience — requires an explicit grant row naming the caller, admin or not. An admin who needs
that access still grants it to themselves in one call, but the grant is now a durable,
attributable, revocable row rather than an invisible property of their group membership.

This is an **audit** control, not a reduction of admin power: `create_grant` lets an admin grant
themselves anything unconditionally (unchanged), so nothing here defends against a malicious
admin. What changes is that every admin data access leaves a trace in `audience_grants`
(`created_by`, `created_at`), visible through `GET .../audience-grants/visible` and
`list_audience_grants()`, and removable by anyone administering the store. Resolves #1598.

## Current State

### The decided part

The query path already has no admin bypass (`audience_based_access_control_plan.md` §5, "No
human-admin query-path bypass"): `is_admin` never maps to `ReadScope::All`, and an admin's
FlightSQL session is audience-filtered like any other caller's. `CallerContext.is_admin`
(`rust/analytics/src/lakehouse/read_scope.rs:54`) is threaded only for the mutating-function
registration gate, an integrity control.

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
| `ingestion_keys.rs:820` (`list_keys`) | every `ingestion_api_keys` row, every audience |
| `ingestion_keys.rs:886` (`revoke_key`) | revokes any key in any audience |
| `ingestion_keys.rs:964` (`import_key`) | imports a key bound to any audience |
| `audience_grants.rs:881` (`my_audiences`) | `held_pairs` forced empty, because the client uses `isAdmin` as a blanket "you may mint/share here" |

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

Applied to the wildcard question, which the two planes answer differently:

- **Delegation** (create/delete a grant row) keeps `*` stripped: holding an audience only via a
  wildcard row must not let you re-share it. Unchanged.
- **Mint authority** (mint, import, revoke a credential) honors `*`, exactly as
  `AudienceMintPolicy`/`selector_matches` already do. Stripping it here would make `public` —
  seeded `('public','mint','*')` precisely so it ships mintable — unmintable and unimportable for
  *everyone*, which contradicts the shipped seed rather than narrowing admin.
- **Visibility** (list keys) honors `*` for the same reason: keys bound to `public` stay listed,
  since `public` is readable by every authenticated caller by an explicit row.

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
with the rule above — read and mint no longer disagree.

### 2. One mint-authorization helper, shared by mint and import

`ingestion_keys.rs` currently inlines the point query → `AudienceGrants::from_rows` →
`AudienceMintPolicy::new(…).resolve_audience(…)` sequence inside `mint_key`. Extract it, so
`import_key` authorizes by the same code rather than a second, drifting copy:

```rust
/// Resolves whether `caller` may mint into `audience`, by a fresh point query against
/// `audience_grants` (never a cached snapshot). The single mint-authority seam: `mint_key` and
/// `import_key` both go through it, so a key can never enter the table under one rule and be
/// listed under another.
async fn authorize_mint(
    pool: &PgPool,
    caller: &AuthContext,
    audience: &str,
) -> Result<(), IngestionKeyError>
```

Error mapping is today's: a failed query is `Unavailable` (a DB outage must never render as a
denial), a policy `Err` is the caller's to handle — `mint_key` falls through to the claim path,
`import_key` returns `Forbidden`.

### 3. `mint_key`: one claim path for every caller

Delete the admin pre-check block (`:411-436`) and the `Err(e) if caller.is_admin()` arm. What
remains is the path non-admins already take:

```
authorize_mint(pool, caller, candidate)
├─ Ok            → ordinary INSERT (insert_key, claimed: false)
└─ Err
   ├─ explicit audience + caller has an email → try_claim_and_mint  (writes mint+read
   │                                             user:<email> rows, claimed: true)
   └─ otherwise → 403
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

The two quota exemptions (`:733` `max_claims_per_caller`, `:350` `max_keys_per_caller`) stay —
they bound self-service abuse, not access, and an administrator bulk-provisioning credentials is
the case they were written to exempt.

### 5. `list_keys`: audience-scoped, still admin-gated

Keep the `AdminUser` extractor — this plan narrows admin, it does not open the route to
non-admins. Narrow the query to keys the caller has authority over, as a named constant so the
predicate is assertable without a database:

```rust
/// Visibility predicate for `list_keys`: a key row is visible to a caller who created it, or who
/// holds any-axis grant on the audience it is bound to. `*` is honored (unlike
/// `caller_holds_pair`'s delegation rule), so `public`-bound keys stay visible to every caller —
/// `public` is readable by everyone by an explicit seeded row, not by an implicit bypass.
///
/// `pub` so the shape a live database would otherwise be needed to observe can be asserted from
/// the test crate.
pub const LIST_KEYS_VISIBILITY_SQL: &str =
    "(k.created_by = $3 OR EXISTS (SELECT 1 FROM audience_grants g \
      WHERE g.audience = k.audience AND g.selector = ANY($4)))";
```

Both branches (`include_revoked` on/off) compose it into their `WHERE`. `$3` is
`caller_identity(caller)`, `$4` the full `caller_selectors(caller)`.

### 6. `revoke_key`: mint authority on the key's audience

Fold the authority into the existing single idempotent `UPDATE`, so a repeat call still preserves
the original `revoked_at`:

```sql
UPDATE ingestion_api_keys k
SET revoked_at = COALESCE(revoked_at, now()),
    revoked_by = COALESCE(revoked_by, $2)
WHERE k.key_id = $1
  AND (k.created_by = $3
       OR EXISTS (SELECT 1 FROM audience_grants g
                  WHERE g.audience = k.audience AND g.axis = 'mint'
                    AND g.selector = ANY($4)))
RETURNING revoked_at
```

On zero rows affected, disambiguate the way `delete_grant` already does, so the route is not an
existence oracle for keys in audiences the caller cannot see:

1. Not visible per `LIST_KEYS_VISIBILITY_SQL` → `404`.
2. Visible but no `mint` authority → `403` ("you hold no mint grant on this key's audience").
3. No such `key_id` → `404`.

### 7. `import_key`: same authority as minting

Call `authorize_mint` on the resolved audience before the `INSERT … ON CONFLICT`; `403` on
denial. No lazy-claim path — see Decisions.

### 8. `my_audiences`: populate `held_pairs` for admins

`audience_grants.rs:881`. Drop the `if caller.is_admin() { Vec::new() }` shortcut and run the
held-pairs query for every caller. `is_admin` stays on the response — the client still needs it
for the grant-administration affordances (Share anywhere, delete any row), which are unchanged.
`held_pairs` becomes what the client reads for the *mint* affordance.

### 9. Startup warning for a custom default audience

`web_server.rs`, at `IngestionKeysState` construction: when `state.default_audience` is neither
`public` nor covered by a `mint` grant row, log a `warn!`. A custom-default deployment previously
relied on the admin bypass for its very first mint; without the warning the resulting `403`
("audience cannot be claimed", since the default audience is reserved from claiming) reads as a
bug rather than a missing one-time grant.

### Unchanged

Every control-plane site listed in Current State stays exactly as it is. In particular
`visible_grants` and `list_audience_grants()` keep showing an admin every row: grant listing *is*
the administration surface named in the issue's own scope ("manage (create/revoke/list) audience
grants and memberships"), and narrowing it would be circular — an admin would have to grant
themselves access in order to read the table that records grants.

## Implementation Steps

**Phase 1 — the policy (no DB, self-contained)**

1. `rust/auth/src/policy.rs`: delete `AudienceMintPolicy::resolve_audience`'s admin arm, hoist
   `is_valid_audience` to run for every caller, rewrite the type doc's asymmetry paragraph.
2. `rust/auth/tests/policy_tests.rs`: admin-with-no-grant is denied; admin-with-a-`mint`-grant is
   allowed; admin-via-`*` is allowed; malformed audience is rejected for admin and non-admin
   alike.

**Phase 2 — ingestion keys**

3. `ingestion_keys.rs`: extract `authorize_mint`; rewrite `mint_key` to call it and delete the
   admin pre-check and the `Err(e) if caller.is_admin()` arm.
4. `ingestion_keys.rs`: delete the four `try_claim_and_mint` admin fallbacks; trim `insert_key`'s
   doc comment.
5. `ingestion_keys.rs`: add `LIST_KEYS_VISIBILITY_SQL`, compose it into both `list_keys` branches.
6. `ingestion_keys.rs`: add the authority predicate to `revoke_key`'s `UPDATE` plus the
   404/403/404 disambiguation.
7. `ingestion_keys.rs`: call `authorize_mint` from `import_key`.
8. `web_server.rs`: the missing-`mint`-grant startup `warn!` for a non-`public` default audience.

**Phase 3 — grants surface**

9. `audience_grants.rs`: populate `held_pairs` for admins; update `MyAudiencesResponse`'s field
   doc and `my_audiences`'s own doc comment.

**Phase 4 — clients**

10. `analytics-web-app/src/components/MintIngestionKeyDialog.tsx`: drop the `isAdmin`
    special-casing (`:70-71`, `:158`, `:179`) — every caller gets the `mint_prefix` composition
    and the claim hint, since every caller now takes the same server path.
11. `analytics-web-app/src/routes/AudienceAccessPage.tsx`: the Mint button (`:793`) switches from
    `!isAdmin` to `heldPairs.has(`${audience}:mint`)`; the Share/delete checks (`:424`, `:430`,
    `:498`) keep `isAdmin`.
12. `python/micromegas/micromegas/cli/setup_telemetry.py`: delete the admin special-case
    `parser.error` (`:200-208`) so admins resolve through the shared `held_pairs` path; update
    `resolve_audience`'s docstring (`:154-162`).

**Phase 5 — tests and docs**

13. Rust unit tests (no DB) — see Testing Strategy.
14. Update the existing live suites' admin expectations.
15. Frontend and Python test updates.
16. Documentation and `CHANGELOG.md`.

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
- `analytics-web-app/src/components/__tests__/ApiKeysAdminPage.test.tsx`,
  `src/routes/__tests__/IngestionApiKeysPage.test.tsx`,
  `src/routes/__tests__/AudienceAccessPage.test.tsx` (as affected)
- `python/micromegas/micromegas/cli/setup_telemetry.py` + its tests
- `mkdocs/docs/admin/authentication.md`, `mkdocs/docs/admin/authorization.md`,
  `mkdocs/docs/admin/api-keys.md`
- `tasks/data_isolation/audience_based_access_control_plan.md`
- `CHANGELOG.md`

## Trade-offs

**Audit-by-grant-row vs. audit-by-log-line.** The cheapest alternative is to keep every bypass and
emit a log line whenever an admin uses one. Rejected: a log line is not queryable alongside the
grants it shadows, not revocable, and not visible on the Audience Access page. The grant row is
all three, and it needs no new storage or surface.

**Narrowing the grant/membership listing too.** The issue lists `visible_grants`' admin branch as
a candidate. Rejected as circular: the grant table is the record of who may access what, so
reading it *is* the administration capability the issue keeps for admins, and an admin who must
self-grant in order to read the grant table has no way to discover what to grant. Group listing
is the same argument.

**Stripping `*` for every write, not just delegation.** Would be the most uniform rule, and would
force an explicit grant even for `public`. Rejected: `('public','mint','*')` is a shipped seed
row, so this would break `public` minting and importing for *every* caller, not narrow admin —
the wildcard row is an explicit grant, which is exactly what this plan asks for.

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
- `*` is honored for mint/import/revoke authority and for key visibility, and kept stripped for
  grant delegation. The two planes are allowed to answer the wildcard question differently
  because delegation and exercise are different effects.
- `revoke_key` requires `mint` (not any-axis) authority: revoking someone's write credential is a
  write against that audience's credential set. Listing takes any axis, since a `read`-only holder
  has a legitimate interest in knowing what writes into an audience they can read.
- `import_key` gets no lazy-claim path. Import exists to migrate an *existing* operator-chosen key,
  which by definition already has a home; duplicating `try_claim_and_mint`'s advisory-lock
  transaction for it buys one convenience at the cost of a second claim implementation. An
  operator importing into a fresh audience creates the grant row first, in one call.
- Quota exemptions (`max_keys_per_caller`, `max_claims_per_caller`, `max_grants_per_caller`) stay
  admin-exempt. They bound self-service abuse, not access.
- `bulk_ingest`'s `is_admin` gate (`flight_sql_service_impl.rs:1511`) is an accepted carve-out. It
  writes the `audience` column verbatim, which by this plan's rule should need a grant — but its
  whole purpose is cross-audience replication of a lake already stamped at origin, so per-audience
  grants cannot express it, and it grants no *read*. Left as-is, documented as the one remaining
  data-plane admin authority.
- `analytics_keys.rs` is out of scope: analytics keys carry no audience binding, so there is no
  audience authority to scope them by.
- Accepted behavior break: an admin can no longer mint into an existing audience they hold no
  grant on, nor into a custom `MICROMEGAS_DEFAULT_AUDIENCE` with no `mint` row. Both are one
  `create_grant` call away, and the second gets a startup warning.

## Documentation

- **`mkdocs/docs/admin/authentication.md`** §Admin Privileges (`:561-582`): add what admin is
  *not* — no implicit `read` or `mint` on any audience; data access is always a grant row, for
  admins too. Keep the existing capability list.
- **`mkdocs/docs/admin/authorization.md`**:
  - §Self-service mint: replace the "An admin's mint claims too" bullet (`:228-230`) with the
    single shared rule; add the custom-default-audience one-time grant step.
  - §Configuration: the `MAX_KEYS_PER_CALLER` row (`:24`) — `list_keys`/`revoke_key` are still
    admin-only but now audience-scoped.
  - §Routes table (`:293`): `held_pairs` is no longer "(empty for an admin)".
  - §Write gate (`:307`): unchanged, but state explicitly that it is unchanged *because* it is
    grant administration.
  - §`list_audience_grants()` (`:332`): unchanged; note it is the grant-administration surface,
    not a data read.
  - §Admin-gated lakehouse functions: add `bulk_ingest` as the remaining data-plane carve-out.
- **`mkdocs/docs/admin/api-keys.md`**: `:17-23` (what stays admin-only), `:92-100` (the gate
  description), `:212-230` (the admin-claims story collapses into the shared path), `:275-300`
  (the admin page now shows an audience-scoped list).
- **`tasks/data_isolation/audience_based_access_control_plan.md`**: generalize §5's "No
  human-admin query-path bypass" into "no human-admin data-plane bypass" and note the
  `bulk_ingest` carve-out; update §"Admin surface" to record that the blanket gate is now scoped
  to the control plane, leaving delegated ownership as the remaining follow-up.
- **`CHANGELOG.md`** Unreleased, with a **Minor breaking change** clause for
  `AudienceMintPolicy::resolve_audience`'s behavior change and the admin-visible route changes.

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

**Unit, no DB — `rust/analytics-web-srv/tests/ingestion_keys_tests.rs`.** Follows the existing
`claim_count_statement_counts_grants_not_keys` pattern, which is how this crate asserts a SQL
shape that would otherwise need a live database. Authorization predicates dropped from a query
fail silently — an over-broad list returns more rows, not an error — so they get an automated
guard even though the tier is coarse:

- `LIST_KEYS_VISIBILITY_SQL` contains `FROM audience_grants`, `k.created_by = $3`, and
  `g.audience = k.audience`
- both `list_keys` branches reference `LIST_KEYS_VISIBILITY_SQL`, and no `SELECT … FROM
  ingestion_api_keys` in the module lists rows without it
- `revoke_key`'s `UPDATE` carries `axis = 'mint'` and the `created_by` arm
- `import_key` calls `authorize_mint`

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
- `live_mint_list_revoke_round_trip` — extend with a second key in an audience the caller holds no
  grant on, asserting it is absent from `list_keys` and returns 404 from `revoke_key`. Extending
  this existing test is what covers the narrowing end-to-end against a real Postgres: the scoping
  is a correlated `EXISTS` across two tables, which the SQL-shape assertions above can confirm is
  *present* but not that it *filters correctly*.
- `live_import_is_idempotent` — needs a `mint` grant on the target audience in setup.
- `live_my_audiences_admin_gets_a_normal_response_regardless_of_knob` — `held_pairs` is now
  populated for an admin.
- `live_visible_admin_sees_every_row` — unchanged; it is the regression guard that the control
  plane did *not* narrow.

**Frontend (vitest).** `MintIngestionKeyDialog`: an admin now sees the prefix composition and the
claim hint. `AudienceAccessPage`: the Mint button follows `held_pairs`, not `isAdmin`, while Share
still follows `isAdmin`. `IngestionApiKeysPage`/`ApiKeysAdminPage`: unchanged behavior, but their
admin fixtures may need a `held_pairs` value.

**Python.** `setup_telemetry`'s `resolve_audience`: the admin branch is gone, so an admin with
exactly one held mint audience resolves it silently, and an admin with none gets the
same "no mintable audience" error as anyone else.

## Manual Verification

Each step below exercises the browser/CLI wiring that no test reaches, and each fails loudly and
immediately — a 403 on a visible button, an empty table — rather than silently.

1. Start the monolith with auth enabled and `MICROMEGAS_SELF_SERVICE_MINT` unset:
   `cargo run --bin micromegas-monolith -- --roles all --listen-endpoint-http 127.0.0.1:9000
   --frontend-dir ../analytics-web-app/dist`. Sign in as a member of `admins` who holds no grant
   rows. Expect Admin → Ingestion API Keys to list only `public`-bound keys.
2. Mint a key naming an existing audience you hold no grant on (Mint dialog → pick it from a name
   you created out-of-band). Expect `403`, with the audience absent from the dialog's list in the
   first place.
3. Mint naming a brand-new audience. Expect `201` with `claimed: true`, and two new
   `user:<you>`/`mint`+`read` rows on Audience Access.
4. `micromegas-grants create team-alpha mint user:<you>` as the admin, then mint into
   `team-alpha`. Expect `201` with `claimed: false`, and the key visible in the list.
5. `micromegas-query "SELECT * FROM list_audience_grants()"` as the admin. Expect every row —
   the control plane did not narrow.
6. Restart with `MICROMEGAS_DEFAULT_AUDIENCE=corp` and no grant on `corp`. Expect the startup
   `warn!` in `/tmp/monolith.log`, and a `403` from a mint with no explicit audience.

## Open Questions

1. **A client-credentials admin (no email) can no longer mint into an unclaimed audience.** No
   `user:` selector can be formed for it, and the claim path needs one. Recommendation: accept —
   such a caller should be granted a `group:` mint row, which is the traceable outcome this plan
   is after. The alternative is a `group:`-selector claim path, which needs a rule for *which* of
   the caller's groups owns the claim.
2. **Should the startup warning (step 9) be a hard startup failure instead?** A custom default
   audience with no `mint` grant means no caller can mint the deployment's default — arguably a
   misconfiguration, not a warning. Recommendation: warn, since the mint route is not the only
   ingestion path and refusing to start would break upgrades of existing custom-default
   deployments.
3. **Does `revoke_key` need a `created_by` escape hatch at all?** Keeping it lets a non-admin's
   self-service key be revoked by its creator once `list_keys`/`revoke_key` are ever opened to
   non-admins — which they are not today. Recommendation: keep it; it costs one `OR` and removes
   the "freeing a slot needs an admin" caveat the config docs currently carry.
