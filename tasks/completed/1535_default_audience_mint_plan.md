# Let a Non-Admin Mint a Key Bound to the Default Audience Plan

Issue: [#1535](https://github.com/madesroches/micromegas/issues/1535)

## Overview

The issue asks for two things. **(1)** Let a non-admin mint into the deployment default audience
(`MICROMEGAS_DEFAULT_AUDIENCE`, `public` when unset). **(2)** Stop
`micromegas-setup-telemetry` from silently rewriting an explicit `--audience X` it believes the
caller cannot mint into `{mint_prefix}X`. The replacement flag, `--claim`, does not prefix either:
the name passed is the name claimed, and namespacing moves into the documented invocation
templates.

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

- `AudienceReadPolicy::resolve` (`policy.rs:513`) inserts `PUBLIC_AUDIENCE` unconditionally.
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
self-service reach.

**That rationale does not hold, and the docs overstate it.** The prefix is composed entirely in this
one Python client. `try_claim_and_mint` (`rust/analytics-web-srv/src/ingestion_keys.rs:709-735`)
rejects exactly two names — `PUBLIC_AUDIENCE` and `state.default_audience` — then applies
`max_claims_per_caller`; it has no notion of a caller namespace and never inspects `mint_prefix`.
Any authenticated non-admin who posts to the mint route directly claims bare `prod` today. So the
prefix buys naming hygiene, not a boundary: it keeps casual script users from colliding in a flat
namespace, and nothing more. If land-grab protection is actually wanted it has to be enforced
server-side, which is out of scope here — but the plan must not carry the claim that the client
already provides it.

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

`created_by = 'default'` rather than a user identity, so the rows read as shipped, not added by a
colleague; the sentinel shares a namespace with caller identities, and that overlap is accepted —
see *Security*. `ON CONFLICT DO NOTHING` because an operator who already created
either row by hand (the path the issue describes for `mint`, and the `{"public": ["*"]}` env entry
the docs say "changes nothing" for `read`) must not fail the migration.

**Literal `public`, not `MICROMEGAS_DEFAULT_AUDIENCE`.** A migration that reads an env var bakes in
whatever the variable happened to be at upgrade time and silently goes stale when it changes. A
deployment with a custom default gets the literal `('public','mint','*')` row regardless. Its callers
gain a standing write path into `public` they did not have (see *Security*); that is accepted, not
special-cased — the seed means one thing everywhere, and a deployment that wants otherwise deletes
the row and adds its own mint row for its actual default.

The row is then an ordinary grant, so both shipped admin surfaces manage it with no new UI or CLI
work. To **remove** it — Audience Access → `public` → Mint → Remove, or:

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

A `read '*'` companion is a separate decision, not an automatic one: it makes the custom default
readable by every authenticated caller, i.e. a second `public`. Per-user or per-group read grants
(`create unassigned read 'user:<email>'`) cover the read-back case without that.

A deployment with a custom default and the knob on that skips the mint row above has no mintable
default and will hit the CLI error (§4) until it does.

**No `rust/` policy code changes** — only the migration. Why a row beats an implicit
`open_audience` arm on `AudienceMintPolicy` is argued once, in *Trade-offs*. The cost is that the
default now has to be *chosen* rather than left implicit, and applied to existing deployments by a
migration — see *Security*.

**Env-map config cannot do this.** `MICROMEGAS_AUDIENCE_GRANTS` is never consulted on the mint
route — `mint_key` builds its `AudienceGrants` purely from the DB point query. This matches the
existing documented rule that self-service mint grants must live in the DB table
(`authentication.md:492-500`), and the docs should not suggest the env map here.

### §2 — Delete the built-in public read grant

One line goes (`policy.rs:513`):

```rust
set.insert(PUBLIC_AUDIENCE.to_string());   // deleted
```

`PUBLIC_AUDIENCE` stays as a const — `default_audience_from_env` (`:94`) and
`ingestion_keys.rs`'s reserved-name check still use it. `policy.rs:513` is the **only** place the
read path hardcodes it; nothing in `rust/analytics/src` or `rust/public/src` does. The resolved set
becomes exactly:

```text
  { a : "*" | "user:<email>" | "group:<g>" ∈ grants[a].read }   env map
∪ { a : selector ∈ store.readers(a) matches caller }            DB store
∪ caller.read_audiences                                         per-key direct grant
```

Doc comments asserting the built-in as intended design need correcting — most notably
`AudienceGrants` (`:242-244`, "`public` is not stored here … (though writing one changes nothing)":
it *is* stored now, and writing one is how it works) and `PUBLIC_AUDIENCE`'s own comment
(`policy.rs:31`: every-principal readability now comes from the seeded row, not the const). Step 2
enumerates every site; `ownership_rewrite.rs:215`'s should also note the empty resolved set is now
reachable in production.

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

**The split-deployment ordering window — not guarded.** Only `telemetry-ingestion-srv` and
`micromegas-monolith` run migrations (`connect_to_remote_data_lake` → `migrate_db`);
`flight-sql-srv` and `analytics-web-srv` do not. So a split deployment that upgrades flight-sql-srv
*before* the ingestion binary has applied v9 would get new code reading a v8 database: no seeded
row, no built-in arm, every query empty until the migration lands.

**No startup schema floor is added for this.** The skew failure is fail-closed (empty result sets,
never leaked rows), and this repo's norm for skew is a runtime message
(`default_provider.rs:130-137`), not a refusal to boot.

### §3 — CLI: `--audience` means what it says, `--claim` claims

`python/micromegas/micromegas/cli/setup_telemetry.py`. New flag, mutually exclusive with
`--audience`:

```
--claim NAME    Claim NAME as a fresh audience, verbatim. Fails if it already
                exists and you hold no grant for it.
```

**`--claim` does not prefix.** The name passed is the name claimed. The CLI's real callers are
scripts and agent skills, not people at a prompt, and for those a silent rewrite is strictly worse
than for a human: the input stops determining the output, so the caller has to read back
`MintResponse.audience` (`ingestion_keys.rs:276`) to learn what it got. Verbatim keeps the resolved
name predictable *before* the call. Namespacing moves into the documented invocation templates
(§ Documentation), where the convention is visible in the line the user copies rather than applied
behind their back.

The web app keeps composing (`AudienceAccessPage.tsx:376,479`) — it renders the composed name live
before commit, which a CLI cannot. `mint_prefix` stays on `MyAudiencesResponse`, since
`AudienceAccessPage.tsx:375` needs it and the CLI still uses it to suggest a namespaced name in the
zero-match error.

`resolve_audience`'s new rule:

| Input | Behaviour |
|---|---|
| both `--audience` and `--claim` | `parser.error` |
| `--claim NAME`, admin caller | `parser.error` — an admin's brand-new audience is claimed server-side; use `--audience NAME` |
| `--claim NAME`, no `email` | `parser.error` — no email to claim with |
| `--claim NAME` | `NAME` verbatim, announced on stderr |
| `--audience X`, `X in audiences` | `X` verbatim (unchanged — and this is the branch `public` lands in once the grant row exists) |
| `--audience X`, admin | `X` verbatim (unchanged) |
| `--audience X`, otherwise | `parser.error` (**was**: silent `{mint_prefix}X`) |
| `--audience`/`--claim` both omitted | see §4 |

The final error names the reason and every way forward:

```
cannot mint audience 'public': it is not in this caller's mintable set
  mintable audiences: team-alpha
  to claim a fresh audience of your own: --claim alice-public
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

The suggested name is still namespaced — the CLI composes `{mint_prefix}{args.audience}` for the
hint — but it is a suggestion in text the caller can edit, not a rewrite of what they typed. When
`mint_prefix` is `None` the hint degrades to `--claim <new-name>`.

**What claiming a bare name now does.** `--claim prod` is passed through, so the outcome is
whatever the route already decides: a 403 `"audience 'prod' already exists and the caller has no
grant for it"` if someone holds it, a successful claim against `max_claims_per_caller` if it is
genuinely free. That is a real behaviour change for this script, and it is the honest one — the
route has always accepted bare names from an authorized non-admin, and the old rewrite only hid
that from users of this one client. The CLI should surface the 403 verbatim rather than
re-interpreting it.

**The `--claim` precondition is `email`, not `mint_prefix`.** Building the name no longer needs the
prefix; what still needs an identity is the server's lazy claim, which writes a `user:<email>` grant
row. Guarding on `my_audiences["email"] is None` tests that directly. The two differ: `mint_prefix_for`
also returns `None` when the local part sanitizes to empty (`audience_grants_tests.rs:356` —
`+++@example.com`), a caller who has an email and can now claim fine.

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
  personal]`) — visible-but-not-personally-held audiences the caller could pass explicitly. This is
  the right outcome: a caller with no audience of their own should not silently publish into the
  shared pool because they omitted a flag. The visible-audience
  hint is new text; the error's existing claim advice also has to change, from `--audience
  <new-name>` to `--claim <new-name>` — under §3, `--audience` for a name outside the caller's
  mintable set is now a hard error, so pointing the caller at it here would send them straight into
  that error.
- Admins are unaffected: `held_pairs` is always empty for an admin, and an admin must already pass
  `--audience` explicitly.

Behaviour change to declare: a deployment that already has a `*` mint row on some audience and
relies on auto-resolution picking it will now get the zero-match error instead. This is niche (it
requires a pre-existing `*` mint row, which nothing in the docs currently recommends) and moves in
the same direction as the rest of the change — explicit beats implicit for a shared audience.

### §5 — The page legend stops describing a special case, and the Mint dialog explains the choice

The Audience Access page's legend (`AudienceAccessPage.tsx:842-846`) currently reads:

> **Scope:** `public` is always readable by every authenticated principal — no row needed.

That sentence is the page-level version of the bug: it tells an operator `public` needs no row,
which was true for read and never true for mint. After §1 and §2 it is simply false — `public` has
two rows, and they are what grant its access. Replace it, without claiming the rows are listed for
the current viewer — `visible_grants` (`audience_grants.rs:672-720`) strips `"*"` from
`caller_selectors` on both non-admin branches, so a non-admin viewer never sees the seeded
`('public', axis, '*')` rows themselves, only their effect:

> **Defaults:** `public` ships with Read and Mint grants for everyone (attributed to
> `default` on the admin view). They are ordinary grants: removing the Read row stops public data from being
> universally readable; removing the Mint row limits minting into `public` to admins.

The legend gets shorter and stops describing a special case, because after this change there isn't
one — every grant on the page is a row on the page, for an admin. That is the real payoff of §2, and
this is where a reader sees it — as an admin; a non-admin still cannot see either seeded row (only
their effect on what audiences they can read and mint), which is unchanged from today.

**Three more strings on the same page assert the same now-false special case.** The admin empty
state (`:931-932`) is still reachable — a pre-v9 DB, or an operator who has deleted both rows — and
its copy is pinned by two tests in `AudienceAccessPage.test.tsx` (~:164, ~:253); reword it (no
grants at all now means nothing is readable or mintable by a non-admin) rather than dropping it.
The non-admin empty state (`:941-942`, "You hold no audience grants. You can read `public`.") gets
the same rephrasing, as `public`'s Read grant rather than a hardcoded exception. The per-audience
note (`:993-994`, "No read grants — only `public` and any env-map grants apply here") is now simply
wrong for any other audience with no read rows — it needs correcting, not just rewording.

**A fifth string, on a different page, states the same effect as a permanent one.** The shared
API-keys mint dialog's audience hint (`analytics-web-app/src/components/ApiKeysAdminPage.tsx:235-237`)
reads "`public` is readable by every authenticated principal; use a named audience to restrict it".
That stays *true* after §1/§2 — the seeded Read row preserves exactly this effect — so this is not a
false claim to correct. It goes stale only in a deployment whose operator deletes that row, which
*Manual* has them do. Align the copy so it reads as a consequence of the grant rather than a
property of the audience; the dialog is admin-gated, so behaviour is unaffected either way. Optional
polish, not a correctness fix.

**The Mint dialog keeps its default and gains a help line.** The dialog's `useEffect`
(`AudienceAccessPage.tsx:361-362`) seeds `audienceChoice` from `me.audiences[0]` with no prefill,
and the `<select>` (`:450`) lists `me.audiences`, sorted server-side. Once the seed lands, `public`
is in every non-admin's `audiences`, and the dialog opens on whichever entry sorts first — `public`
for a caller who holds nothing else, and for anyone whose own audiences sort after it (personal names
are `{email-local-part}-…`, so a local part starting p–z lands there). That default stays: the value is visible before commit, the name
says what it is, and changing it is one interaction — unlike the CLI's silent auto-resolution, which
is why §4 guards that path and this one needs no equivalent.

What the field does lack is any statement of the alternative. Add one help line under the
`<select>`, shown to non-admins:

> `public` is readable by every authenticated user. Pick *New audience…* to give this key's data its
> own audience, with read access managed separately.

"Managed separately" is deliberate over "keep it to yourself" — a claimed audience is not a private
hole, it is a scope whose readers are granted afterwards. The passive matters too: the claimer grants
via Share, but so can an admin, so the copy does not pin the managing to the caller. The line names
the trade without promising exclusivity the model does not offer.

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
   built-in-read-grant doc comments at `:242-244` (`AudienceGrants`), `:251`
   (`AudienceGrants::empty()`), `:440-461` (`AudienceReadPolicy`), `:482`
   (`AudienceReadPolicy::from_env`), `:31` (`PUBLIC_AUDIENCE`), and `:539` (`AudienceMintPolicy`);
   plus the same assertion in `rust/analytics/src/lakehouse/ownership_rewrite.rs:215`,
   `rust/monolith/src/main.rs:250-252`, `rust/public/src/servers/flight_sql_server.rs:146`
   (`with_read_policy`'s builder doc), and `rust/auth/tests/policy_tests.rs:676`. Also annotate
   `tasks/data_isolation/audience_based_access_control_plan.md` at `:195`, `:322`, `:341`, `:696`,
   `:751`, `:1428` with its existing `Superseded by …` convention: the built-in read grant is
   superseded by the v9 seeded `('public','read','*')` row.
3. `rust/public/src/servers/flight_sql_server.rs:282-291`: give the injected-provider branch the
   same env+store-backed default the `use_default_auth` branch builds at `:311-321`. Add a comment
   on the disabled-auth branch (`:333`) noting that its never-resolved property is now load-bearing.
4. `rust/auth/tests/policy_tests.rs`: update the read-side assertions that assume the built-in arm
   (`read_policy_public_is_always_present` `:89-95`, `read_policy_grantless_caller_resolves_to_exactly_public`
   `:148-157`, `read_policy_read_audiences_folds_into_the_read_axis` `:161-180`) to supply `public`
   through a grant map instead. They get better in the process: they will exercise the configuration
   production actually runs. `:615-620` (`from_env_with_unset_var_resolves_to_public_only`) pins an
   *unset* env var, not the built-in arm — supplying a grant map there would destroy that premise, so
   instead change its assertion to the empty set and rename it accordingly. `:685-690`
   (`default_audience_from_env_neither_var_set_is_public`) asserts `default_audience_from_env(...) ==
   PUBLIC_AUDIENCE` and builds no read policy at all, so it needs no change here — its doc comment at
   `:676` is already covered by step 2.

**Phase 3 — CLI**

5. `python/micromegas/micromegas/cli/setup_telemetry.py`: add `--claim`; rewrite `resolve_audience`
   per §3's table; add §4's `held_pairs` filter to the omitted branch, including rewriting its
   zero-match error's claim advice from `--audience <new-name>` to `--claim <new-name>`; drop the
   unused `client` parameter; update `--audience`'s help text, the function docstring, and the
   module docstring.
6. `python/micromegas/tests/cli/test_setup_telemetry.py`: update existing cases for the new
   signature and add the new ones (see *Testing Strategy*). This also touches the shared fixtures —
   `make_args`'s defaults gain a `claim` key and `FakeClient`'s default `my_audiences_result` gains
   a `held_pairs` key, since §4 reads `my_audiences["held_pairs"]` directly — and converts
   `test_run_non_admin_claim_does_not_call_create_audience_grant`, which currently drives `run()`
   with a non-admin, `audiences: []`, and `--audience laptop`, to the `--claim laptop` path (its
   `--audience` input is now a hard `parser.error` under §3).

**Phase 4 — pin the mechanism**

7. `rust/auth/tests/policy_tests.rs`: add `mint_policy_wildcard_selector_grants_mint_to_any_caller`.
   Nothing currently asserts `"*"` on the **mint** axis at all — it is only exercised on `read`.
   That behaviour is now load-bearing for a documented operator recipe, so it should be pinned
   rather than left as an emergent property of `selector_matches`. Amend the doc comment on
   `mint_policy_public_is_not_mintable_by_default` to say it pins "from an empty grant map", so the
   two tests read as complementary rather than contradictory.
8. `rust/analytics-web-srv/tests/ingestion_keys_tests.rs`:
   `live_mint_rejects_a_non_admin_claim_of_the_public_audience` (`:1094-1120`) starts failing once
   the v9 seed lands (the policy now grants the mint). Repurpose it into the knob-off → 403 case
   (same request, non-admin, `self_service_mint_enabled: false`), keeping the pin that `MintGate`
   rejects before the policy is ever consulted. Do not create or delete anything on `public` —
   `cleanup_audience(&pool, "public")` would delete both v9-seeded rows from the shared dev
   database. No new end-to-end case: grant-row → 201 → no-claim is already pinned by
   `live_mint_succeeds_for_non_admin_with_a_matching_grant_no_claim_attempted`, `"*"` on the mint
   axis by step 7's unit test, and the seeded rows by the migration test.

**Phase 5 — the page legend and the Mint dialog help line**

9. `analytics-web-app/src/routes/AudienceAccessPage.tsx`: extend the **Scope:** legend line
   (`:842-846`) per §5, and fix its three companion strings — reword the admin empty state
   (`:931-932`, still reachable — see §5), rephrase the non-admin empty state (`:941-942`), and
   correct the per-audience no-read-grants note (`:993-994`). Check whether
   `analytics-web-app/src/routes/__tests__/AudienceAccessPage.test.tsx` asserts on any of that
   text — it already pins the admin empty state's copy in the two tests named in §5. Also reword
   `analytics-web-app/src/components/ApiKeysAdminPage.tsx`'s shared mint-dialog audience hint
   (`:235-237`) per §5's fifth string — `public` readable via its seeded Read grant, not built in.
10. `analytics-web-app/src/routes/AudienceAccessPage.tsx`: add the §5 help line under the Mint
   dialog's audience `<select>` (`:450`), for non-admins only. The dialog's default selection,
   `prefillAudience` precedence, and the admin path are all unchanged.

**Phase 6 — docs and changelog**

11. `mkdocs/docs/admin/authentication.md`, `admin/functions-reference.md`, `admin/api-keys.md`,
   `admin/web-app.md`, `query-guide/python-api.md` — see *Documentation*. Also correct the two
   remaining stale `mint_prefix` doc strings this reword touches:
   `rust/analytics-web-srv/src/audience_grants.rs`'s `mint_prefix_for` doc comment (`:725-739`) and
   `python/micromegas/micromegas/web_client.py`'s `my_audiences()` docstring (`:180-192`) — both
   currently say the prefix is what a fresh claim is minted under; correct that and add `held_pairs`
   to the documented response keys in the latter.
12. `CHANGELOG.md` — three things: the schema-v9 seeded default, naming the row and how to remove
   it; the built-in public-read-grant removal (public read now requires the seeded
   `('public','read','*')` row), with its own **Deploy-order requirement** paragraph — roll v9
   before or with the `flight-sql-srv`/`analytics-web-srv` upgrade, mirroring the existing v7
   `audience_grants` entry's deploy-order paragraph for the same flight-sql-srv-vs-`migrate_db`
   skew (`CHANGELOG.md:69`); and an amendment to the
   existing `## Unreleased` `micromegas-setup-telemetry` entry describing the new `--audience`/`--claim`
   split in place (no **Minor breaking change** clause — that entry has never shipped in a release,
   so there is no compatibility window to call out; see *Trade-offs* and the repo's own precedent
   for this exact situation, `CHANGELOG.md:87`).

