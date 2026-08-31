# Let a Non-Admin Mint a Key Bound to the Default Audience Plan

Issue: [#1535](https://github.com/madesroches/micromegas/issues/1535)

## Overview

The issue asks for two things. **(1)** Let a non-admin mint into the deployment default audience
(`MICROMEGAS_DEFAULT_AUDIENCE`, `public` when unset). **(2)** Stop
`micromegas-setup-telemetry` from silently rewriting an explicit `--audience X` it believes the
caller cannot mint into `{mint_prefix}X`.

**(1) needs no policy code — it needs a seeded default.** A single grant row,
`('public', 'mint', '*')`, already delivers it through the shipped `AudienceMintPolicy`; it is
already gated by `MICROMEGAS_SELF_SERVICE_MINT` with no new gating logic, and it is auditable and
revocable in a way an implicit code arm is not. But leaving that row for each operator to discover
and create by hand just relocates the original complaint. So the row ships **seeded by a migration**
(schema v9), and the work for (1) is the seed plus saying what it means.

**The same reasoning finishes the read axis.** `AudienceReadPolicy::resolve` grants every caller
read on `public` with a hardcoded `set.insert(PUBLIC_AUDIENCE)` — the exact shape declined above for
mint, with the same costs: invisible on the Audience Access page, impossible to narrow or revoke, and
the reason that page has to explain public read *in prose*. Once the table is seeded, that arm has
no reason to exist. v9 seeds `('public','read','*')` alongside the mint row and the arm is deleted,
so **neither axis has a built-in grant** and the table is the single expression of policy.

**(2) is the CLI change.** `--audience` stops meaning two things; the fresh-claim path moves to its
own `--claim NAME` flag. This is confined to `setup_telemetry.py` — no server change, no
`/my-audiences` change.

## Current State

### Mint is grant-only for a non-admin; read is not

`rust/auth/src/policy.rs`:

- `AudienceReadPolicy::resolve` (`:504-533`) unconditionally seeds the readable set with
  `PUBLIC_AUDIENCE` — every authenticated caller can read `public`.
- `AudienceMintPolicy::resolve_audience` (`:583-626`) has two arms: `caller.is_admin` mints any
  format-valid audience; everyone else needs `aud` to appear in `grants[a].mint`.
- `rust/auth/tests/policy_tests.rs:229-241` (`mint_policy_public_is_not_mintable_by_default`) pins
  that asymmetry — but read it precisely: it asserts `public` is not mintable **from an empty grant
  map**. It says nothing about a grant map that names `public`.

The asymmetry is correct and stays: mint is integrity, read is confidentiality
(`policy.rs:535-549`).

### The mechanism already exists

`selector_matches` (`policy.rs`) returns `true` unconditionally for `"*"`, on either axis.
`mint_key` (`rust/analytics-web-srv/src/ingestion_keys.rs:328-491`) runs a per-request point query
for `audience = $1 AND axis = 'mint'`, feeds exactly those rows to `AudienceGrants::from_rows`, and
calls `AudienceMintPolicy::resolve_audience`. So a row `('public', 'mint', '*')` makes `public`
mintable by every authenticated caller, today, with no code change.

That row is also creatable and removable through the shipped admin surface, with no new UI or CLI
work — which is what makes it a *default* an operator can actually see and override, rather than a
hidden one:

- **Audience Access page** (`/audiences`), admin-only **Add grant** button
  (`AudienceAccessPage.tsx:796-801`): Audience = free text, Axis = Read/**Mint**
  (`:265-266`), Selector = **Everyone** / User / Group, where Everyone submits `'*'`
  (`:166`). Audience `public` + Axis `Mint` + Selector `Everyone` is exactly the row. `Everyone` is
  offered only in the admin add mode, never in the non-admin Share dialog — the client-side mirror
  of `create_grant`'s own `selector == "*"` refusal for non-admins.
- **CLI**, for scripted setup (`python/micromegas/micromegas/cli/grants.py:110-112`):

  ```bash
  micromegas-grants --url https://analytics.example.com create public mint '*'
  ```

`create_grant` (`audience_grants.rs`) validates only `is_valid_audience` / axis ∈ {read, mint} /
`valid_selector`, with no reserved-name check, so `public` + `mint` + `*` is accepted from an admin
on either path. The row then appears on that same page, under `public`'s Mint column.

**The knob still gates it, for free.** `MintGate` (`ingestion_keys.rs:293-319`) rejects every
non-admin before `mint_key`'s body runs whenever `MICROMEGAS_SELF_SERVICE_MINT` is off. So the row
confers nothing while the knob is off — which is exactly the "cannot widen anyone's surface on
upgrade" property the issue wanted from gating on that knob.

The issue itself notes this path exists ("pre-insert a `('public', 'mint', <selector>)` grant
row") and objects that it is "not discoverable from the CLI's own help or error output." That is a
discoverability problem, not a missing capability.

### The grant table currently ships empty

`audience_grants` is created by `upgrade_data_lake_schema_v7`
(`rust/ingestion/src/sql_migration.rs:188-226`) with no rows, and nothing seeds it afterwards. So a
fresh deployment starts with an empty table and *zero* expressed policy — every default it appears
to have is really a hardcoded arm somewhere:

- `AudienceReadPolicy::resolve` (`policy.rs:512`) inserts `PUBLIC_AUDIENCE` unconditionally.
  `AudienceGrants`'s own doc comment (`:242-244`) states the intent: "`public` is not stored here:
  it is the sole built-in read grant, applied by `AudienceReadPolicy::resolve` directly rather than
  needing a `{"public": ["*"]}` entry (though writing one changes nothing)."
- The Audience Access page then has to *explain that in prose* (`AudienceAccessPage.tsx:842-846`),
  precisely because there is no row to show.

An empty table is why "0 grants across 0 audiences" is the honest summary of a working deployment,
and why an operator reading `public`'s empty Mint column concludes minting needs no grant either.
Whatever the mechanism for (1), it must not leave the table empty by default.

### Why the deployment default is a reasonable thing to open

`rust/public/src/servers/write_audience.rs:21-36` — `resolve_write_audience` stamps the deployment
default on any credential with no `bound_audience`, and on `ctx: None`. `rust/auth/src/oidc.rs`
never sets `bound_audience`, and `serve_ingestion`
(`rust/public/src/servers/ingestion.rs:144-200`) puts every ingestion route behind a plain
`auth_middleware` with no admin check. So in any deployment whose ingestion service has OIDC
configured, an ordinary authenticated user can **already write into the deployment default audience
today**, with no grant. The `*` mint row does not grant new write authority; it grants a *standing
credential* for authority the caller already has interactively.

### The CLI substitutes instead of asking

`python/micromegas/micromegas/cli/setup_telemetry.py:55-125` — `resolve_audience` consults
`GET /api/audience-grants/my-audiences` and applies a three-way rule. The last branch:

```python
resolved = f"{mint_prefix}{args.audience}"          # "public" -> "jane-doe-public"
print(f"claiming fresh audience: {resolved}", file=sys.stderr)
return resolved
```

`my_audiences` (`audience_grants.rs:802-858`) returns
`SELECT DISTINCT audience FROM audience_grants WHERE axis = 'mint' AND selector = ANY(...)`, where
the bound array is `caller_selectors(&caller)` — which **includes `"*"`**. So the moment the grant
row above exists, `public` appears in every caller's `audiences` list and the CLI's *existing*
first branch ("already in the list → used verbatim") handles `--audience public` correctly with no
change at all. Without the row, `public` is absent and the silent rewrite fires.

`mkdocs/docs/query-guide/python-api.md:948-953` documents the rewrite as intentional ("There is no
flag to bypass this prefixing"), on the grounds that it keeps bare names like `prod`/`ci` out of
self-service reach. That guardrail is worth keeping — but it does not require *silence*.

### The one thing a seeded `public` mint row perturbs

`resolve_audience`'s `--audience`-omitted branch auto-resolves from `audiences` and uses a single
match silently. Once the seed puts `public` in every caller's list, a caller holding one personal grant has
**two** entries, so `micromegas-setup-telemetry --name my-laptop` (the `eval "$(...)"` recipe the
docs lead with) starts failing with "multiple mintable audiences found". That has to be handled —
see Design §4.

## Design

### §1 — A seeded default row, not a policy code path

Schema **v9** (`rust/ingestion/src/sql_migration.rs`, a new `upgrade_data_lake_schema_v9` plus its
arm in `execute_migration:390-398`) seeds one row:

```sql
INSERT INTO audience_grants (audience, axis, selector, created_at, created_by)
VALUES ('public', 'read', '*', now(), 'default'),
       ('public', 'mint', '*', now(), 'default')
ON CONFLICT DO NOTHING;
```

Two rows, one per axis. The `read` row replaces the built-in arm removed in §2; the `mint` row is
what the issue asks for.

`created_by = 'default'` rather than a user identity, so the rows are self-describing on the
Audience Access page and in `list_audience_grants()`: an operator can see they were shipped, not
added by a colleague. `ON CONFLICT DO NOTHING` because an operator who already created either row by
hand (the path the issue describes for `mint`, and the `{"public": ["*"]}` env entry the docs say
"changes nothing" for `read`) must not fail the migration.

**Literal `public`, not `MICROMEGAS_DEFAULT_AUDIENCE`.** A migration that reads an env var bakes in
whatever the variable happened to be at upgrade time and silently goes stale when it changes. A
deployment with a custom default adds its own row — and should, since a custom default is
already a deliberate isolation posture (see *Security*).

The row is then an ordinary grant, so both shipped admin surfaces manage it with no new UI or CLI
work. To **remove** it (a deployment that wants per-caller isolation with the knob on), Audience
Access → `public` → Mint → Remove, or:

```bash
micromegas-grants --url https://analytics.example.com delete public mint '*'
```

To **add** the equivalent mint row for a custom default:

```
Audience:  unassigned      Axis: Mint    Selector: Everyone
```

```bash
micromegas-grants --url ... create unassigned mint '*'
```

**Do not pair it with `read '*'` by reflex.** A custom `MICROMEGAS_DEFAULT_AUDIENCE` is, by the
argument in *Security*, a deployment that has already opted into an isolation posture the built-in
`public` seed does not carry — `unassigned` starts out readable only through explicit grants.
`micromegas-grants create unassigned read '*'` erases exactly that: it makes `unassigned` readable
by every authenticated caller, i.e. a second `public`, which is the posture a custom default exists
to avoid. If callers writing where they cannot read back is a real problem for such a deployment,
the isolation-preserving fix is per-user or per-group read grants on the default
(`micromegas-grants create unassigned read 'user:<email>'` / `'group:<g>'` for the callers who need
to read back what they wrote), not a blanket `'*'`.

A deployment with a custom default and the knob on that skips the mint row above has no mintable
default and will hit the CLI error (§4) until it does. A startup `warn!` for that case
("self-service mint is on and `<default>` has no mint grant") is cheap and would be a natural
companion to the seed, but is not planned here — it is a separate, additive change, not required for
this issue.

**No `rust/` policy code changes** — only the migration. What this buys over an implicit
`open_audience` arm on `AudienceMintPolicy`:

- **Auditable, on the same page that creates it.** The row shows up under `public`'s Mint column on
  the Audience Access page and in `list_audience_grants()`, so an operator can see *why* every
  caller can mint into `public`, and revoke it there. An implicit code arm is invisible on both
  surfaces — an operator auditing "who may mint into `public`" would read an empty Mint column and
  be wrong.
- **Revocable and tunable per deployment.** `delete` the row, or narrow `'*'` to `group:eng`. The
  code arm is all-or-nothing and tied to a knob that governs four other things.
- **Composes.** The read half of the problem (a custom default that callers can write but not read)
  is the same mechanism with `read` instead of `mint`. A code arm would need a second code arm.
- **Already gated.** `MintGate` blocks knob-off non-admins, so the row is inert until the operator
  opts into self-service — no new gating logic, no second place for the knob to be checked.
- **Consistent with existing guidance.** The docs already tell operators to pre-create a
  placeholder row for the default before turning the knob on
  (`mkdocs/docs/admin/authentication.md:541-560`). This is that same row, with axis `mint`.

The cost is that the default now has to be *chosen* rather than left implicit, and applied to
existing deployments by a migration — see *Security* for the upgrade question that raises, which is
the one genuinely contentious decision in this plan.

**Env-map config cannot do this.** `MICROMEGAS_AUDIENCE_GRANTS` is never consulted on the mint
route — `mint_key` builds its `AudienceGrants` purely from the DB point query. This matches the
existing documented rule that self-service mint grants must live in the DB table
(`authentication.md:492-500`), and the docs should not suggest the env map here.

### §2 — Delete the built-in public read grant

One line goes (`policy.rs:512`):

```rust
set.insert(PUBLIC_AUDIENCE.to_string());   // deleted
```

`PUBLIC_AUDIENCE` stays as a const — `default_audience_from_env` (`:94`) and
`ingestion_keys.rs`'s reserved-name check still use it. `policy.rs:512` is the **only** place the
read path hardcodes it; nothing in `rust/analytics/src` or `rust/public/src` does. The resolved set
becomes exactly:

```text
  { a : "*" | "user:<email>" | "group:<g>" ∈ grants[a].read }   env map
∪ { a : selector ∈ store.readers(a) matches caller }            DB store
∪ caller.read_audiences                                         per-key direct grant
```

Doc comments to correct, since all five currently assert the built-in as intended design:

- `AudienceGrants` (`:242-244`): "`public` is not stored here: it is the sole built-in read grant
  … (though writing one changes nothing)." It *is* stored now, and writing one is how it works.
- `AudienceReadPolicy` (`:440-461`): the formula at `:444` loses its leading `{ PUBLIC_AUDIENCE }`
  term.
- `PUBLIC_AUDIENCE`'s own doc comment (`policy.rs:31`): "The reserved audience every authenticated
  principal may read" is no longer true of the const itself — it stays true only because of the
  seeded row now.
- `ownership_rewrite.rs:215`: "a caller matching no grant resolves to `{public}`" is the rationale
  given for that code's fail-closed `lit(false)` arm; the empty set is now reachable in production
  (missing seed, un-migrated DB) for the first time, so the comment needs to say so.
- `monolith/main.rs:250-252`: "an empty grant map -> a real caller's resolved scope is just
  `{public}`" describes the exact wiring site §2 depends on and needs the same correction.

**Closing the one path where this could go dark.** `AudienceReadPolicy` has four construction
sites; three are already safe, one is not:

| Site | Store | After the deletion |
|---|---|---|
| `flight_sql_server.rs:320` (default-auth) | `with_store(Some(..))` | fine — reads the seeded row |
| `monolith/main.rs:276-277` | `with_store(Some(..))` | fine |
| `flight_sql_server.rs:333` (disabled-auth) | none | fine — no `AuthContext` extension is ever inserted on this path, so the absent-extension convention supplies `ReadScope::All` and this policy is never resolved |
| `flight_sql_server.rs:289` (injected-provider) | **none** | **grants nothing to anyone** |

`:289` is the default for a caller that pairs `with_auth_provider` without `with_read_policy`
(the monolith does call it, so this is the embedder-forgot fallback). Today it still reads `public`
because of the built-in arm; with the arm gone it resolves to the empty set and every query returns
nothing. Fix it by giving that branch the same env+store-backed default the `use_default_auth`
branch builds at `:311-321` — `lake_pool_for_keys` is already in scope, so it is a few lines. The
"defensive empty default" was only ever safe because of the arm being removed.

**The split-deployment ordering window.** Only `telemetry-ingestion-srv` and `micromegas-monolith`
run migrations (`connect_to_remote_data_lake` → `migrate_db`); `flight-sql-srv` and
`analytics-web-srv` do not. So a split deployment that upgrades flight-sql-srv *before* the
ingestion binary has applied v9 gets new code reading a v8 database: no seeded row, no built-in arm,
every query silently empty. That is the real risk in this section, and documenting a deploy order is
not enough for a silent failure.

Make it loud: `FlightSqlServer::build_and_serve` (`rust/public/src/servers/flight_sql_server.rs`)
checks the data-lake schema version right after it obtains `lakehouse` — the earliest point in that
function with a DB pool, whether `lakehouse` came from `LakehouseContext::from_env()` or was
injected — and refuses to proceed below v9. **The floor is v9 specifically, not
`LATEST_DATA_LAKE_SCHEMA_VERSION`.** `default_provider.rs:130-137` is the only existing precedent
for a startup schema floor, and it too names a specific version ("has the schema reached migration
v5?") rather than "latest" — a LATEST-based floor would impose a hard rollout ordering constraint on
every future migration, which nothing in the codebase does today (v7 and v8 skew was handled with
CHANGELOG deploy-order notes instead); that is a separate, more sweeping change than what this issue
needs. There is precedent for the message, too —
`default_provider.rs:130-137` already tells the operator "the ingestion binary or monolith must run
the migration before flight-sql starts in a split deployment" for the v5 key tables.
`read_data_lake_schema_version` (`rust/ingestion/src/sql_migration.rs`, `pub`, currently only
called from `remote_data_lake.rs`) is the existing read-only helper. The check runs unconditionally,
before the `--disable-auth` / `use_default_auth` / injected-provider branch is chosen: split-deployment
skew is a property of the database, not of the auth path, so it also covers the monolith's own call
into `build_and_serve` (redundant there, since the monolith runs the migration itself first, but
harmless) and the `--disable-auth` path (whose `ReadScope::All` doesn't depend on the schema, but
which should not mask a stale DB either). This converts a data blackout into a startup failure naming
the fix, and is worth having independent of this change — flight-sql-srv validates no schema version
at all today.

**Not doing:** a conditional fallback ("insert `public` if the store snapshot and env map are both
empty"). That is the hardcode with extra steps, and it makes "nothing configured yet" indistinguishable
from "deliberately empty" — the exact ambiguity the seed exists to remove.

### §3 — CLI: `--audience` means what it says, `--claim` claims

`python/micromegas/micromegas/cli/setup_telemetry.py`. New flag, mutually exclusive with
`--audience`:

```
--claim NAME    Claim a fresh audience under your own namespace. Minted as
                "{mint_prefix}NAME" (e.g. "alice-ci-runner"), never the bare name.
```

`resolve_audience`'s new rule:

| Input | Behaviour |
|---|---|
| both `--audience` and `--claim` | `parser.error` |
| `--claim NAME`, admin caller | `parser.error` — an admin's brand-new audience is claimed server-side; use `--audience NAME` |
| `--claim NAME`, no `mint_prefix` | `parser.error` (unchanged text: no email to claim with) |
| `--claim NAME` | `f"{mint_prefix}{NAME}"`, announced on stderr (today's fresh-claim path, now reached only on request) |
| `--audience X`, `X in audiences` | `X` verbatim (unchanged — and this is the branch `public` lands in once the grant row exists) |
| `--audience X`, admin | `X` verbatim (unchanged) |
| `--audience X`, otherwise | `parser.error` (**was**: silent `{mint_prefix}X`) |
| `--audience`/`--claim` both omitted | see §4 |

The final error names the reason and every way forward:

```
cannot mint audience 'public': it is not in this caller's mintable set
  mintable audiences: team-alpha
  to claim a fresh audience under your own namespace: --claim public  (mints as 'alice-public')
  otherwise, ask an admin to grant it:
      micromegas-grants --url <url> create public mint 'user:alice@example.com'
    or, to open it to every authenticated caller:
      micromegas-grants --url <url> create public mint '*'
```

That last hint is the whole of the issue's discoverability complaint, answered where the user hits
it. The CLI knows `args.audience`, and knows the caller's email from `my_audiences["email"]`, so
both commands can be rendered concretely rather than as a template.

Note what this does **not** need: no `default_audience` or `default_audience_mintable` field on
`MyAudiencesResponse`, no `default_audience` on `AudienceGrantsState`, no `web_server.rs` change,
and none of the 33 `AudienceGrantsState { .. }` test literals. The server already tells the client
everything it needs, because the mechanism is a grant row and grant rows are what
`/my-audiences` reports.

**The bare-name guardrail survives.** A non-admin still cannot mint bare `prod` through this
script — the outcome changes from a silent rename to a loud error. (The *route* still accepts any
valid unclaimed name from an authorized non-admin; the prefixing was always a script convention and
remains one.)

`my_audiences.get("email")` rather than `[...]`, consistent with the existing `.get("mint_prefix")`.

While editing: `resolve_audience`'s `client` parameter has been unused since #1510 removed the
admin branch's list calls — drop it, since every caller and test is being touched anyway.

### §4 — Keep auto-resolution working now that `public` is in every caller's list

`--audience`-omitted resolution filters `audiences` to the caller's **personally held** mint
audiences before applying the existing zero/one/many rule:

```python
held = set(my_audiences["held_pairs"])
personal = [a for a in audiences if f"{a}:mint" in held]
```

`held_pairs` (`audience_grants.rs:772-786`) is exactly this: `"{audience}:{axis}"` for every pair
the caller holds via an *identity* selector, with `"*"` filtered out of `caller_selectors` before
the query. It already exists on the response and needs no server change; it was added so the web
app could tell "a pair I hold" from "a pair I can merely see", which is the same distinction needed
here. `/my-audiences` and `held_pairs` shipped together, both still `## Unreleased` — no released
server can return a `my_audiences` response missing the key at all (`client.my_audiences()`
fails first, on the 404), so there is no "older server" case to fall back for. Read the key directly
and let a genuinely malformed response raise, same as any other unexpected shape from this client.

Effects:

- A caller with one personal grant plus the seeded `public` row → one match → resolved silently,
  exactly as today. The `eval "$(micromegas-setup-telemetry --name my-laptop)"` recipe keeps
  working.
- A caller whose only mint authority is the seeded row → zero matches → the existing "no mintable
  audience found" error, whose text gains a suggestion built from data the CLI already has: the
  entries of `audiences` that are not in `personal` (i.e. `[a for a in audiences if a not in
  personal]`) — visible-but-not-personally-held audiences the caller could pass explicitly. On the
  stock deployment that is `public`; on a deployment with `MICROMEGAS_DEFAULT_AUDIENCE=unassigned`
  and its own seeded mint row (§1) it is `unassigned` instead, so the hint never names an audience
  the caller cannot actually mint. This is the right outcome: a caller with no audience of their own
  should not silently publish into the shared pool because they omitted a flag.
- Admins are unaffected: `held_pairs` is always empty for an admin, and an admin must already pass
  `--audience` explicitly.

Behaviour change to declare: a deployment that already has a `*` mint row on some audience and
relies on auto-resolution picking it will now get the zero-match error instead. This is niche (it
requires a pre-existing `*` mint row, which nothing in the docs currently recommends) and moves in
the same direction as the rest of the change — explicit beats implicit for a shared audience.

Simpler alternative, if the filter feels like too much: skip §4 entirely and accept that every
caller must pass `--audience` explicitly once the seed lands. Rejected as
the default because it degrades the headline recipe for every caller in the deployment, but it is a
one-line difference if preferred.

### §5 — The page legend stops describing a special case, and the Mint dialog stops defaulting to it

The Audience Access page's legend (`AudienceAccessPage.tsx:842-846`) currently reads:

> **Scope:** `public` is always readable by every authenticated principal — no row needed.

That sentence is the page-level version of the bug: it tells an operator `public` needs no row,
which was true for read and never true for mint. After §1 and §2 it is simply false — `public` has
two rows, and they are what grant its access. Replace it, without claiming the rows are listed for
the current viewer — `visible_grants` (`audience_grants.rs:672-720`) strips `"*"` from
`caller_selectors` on both non-admin branches, so a non-admin viewer never sees the seeded
`('public', axis, '*')` rows themselves, only their effect:

> **Defaults:** `public` ships with Read and Mint grants for everyone (attributed to `default` on
> the admin view). They are ordinary grants: removing the Read row stops public data from being
> universally readable; removing the Mint row limits minting into `public` to admins.

The legend gets shorter and stops describing a special case, because after this change there isn't
one — every grant on the page is a row on the page, for an admin. That is the real payoff of §2, and
this is where a reader sees it — as an admin; a non-admin still cannot see either seeded row (only
their effect on what audiences they can read and mint), which is unchanged from today.

**The Mint dialog's own default must change, not just its legend.** The header "Mint ingestion key"
button (`AudienceAccessPage.tsx:744`, `showMintButton`) is offered to any non-admin once the knob is
on, and its dialog's `useEffect` (`:361-362`) seeds `audienceChoice` from `me.audiences[0]` with no
prefill — the `<select>` (`:455`) lists `me.audiences` verbatim, sorted server-side by
`my_audiences` (`audience_grants.rs`). Once the seed lands, `public` is in every non-admin's
`audiences` and sorts ahead of most personal audience names, so the dialog opens pre-selected on
`public` for anyone who hasn't personally claimed something earlier alphabetically — silently
defaulting a non-admin's key to the shared audience, the exact hazard §4 exists to prevent on the
CLI. Apply the same rule here: default `audienceChoice` from the caller's **personally held** mint
audiences (`me.held_pairs`, already read elsewhere on this page at `:644` — filter `me.audiences` to
entries with `"{audience}:mint"` in `held_pairs`, the same test §4 applies), taking the first match;
fall back to `'__new__'` when the caller holds none, exactly as today's `!me?.audiences.length`
branch does. `public` (or any other `'*'`-only audience) stays selectable in the dropdown — nothing
is hidden — it is just never the initial selection unless the caller personally holds it or
`prefillAudience` names it explicitly.

## Implementation Steps

**Phase 1 — the seeded default**

1. `rust/ingestion/src/sql_migration.rs`: add `upgrade_data_lake_schema_v9` with the seed INSERT,
   bump `LATEST_DATA_LAKE_SCHEMA_VERSION`, and add the `if 8 == current_version` arm in
   `execute_migration` (`:390-398` is the v8 arm to mirror). Check whether
   `rust/ingestion/tests/sql_migration_test.rs` pins the version — it will need a new
   `build_v8_schema` helper (chaining `build_v7_schema` with the v8 step directly, bypassing
   `execute_migration`, in the same style as `build_v6_schema`/`build_v7_schema`) so a v9 test has a
   pre-v9 fixture to migrate from.

**Phase 2 — remove the built-in read grant**

2. `rust/auth/src/policy.rs`: delete `set.insert(PUBLIC_AUDIENCE.to_string())` (`:512`); correct the
   `AudienceGrants` (`:242-244`), `AudienceReadPolicy` (`:440-461`), and `PUBLIC_AUDIENCE` (`:31`)
   doc comments, plus the built-in-read-grant assertions in
   `rust/analytics/src/lakehouse/ownership_rewrite.rs:215` and `rust/monolith/src/main.rs:250-252`.
3. `rust/public/src/servers/flight_sql_server.rs:282-291`: give the injected-provider branch the
   same env+store-backed default the `use_default_auth` branch builds at `:311-321`. Add a comment
   on the disabled-auth branch (`:333`) noting that its never-resolved property is now load-bearing.
4. `rust/public/src/servers/flight_sql_server.rs`: in `FlightSqlServer::build_and_serve`, right after
   `lakehouse` is obtained (either path), refuse to proceed below data-lake schema **v9 specifically**
   (not a general `LATEST_DATA_LAKE_SCHEMA_VERSION` floor — see §2) via
   `micromegas_ingestion::sql_migration::read_data_lake_schema_version`, with the split-deployment
   message `default_provider.rs:130-137` already uses as its model. Applies unconditionally,
   including on the `--disable-auth` and injected-lakehouse (monolith) paths.
5. `rust/auth/tests/policy_tests.rs`: update the read-side assertions that assume the built-in arm
   (`read_policy_public_is_always_present` `:89-95`, `read_policy_grantless_caller_resolves_to_exactly_public`
   `:148-157`, `read_policy_read_audiences_folds_into_the_read_axis` `:161-180`, plus `:615-620`
   and `:685-690`) to supply `public` through a grant map instead. They get better in the process:
   they will exercise the configuration production actually runs.

**Phase 3 — CLI**

6. `python/micromegas/micromegas/cli/setup_telemetry.py`: add `--claim`; rewrite `resolve_audience`
   per §3's table; add §4's `held_pairs` filter to the omitted branch; drop the unused `client`
   parameter; update `--audience`'s help text, the function docstring, and the module docstring.
7. `python/micromegas/tests/cli/test_setup_telemetry.py`: update existing cases for the new
   signature and add the new ones (see *Testing Strategy*).

**Phase 4 — pin the mechanism**

8. `rust/auth/tests/policy_tests.rs`: add `mint_policy_wildcard_selector_grants_mint_to_any_caller`.
   Nothing currently asserts `"*"` on the **mint** axis at all — it is only exercised on `read`.
   That behaviour is now load-bearing for a documented operator recipe, so it should be pinned
   rather than left as an emergent property of `selector_matches`. Amend the doc comment on
   `mint_policy_public_is_not_mintable_by_default` to say it pins "from an empty grant map", so the
   two tests read as complementary rather than contradictory.
9. `rust/analytics-web-srv/tests/ingestion_keys_tests.rs`, `#[ignore]` live-DB section (`:903+`):
   one end-to-end case — non-admin, knob on, `('public','mint','*')` row present,
   `{"audience": "public"}` → 201 with `audience == "public"` and `claimed == false`, and **no new
   `audience_grants` row for the caller** (mintable is not claimable; the reserved-name arm in
   `try_claim_and_mint` is never reached because the policy already said `Ok`). Unlike every other
   case in this section, this one must **not** call `cleanup_audience(&pool, "public")` at the
   end — that helper runs `DELETE FROM audience_grants WHERE audience = $1` against the real
   `MICROMEGAS_SQL_CONNECTION_STRING` database, and for `public` it would delete both v9-seeded
   rows, silently revoking public read/mint for the deployment and breaking every later run.
   Assert against the seeded rows and leave them in place; do not create or delete anything on
   `public`. This new case supersedes `live_mint_rejects_a_non_admin_claim_of_the_public_audience`
   (`:1094-1120`), which asserts the opposite outcome against the same live DB and starts failing
   once the v9 seed lands: repurpose it into the knob-off → 403 case instead (same request, non-admin,
   `('public','mint','*')` row present, `self_service_mint_enabled: false`), so it keeps pinning that
   `MintGate` rejects before the policy is ever consulted.

**Phase 5 — the page legend and Mint dialog default**

10. `analytics-web-app/src/routes/AudienceAccessPage.tsx:842-846`: extend the **Scope:** legend line
   per §5. Check whether `AudienceAccessPage.test.tsx` asserts on that text.
11. `analytics-web-app/src/routes/AudienceAccessPage.tsx:361-362`: change the Mint dialog's
   open-effect to default `audienceChoice` from `me.held_pairs`-filtered `me.audiences` (first
   match), falling back to `'__new__'`, per §5. `prefillAudience` keeps taking priority, unchanged.

**Phase 6 — docs and changelog**

12. `mkdocs/docs/admin/authentication.md`, `admin/api-keys.md`, `admin/web-app.md`,
   `query-guide/python-api.md` — see *Documentation*.
13. `CHANGELOG.md` — two things: the schema-v9 seeded default, naming the row and how to remove it,
   and an amendment to the existing `## Unreleased` `micromegas-setup-telemetry` entry describing
   the new `--audience`/`--claim` split in place (no **Minor breaking change** clause — that entry
   has never shipped in a release, so there is no compatibility window to call out; see *Trade-offs*
   and the repo's own precedent for this exact situation, `CHANGELOG.md:87`).

## Files to Modify

| File | Change |
|---|---|
| `rust/ingestion/src/sql_migration.rs` | schema v9: seed `('public','read','*')` + `('public','mint','*')` |
| `rust/auth/src/policy.rs` | delete the built-in `PUBLIC_AUDIENCE` read insert; correct three doc comments |
| `rust/analytics/src/lakehouse/ownership_rewrite.rs` | correct the fail-closed comment's built-in-read-grant assertion |
| `rust/monolith/src/main.rs` | correct the wiring-site comment's built-in-read-grant assertion |
| `rust/public/src/servers/flight_sql_server.rs` | real env+store default on the injected-provider branch; refuse to start below schema v9 in `build_and_serve` |
| `rust/auth/tests/policy_tests.rs` | read-side assertions supply `public` via a grant map; pin `"*"` on mint |
| `python/micromegas/micromegas/cli/setup_telemetry.py` | `--claim`; `resolve_audience` rewrite; `held_pairs` filter |
| `python/micromegas/tests/cli/test_setup_telemetry.py` | updated + new cases |
| `rust/analytics-web-srv/tests/ingestion_keys_tests.rs` | one `#[ignore]` live-DB end-to-end case |
| `analytics-web-app/src/routes/AudienceAccessPage.tsx` | extend the **Scope:** legend line; default the Mint dialog's audience choice from `held_pairs`, not `me.audiences[0]` (§5) |
| `mkdocs/docs/admin/authentication.md` | the Mint/Everyone recipe; mint-vs-claim distinction; updated script examples |
| `mkdocs/docs/admin/api-keys.md` | `--claim` in the naming-convention paragraph |
| `mkdocs/docs/admin/web-app.md` | Audience Access section: opening the default from the Add grant dialog |
| `mkdocs/docs/query-guide/python-api.md` | `micromegas-setup-telemetry` reference |
| `CHANGELOG.md` | Unreleased entry |

**No route or mint-policy changes.** `rust/analytics-web-srv/src/*` and `web_server.rs` are
untouched — `AudienceMintPolicy` gains nothing, and the migration adds rows, not columns (no
`SCHEMA_VERSION` file-schema concern, no Arrow schema change). The web-app changes are confined to
`AudienceAccessPage.tsx`: the legend string, and the Mint dialog's default-selection fix (§5) — the
Add grant dialog already does everything needed for granting
(`AudienceAccessPage.tsx:265-266, :166`), and the *separate* ingestion-key mint dialog on
`IngestionApiKeysPage.tsx` is behind `AuthGuard requireAdmin` (`:47`) and unaffected.

## Trade-offs

**Seeded rows vs. built-in arms, on both axes.** The mint side never got a built-in arm, so this is
a choice; the read side has one, so this is a removal. Same argument either way: a built-in grant is
invisible on the Audience Access page and in `list_audience_grants()`, cannot be narrowed or
revoked, and forces the UI to explain in prose what a row would show. The read arm additionally made
the page's "0 grants across 0 audiences" a lie about a working deployment. The cost of removing it
is the ordering window and the `:289` fallback, both addressed in §2 — the removal is not free, but
it is bounded and each hazard has a loud failure mode instead of a silent one.

**Config row vs. an `open_audience` arm on `AudienceMintPolicy`.** The code arm was the first
design: an `Option<String>` field plus a third arm in `resolve_audience`, wired as
`state.self_service_mint_enabled.then(|| state.default_audience.clone())`. It works and needs no
operator action, which is its only advantage. Against it: it is invisible to every audit surface the
project already has, it is all-or-nothing, it cannot express the read half of the same problem, it
needs `default_audience` plumbed onto a second server state and two new fields on
`/my-audiences` (touching ~35 construction sites and the CLI's client contract), and it puts a
second, differently-shaped authorization rule behind a knob that already governs four things. The
grant row reuses a mechanism that is already tested, already audited, already documented, and
already gated. Chosen for that reason.

**Seed unconditionally, not only into an empty table.** The alternative was to guard the INSERT
with `... WHERE NOT EXISTS (SELECT 1 FROM audience_grants)`, seeding only a deployment that has
never expressed any grant policy. Rejected: "your default depends on whether you ever touched this
table" is unpredictable, invisible in the resulting state, and makes two deployments on the same
version behave differently for reasons nobody can see afterwards. Since the seed codifies what is
already true of live deployments rather than changing it (*Security*), there is nothing for the
guard to protect.

**The residual gap: one admin action.** The issue's framing — "the documented onboarding recipe
[should] work as written for a non-admin operator" — is met one action later than the code arm would
meet it — **unless** the seed ships, which is why the seed is in the plan. With it, a fresh
deployment with only `MICROMEGAS_SELF_SERVICE_MINT=true` works with no admin action at all, which is
the code arm's only real advantage, obtained without the code. What remains manual is the *custom
default* case (a deployment running `MICROMEGAS_DEFAULT_AUDIENCE=unassigned`), and that is a
deployment which has already opted into an isolation posture and should be making the call
explicitly.

**`--claim NAME` rather than `--audience X --claim`.** A boolean modifier keeps `--audience`
overloaded with two meanings, which is the shape that produced the bug. A separate value flag makes
each flag mean one thing, and makes the error message a literal command the user can paste. Cost:
`--audience <fresh-name>` scripts must be edited — the breaking half of the change, called out as
such, with the replacement named in the error. No compatibility window or deprecation period for
this: `micromegas-setup-telemetry` and its `--audience`-prefixing rule have never shipped in a
release (`## Unreleased` in `CHANGELOG.md`), so there is no installed base to stage a rollout for —
the repo's own precedent for exactly this situation (`CHANGELOG.md:87`, the `MICROMEGAS_UNSTAMPED_AUDIENCE`
removal) is to ship the break directly and say so in the existing Unreleased entry, not to open a
warn-only period for behaviour nobody has depended on yet.

**`'*'` on `mint` is a real widening, unlike `'*'` on `read`.** A `*` read row exposes data; a `*`
mint row lets any authenticated caller obtain a standing write credential for that audience. For
the *deployment default* this is close to a no-op (see *Current State* — those callers can already
write there interactively), which is why the recipe is scoped to the default and the docs should
not present `create <anything> mint '*'` as a general pattern.

## Security

**The seed codifies the status quo; it does not widen it.** Deployments running today are fully
open — every audience mechanism shipped so far is a *forward*-looking isolation seam, and no live
deployment is relying on `public` being un-mintable to keep anything apart. Seeding
`('public','mint','*')` writes down what is already true rather than changing a posture anyone
holds. Reinforcing that:

- A knob-**off** deployment is unaffected regardless: `MintGate` (`ingestion_keys.rs:293-319`)
  rejects every non-admin before `mint_key` runs, so the row is inert. That is the default.
- A knob-**on** deployment has already opted into non-admin self-service mint, and its callers can
  already write into `public` interactively (see above) — what they gain is the standing credential.
- A deployment with a custom `MICROMEGAS_DEFAULT_AUDIENCE` is untouched by the seed itself: it names
  `public` literally, and their unaudienced writes do not land there. But the §1 recipe such a
  deployment follows to open its own default carries a real widening if followed carelessly: pairing
  the mint row with a blanket `read '*'` companion turns the custom default into a second `public`,
  undoing the isolation the custom default was chosen for. §1 recommends per-user/per-group read
  grants instead, precisely to keep that posture intact.
- The row is visible on the Audience Access page immediately after upgrade, labelled `default`, and
  removable from that same page.

So this is an ordinary CHANGELOG entry naming the new row and how to remove it — not an upgrade
warning.

**Removing the built-in read grant is fail-closed, not fail-open.** Every way it can go wrong
(missing seed, un-migrated DB, an embedder's unset policy) results in *less* access, never more:
queries return nothing. That is the right direction for a confidentiality control, and it is why the
work in §2 is about making those failures loud (a startup refusal below v9, a real default on the
injected-provider branch) rather than about containing an over-grant. The one thing to verify in
review is that no path can resolve an *empty* readable set and have a caller interpret it as
`ReadScope::All` — `ReadPolicy`'s contract already forbids that softening, and the disabled-auth
branch reaches `ReadScope::All` through the absent-extension convention, never through a resolved
empty set.

The mint delta is otherwise a **standing credential** for write authority that already exists
interactively.
An OIDC-authenticated non-admin can already write into the deployment default via
`resolve_write_audience`; the row lets them mint a key for it, with a different lifetime and
revocation story than a session token. That is why `revoke_key` staying `AdminUser`-gated matters,
and why the row is an explicit operator action rather than implied by the knob.

One caller class genuinely gains authority: **a caller with no email**
(client-credentials / analytics API key, `AuthContext.email == None`). Today, knob-on, they can mint
nothing — no grant, and the lazy claim needs an email to write its `user:<email>` row. With a `*`
mint row they can mint into the default. This is deliberate: it is what makes "N per-sender service
keys under the shared default audience" work non-interactively, which is the issue's stated use
case. `max_keys_per_caller` still bounds them — `ingestion_keys.rs:357` keys the count on
`caller.email.unwrap_or(caller.subject)`.

Unchanged: the read path (`is_admin` is never a read bypass), the default's *unclaimability*
(`ingestion_keys.rs:715-735` — mintable and claimable stay distinct), the mint-is-DB-only rule, and
the placeholder-row guidance for names that exist only in `{prefix}_AUDIENCE_GRANTS`.

## Documentation

- **`mkdocs/docs/admin/authentication.md`**, *Self-service ingestion key mint*: new subsection —
  the seeded `('public','mint','*')` default, why the knob still gates it, how to remove it, the
  custom-default case (add your own mint row; warn against pairing it with a blanket `read '*'`,
  which turns the custom default into a second `public`, and point to per-user/per-group read
  grants instead for a deployment that needs callers to read back what they wrote), and a note
  that a `*` mint row is a deliberate choice for the shared default rather than a general pattern.
  Also update the existing "before turning on the knob, pre-create a
  placeholder row" checklist (`:541-560`), which now overlaps the seed for `public`. Amend the existing
  "`public` and the deployment's own `MICROMEGAS_DEFAULT_AUDIENCE` can never be claimed" sentence to
  keep *claimable* and *mintable* visibly distinct. Update the two `micromegas-setup-telemetry`
  examples: the fresh-claim one becomes `--claim ci-runner`, plus an `--audience public` example.
- **`mkdocs/docs/admin/api-keys.md`**: update the naming-convention paragraph (`:281-295`) for
  `--claim`.
- **`mkdocs/docs/admin/web-app.md`**, *Audience Access* (`:193-204`): the Add grant dialog's
  Mint + Everyone combination as the way to open the deployment default, and that `public`'s empty
  Mint column does not mean "no grant needed" the way its read scope does.
- **`mkdocs/docs/query-guide/python-api.md:924-975`**: rewrite the `--audience` bullet list for the
  new rule, document `--claim`, document the `held_pairs`-based auto-resolution, and replace "There
  is no flag to bypass this prefixing" — false in letter now (there is a flag to *request* it),
  still true in spirit (a non-admin still cannot mint a bare name here).
- **`mkdocs/docs/admin/authentication.md`**, grant-model section: `public` read is no longer a
  built-in — it is a seeded row like any other, and removing it removes public read. This is the
  most consequential doc change here; anywhere the docs say public read needs no grant is now wrong.
- Per `CLAUDE.md`, none of the new prose cites issue numbers or stage labels.

## Testing Strategy

**Migration** (`rust/ingestion/tests/sql_migration_test.rs`, live-DB, `#[ignore]`d): add a
`build_v8_schema` helper alongside the existing `build_v5_schema`/`build_v6_schema`/`build_v7_schema`
chain, then: a DB migrated from that pre-v9 snapshot through `execute_migration` ends with exactly
the two seeded `('public','read','*')` / `('public','mint','*')` rows and
`LATEST_DATA_LAKE_SCHEMA_VERSION`; running `execute_migration` twice is a no-op (the `ON CONFLICT`);
a DB where an operator already created either row by hand migrates cleanly and does not duplicate
or overwrite its `created_by`.

**Read side** — `rust/auth/tests/policy_tests.rs`, after the built-in arm is gone:

- an empty grant map with no store resolves to the **empty** set for a grantless caller (the
  inverse of today's `read_policy_grantless_caller_resolves_to_exactly_public`)
- a grant map naming `{"public": ["*"]}` resolves to `{public}` for any caller — the env-map path
- a store snapshot containing `('public','read','*')` resolves to `{public}` — the production path
  (`db_audience_grants_tests.rs` is where a store-backed case belongs)
- `read_audiences` still folds in independently of `public`
- `flight-sql-srv` refuses to start against a pre-v9 schema, with the message naming the migration

**Mint side** — `rust/auth/tests/policy_tests.rs` (no DB):

- `"*"` on the mint axis grants mint to a caller with no email, no groups, `is_admin: false`
- the same policy denies a *different* audience (the row widens exactly one name)
- `mint_policy_public_is_not_mintable_by_default` unchanged, doc comment clarified

**`rust/analytics-web-srv/tests/ingestion_keys_tests.rs`**, `#[ignore]` live-DB section: the
end-to-end case in Implementation Step 9, plus knob-off → 403 from `MintGate` with the row present
(the row is inert until the operator opts in) — this is
`live_mint_rejects_a_non_admin_claim_of_the_public_audience` repurposed, since its current
knob-on/403 assertion is exactly what the new e2e case supersedes. Both `public` cases must skip
the section's usual `cleanup_audience(&pool, &audience)` teardown — run against `"public"` it
deletes the v9-seeded rows from the shared dev database.

**`python/micromegas/tests/cli/test_setup_telemetry.py`**:

- `--audience public` with `"public" in audiences` → verbatim, nothing on stderr
- `--audience prod`, non-admin, not in `audiences` → `parser.error`, message contains both
  `micromegas-grants` commands with the audience and email substituted. This is the regression test
  for the reported bug; it replaces
  `test_fresh_audience_non_admin_is_prefixed_and_announced_to_stderr`
- `--claim ci-runner`, `mint_prefix == "alice-"` → `"alice-ci-runner"`, announced on stderr
- `--claim` with `mint_prefix is None` → error; `--claim` as an admin → error;
  `--audience` + `--claim` together → error
- omitted, `audiences == ["public", "team-alpha"]`, `held_pairs == ["team-alpha:mint"]` →
  `"team-alpha"` silently (§4's headline case)
- omitted, `audiences == ["public"]`, `held_pairs == []` → error naming `--audience public`
- existing omitted-`--audience` and admin cases otherwise unchanged

**`analytics-web-app`**: the Add grant dialog's Mint/Everyone path is already covered
(`AudienceAccessPage.test.tsx:176-187` asserts the Everyone default submits `'*'`; `:285-297` asserts
Everyone is absent from the non-admin Share dialog) and needs only a fix to any snapshot or text
assertion the §5 legend edit disturbs. The Mint dialog's default selection is new behaviour and
needs its own cases: opening the dialog with `me.audiences == ['public', 'team-alpha']` and
`held_pairs == ['team-alpha:mint']` defaults `audienceChoice` to `'team-alpha'`, not `'public'`;
with `held_pairs == []` (only the seeded `public` row visible) it defaults to `'__new__'`; a
`prefillAudience` still wins over both.

**Manual**, against `local_test_env` with `MICROMEGAS_SELF_SERVICE_MINT=true`: on a freshly
migrated DB, confirm the Audience Access page shows both seeded rows under `public` (Read and Mint,
`default` attribution) and that ordinary queries still return data — i.e. the read path now runs
through the row rather than the deleted arm. Then, as a non-admin OIDC login, run
`micromegas-setup-telemetry --audience public` and confirm the key's `audience` column reads
`public`; send OTLP with it and confirm the process is visible to a plain `public` reader through
`micromegas-query`. Then remove the Mint row from the same page and confirm the CLI errors with the
suggested command instead of minting `jane-doe-public` — i.e. that removing the default actually
restores per-caller isolation. Removing the **Read** row is the sharper check: public data should
disappear from an ordinary caller's queries, proving the row is genuinely what grants it.

## Open Questions

1. ~~**Is the unconditional seed on upgrade acceptable?**~~ Resolved: yes. Live deployments are
   fully open today, so the seed records the existing posture rather than changing one. No
   conditional guard, no upgrade warning.

None outstanding. (The schema-floor and CLI-compatibility-window questions previously listed here
are resolved in §2 and *Trade-offs* respectively, and reflected in Implementation Steps 4 and 13.)