## Files to Modify

| File | Change |
|---|---|
| `rust/ingestion/src/sql_migration.rs` | schema v9: seed `('public','read','*')` + `('public','mint','*')` |
| `rust/ingestion/tests/sql_migration_test.rs` | `build_v8_schema` fixture; v9 seed assertions |
| `rust/auth/src/policy.rs` | delete the built-in `PUBLIC_AUDIENCE` read insert; doc-comment sweep (step 2) |
| `rust/analytics/src/lakehouse/ownership_rewrite.rs` | doc-comment sweep (step 2) |
| `rust/monolith/src/main.rs` | doc-comment sweep (step 2) |
| `rust/public/src/servers/flight_sql_server.rs` | real env+store default on the injected-provider branch; doc-comment sweep (step 2) |
| `rust/auth/tests/policy_tests.rs` | read-side assertions supply `public` via a grant map; pin `"*"` on mint; doc-comment sweep (step 2) |
| `tasks/data_isolation/audience_based_access_control_plan.md` | six `Superseded by` annotations (step 2) |
| `python/micromegas/micromegas/cli/setup_telemetry.py` | `--claim`; `resolve_audience` rewrite; `held_pairs` filter |
| `python/micromegas/tests/cli/test_setup_telemetry.py` | updated + new cases |
| `rust/analytics-web-srv/tests/ingestion_keys_tests.rs` | repurpose the public-claim live case to knob-off → 403 (step 8) |
| `analytics-web-app/src/routes/AudienceAccessPage.tsx` | extend the **Scope:** legend line and its three companion strings (admin/non-admin empty states, per-audience note); add the Mint dialog's audience help line (§5) |
| `analytics-web-app/src/routes/__tests__/AudienceAccessPage.test.tsx` | fix any snapshot/text assertion the legend edit disturbs; one help-line visibility case (non-admin sees it, admin does not) |
| `analytics-web-app/src/components/ApiKeysAdminPage.tsx` | reword the shared mint dialog's audience hint (§5) — public readability is a removable grant, not intrinsic |
| `mkdocs/docs/admin/authentication.md` | the Mint/Everyone recipe; mint-vs-claim distinction; updated script examples; v9 deploy-order note; built-in-read-grant correction |
| `mkdocs/docs/admin/functions-reference.md` | correct `list_audience_grants()`'s "none of them appear here" claim about `public` — seeded rows appear only for an admin caller |
| `mkdocs/docs/admin/api-keys.md` | `--claim` in the naming-convention paragraph |
| `mkdocs/docs/admin/web-app.md` | Audience Access section: opening the default from the Add grant dialog |
| `mkdocs/docs/query-guide/python-api.md` | `micromegas-setup-telemetry` reference |
| `rust/analytics-web-srv/src/audience_grants.rs` | correct `mint_prefix_for`'s doc comment: the prefix is now the web app's claim composition plus the CLI's suggested name, not something the CLI mints under |
| `python/micromegas/micromegas/web_client.py` | correct `my_audiences()`'s docstring: `mint_prefix` is now the web app's claim composition plus the CLI's suggested name, not something a fresh claim is minted under; add `held_pairs` to the documented response keys |
| `CHANGELOG.md` | Unreleased entry |

**No route or mint-policy behaviour change.** `web_server.rs` is untouched, and `AudienceMintPolicy`
gains nothing — the migration adds rows, not columns (no `SCHEMA_VERSION` file-schema concern, no
Arrow schema change). The one `rust/analytics-web-srv/src/*` edit is the `mint_prefix_for`
doc-comment correction above: its claim that the CLI mints fresh audiences under the prefix no
longer holds once §3 lands. The web-app changes are otherwise confined to
`AudienceAccessPage.tsx`: the legend string, and the Mint dialog's help line (§5) — the
Add grant dialog already does everything needed for granting
(`AudienceAccessPage.tsx:265-266, :166`). The *separate* ingestion-key mint dialog rendered by
`ApiKeysAdminPage.tsx` (behind `AuthGuard requireAdmin` on `IngestionApiKeysPage.tsx:47`) needs only
its audience hint reworded (§5) — its behaviour is unaffected.

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

**`--claim` claims verbatim, at the cost of a flat namespace.** Prefixing was the only thing keeping
casual users of this script from competing for names like `ci` and `prod`, and dropping it means the
first caller to claim a good name keeps it. Accepted, for two reasons: it was never a boundary — the
route accepts bare names from any authorized non-admin regardless of client — so the protection was
illusory for anyone not using this script; and the script's callers are automation, for which a name
that survives the round trip unchanged is worth more than collision avoidance. A collision is a loud
403, not silent breakage. Real enforcement, if wanted later, belongs in `try_claim_and_mint` where it
would apply to every client.

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
holds. Two caveats:

- The no-widening argument holds exactly where the deployment default *is* `public`. A deployment
  running a custom `MICROMEGAS_DEFAULT_AUDIENCE` gets the same literal `public` mint row, and its
  callers could not previously mint into `public` — so with the knob on, that deployment does gain a
  standing write path into a world-readable audience. Deliberately not special-cased: the seed means
  one thing everywhere, and deleting the row is one command against a row the Audience Access page
  shows. Conditional seeding would make a migration's result depend on an env var, which is worse.
  Only the mint half is at issue — `public` is world-readable in every deployment today through the
  built-in arm §2 removes, so the seeded read row codifies the status quo everywhere.
- **`created_by = 'default'` shares a namespace with caller identities — accepted.** `delete_grant`
  lets a non-admin delete a row whose `created_by` equals their own `caller_identity`
  (`email.unwrap_or(subject)`), and neither claim is shape-validated, so an identity that is literally
  the string `default` could revoke the seeded rows without admin rights. No issuer hands out such an
  identity, and a qualified sentinel would cost the attribution its plainness on the admin view. Worth
  revisiting only if identity shape validation lands.

So this is an ordinary CHANGELOG entry naming the new row and how to remove it — not an upgrade
warning.

**Removing the built-in read grant is fail-closed, not fail-open.** Every way it can go wrong
(missing seed, un-migrated DB, an embedder's unset policy) results in *less* access, never more:
queries return nothing. That is the right direction for a confidentiality control, and it is why the
work in §2 is about making those failures loud (a real default on the
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
  the seeded `('public','mint','*')` default, that the knob still gates it, and how to remove it
  (`micromegas-grants delete public mint '*'`, or the Audience Access page). Keep this short: anyone
  who can mint a key chooses its audience, and a shared audience readable by every authenticated
  user is the obvious default for one. Note in a sentence that a deployment running a custom
  `MICROMEGAS_DEFAULT_AUDIENCE` gets the same literal `public` row and can delete it — no extended
  warning, no recommended posture. Also update the existing "before turning on the knob, pre-create a
  placeholder row" checklist (`:541-560`), which now overlaps the seed for `public` — and replace its
  `micromegas-grants create legacy-default read '*'` example with an identity-selector placeholder
  (e.g. `read 'group:eng'`), so the example agrees with §1's guidance against a blanket read wildcard
  on a custom default. Amend the existing
  "`public` and the deployment's own `MICROMEGAS_DEFAULT_AUDIENCE` can never be claimed" sentence to
  keep *claimable* and *mintable* visibly distinct. Update the two `micromegas-setup-telemetry`
  examples: the fresh-claim one becomes `--claim "$USER-ci-runner"`, plus an `--audience public`
  example.
- **`mkdocs/docs/admin/api-keys.md`**: update the naming-convention paragraph (`:281-295`) for
  `--claim`, and correct `:255-256` ("`public` is the one built-in: ... no grant entry needed in
  either source"), the one other place in the docs beside `authentication.md` that asserts the
  built-in read grant §2 removes.
- **`mkdocs/docs/admin/web-app.md`**, *Audience Access* (`:193-204`): document that `public` ships
  with seeded Read and Mint rows, visible to an admin (attributed to `default`) like any other
  grant on the page — not a special case with an empty Mint column — and the Add grant dialog's
  Mint + Everyone combination as the recipe for opening a *custom* deployment default the same way.
- **`mkdocs/docs/admin/authentication.md`**, *DB-backed audience grants* section, beside the
  existing v7 **Deploy-order requirement** paragraph (`:766-778`): add a matching note for v9's
  seed — the same `flight-sql-srv`/`analytics-web-srv`-vs-`migrate_db` skew window applies (no
  seeded row, no built-in arm, every query returns nothing until the migration lands), pointing to
  the matching CHANGELOG entry the same way the v7 paragraph does.
- **`mkdocs/docs/query-guide/python-api.md:671-674`**, the `my_audiences()` client reference: reword
  `mint_prefix` the same way as `web_client.py`'s own docstring (the web app's claim composition plus
  the CLI's suggested name, not something a fresh claim is minted under), and add `held_pairs` to the
  documented response keys.
- **`mkdocs/docs/admin/authentication.md:655`**, the `/my-audiences` route table row: reword the same
  stale `mint_prefix` sentence ("the caller-derived namespace prefix a fresh claim mints under") the
  same way — this row already lists `held_pairs` among the response keys, so only the `mint_prefix`
  wording needs correcting here.
- **`mkdocs/docs/query-guide/python-api.md:924-975`**: rewrite the `--audience` bullet list for the
  new rule, document `--claim`, and document the `held_pairs`-based auto-resolution. Delete "There
  is no flag to bypass this prefixing" and the self-service-reach rationale under it: the CLI no
  longer prefixes at all, and the claim was never true of the route. Replace it with copyable
  invocation templates that carry the namespacing convention in the visible text — e.g.
  `micromegas-setup-telemetry --claim "$USER-ci-runner"` — since the script's callers are other
  scripts and agent skills rather than people at a prompt. Say plainly that a bare `--claim prod`
  is passed through and fails with the route's 403 if the name is taken.
- **`mkdocs/docs/admin/authentication.md`**, grant-model section: `public` read is no longer a
  built-in — it is a seeded row like any other, and removing it removes public read. This is the
  most consequential doc change here. Two specific sites need correcting, not just the general
  claim: the Query-Enforcement section's "every authenticated caller's readable set always
  includes `public`, regardless of identity — there is no caller kind whose resolved set is
  otherwise empty" (`:207-212`), which states the built-in as an identity-independent invariant
  rather than as "needs no grant" and so needs its own rewording; and
  `mkdocs/docs/admin/functions-reference.md:472` ("`public`, the `MICROMEGAS_AUDIENCE_GRANTS` env
  map, and a per-key `read_audiences` list all also apply and none of them appear here"), which
  claims `list_audience_grants()` cannot show `public`'s effective read access — after §2 `public`
  read *is* a DB-table row, but whether it appears in this function's result still depends on the
  caller: an admin gets `GrantVisibility::All` and sees the seeded rows directly; a non-admin gets
  `GrantVisibility::Held`, which strips `"*"` from `grant_selectors` before binding
  (`list_audience_grants_table_function.rs:151-171`), so the seeded `('public', axis, '*')` rows
  stay invisible to them — they see only the effect, never the row. So the sentence should say the
  seeded `public` rows appear here for an admin caller only, while the env map and per-key
  `read_audiences` stay outside the function's view for everyone.
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
- `read_audiences` still folds in independently of `public`

**Mint side** — `rust/auth/tests/policy_tests.rs` (no DB):

- `"*"` on the mint axis grants mint to a caller with no email, no groups, `is_admin: false`
- the same policy denies a *different* audience (the row widens exactly one name)
- `mint_policy_public_is_not_mintable_by_default` unchanged, doc comment clarified

**`rust/analytics-web-srv/tests/ingestion_keys_tests.rs`**, `#[ignore]` live-DB section: the
repurposed knob-off → 403 case from Implementation Step 8. It must not run `cleanup_audience`
against `"public"` — that deletes the v9-seeded rows from the shared dev database.

**`python/micromegas/tests/cli/test_setup_telemetry.py`**:

- `--audience public` with `"public" in audiences` → verbatim, nothing on stderr
- `--audience prod`, non-admin, not in `audiences` → `parser.error`, message contains both
  `micromegas-grants` commands with the audience and email substituted. This is the regression test
  for the reported bug; it replaces
  `test_fresh_audience_non_admin_is_prefixed_and_announced_to_stderr`
- `--claim alice-ci-runner` → `"alice-ci-runner"` verbatim, announced on stderr
- `--claim ci-runner` with `mint_prefix == "alice-"` → `"ci-runner"` verbatim, **not**
  `"alice-ci-runner"` — pins that the prefix is never applied to what the caller passed
- `--claim` with `email is None` → error; `--claim` as an admin → error;
  `--audience` + `--claim` together → error
- `--audience prod` zero-match error with `mint_prefix == "alice-"` → hint reads
  `--claim alice-prod`; with `mint_prefix is None` → hint degrades to `--claim <new-name>`
- omitted, `audiences == ["public", "team-alpha"]`, `held_pairs == ["team-alpha:mint"]` →
  `"team-alpha"` silently (§4's headline case)
- omitted, `audiences == ["public"]`, `held_pairs == []` → error naming `--audience public`, whose
  claim advice reads `--claim <new-name>`, not `--audience <new-name>`
- existing omitted-`--audience` cases (`test_omitted_audience_non_admin_exactly_one_match_is_used_silently`,
  `..._multiple_matches_is_an_error`, `..._no_matches_is_an_error`) keep their outcomes once their
  `my_audiences` fixtures gain a matching `held_pairs` entry (e.g. `held_pairs: ["team-alpha:mint"]`
  for the first two, `held_pairs: []` for the no-matches case) — without one, §4's personal filter
  reads a missing key and raises `KeyError` instead of reaching the zero/one/many rule at all; admin
  cases are otherwise unchanged (`test_omitted_audience_admin_is_always_an_error` is unaffected — the
  admin arm errors before the filter runs)
- `test_run_non_admin_claim_does_not_call_create_audience_grant` is converted from driving `run()`
  with `--audience laptop` (now a hard error) to `--claim laptop`, asserting the mint call and
  minted audience are the verbatim name rather than a `{mint_prefix}`-composed one

**`analytics-web-app/src/routes/__tests__/AudienceAccessPage.test.tsx`**: the Add grant dialog's
Mint/Everyone path is already covered
(`:176-187` asserts the Everyone default submits `'*'`; `:285-297` asserts
Everyone is absent from the non-admin Share dialog) and needs only a fix to any snapshot or text
assertion the §5 legend edit disturbs. The Mint dialog needs one added case: a non-admin opening
it sees the help line, and an admin does not. No default-selection cases — the default is unchanged,
and `:457` already covers opening the dialog.

**Manual**, against `local_test_env` with `MICROMEGAS_SELF_SERVICE_MINT=true`: on a freshly
migrated DB, confirm the Audience Access page shows both seeded rows under `public` (Read and Mint,
`default` attribution) and that ordinary queries still return data — i.e. the read path now runs
through the row rather than the deleted arm. Then, as a non-admin OIDC login, run
`micromegas-setup-telemetry --audience public` and confirm the key's `audience` column reads
`public`; send OTLP with it and confirm the process is visible to a plain `public` reader through
`micromegas-query`. Then remove the Mint row from the same page and confirm the CLI errors with the
suggested command instead of minting `jane-doe-public` — i.e. that removing the default actually
restores per-caller isolation. Removing the **Read** row is the sharper check: public data should
disappear from an ordinary caller's queries, proving the row is genuinely what grants it. **Restore
both rows immediately afterward** — `micromegas-grants create public read '*'` and
`micromegas-grants create public mint '*'` — since `execute_migration` only runs the v8→v9 arm once
and will not re-seed them on a DB already at v9.

## Open Questions

None.
