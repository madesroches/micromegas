# `setup-telemetry`: `--user-audience`, and a Lazily-Creating `--audience` Plan

Issue: [#1571](https://github.com/madesroches/micromegas/issues/1571)

## Overview

Give `micromegas-setup-telemetry` two audience flags with one meaning each, and make both of
them create the audience lazily when it doesn't exist:

```bash
# The documented recipe: a name namespaced under the caller's own email-derived prefix.
micromegas-setup-telemetry --url ... --name laptop --user-audience claude
#   alice@example.com  ->  alice-claude

# A verbatim name, for an org/team/service audience.
micromegas-setup-telemetry --url ... --name ci-runner --audience build-fleet
```

In both cases the caller never has to know in advance which of three outcomes applies — hold a
`mint` grant on the resolved name and an additional key is minted under it; the name is genuinely
fresh and it is claimed *and* minted; it belongs to someone else and the mint is refused. The
server already decides all three from the audience name alone, so **no Rust change is required**.

`--user-audience` becomes the recipe every doc leads with, because the prefix is composed
server-side from an already-shipped, role-independent pure function. That removes the two
interface warts the issue names: the caller no longer has to hand-compose their own namespaced
name, and the identical command now works for an admin and a non-admin, so no guide has to
classify its reader before its first command.

`--claim` collapses into `--audience` — with the CLI's client-side refusal gone, `--audience` *is*
`--claim`. It stays one release as a hidden, deprecated alias, since it shipped in v0.30.0 and is
published in a blog post's copy-paste block.

## Current State

### The CLI has three audience spellings and two of them are role-gated

`python/micromegas/micromegas/cli/setup_telemetry.py`:

- `resolve_audience` (`:98-190`) branches over `--audience` / `--claim` / neither.
- `--audience X` for a **non-admin** whose `my_audiences["audiences"]` doesn't contain `X` is a
  hard client-side `parser.error(_cannot_mint_hint(...))` (`:152-155`) — it never reaches the
  server, so it never reaches the server's own lazy-claim path.
- `--audience X` for an **admin** is used verbatim (`:152-153`), which for a fresh name means the
  server claims it. So the same command is a refusal for one role and a claim for the other.
- `--claim NAME` (`:139-150`) claims `NAME` verbatim, and is `parser.error`-rejected outright for
  an admin caller on role alone (`:140-145`), ahead of the email check.
- Both omitted (`:166-190`) resolves from the caller's *personally held* mint audiences
  (`held_pairs`), erroring on zero or multiple matches. This path is good and stays.
- `_claim_suggestion` (`:61-68`) already composes `mint_prefix + args.audience` — but only to
  render text inside the `_cannot_mint_hint` error at `:70-92`.

`build_parser` (`:235-286`) declares both flags and documents them as mutually exclusive; the
module docstring (`:11-15`) states the same split.

### The prefix is already computed, already role-independent, already tested

`mint_prefix_for` (`rust/analytics-web-srv/src/audience_grants.rs:782-810`) is pure and sync:
lowercase the email's local part, map every character outside `[a-z0-9_-]` to `-`, collapse runs,
trim, append one `-` as the separator. It takes `&Option<String>` and **never looks at
`is_admin`** — the `my_audiences` handler calls it unconditionally at `:871`
(`mint_prefix_for(&caller.email)`), for every caller.

| email | `mint_prefix` | `--user-audience claude` resolves to |
|---|---|---|
| `alice@example.com` | `alice-` | `alice-claude` |
| `alice.smith+ci@example.com` | `alice-smith-ci-` | `alice-smith-ci-claude` |
| `Alice.Smith@Example.com` | `alice-smith-` | `alice-smith-claude` |

Pinned by `rust/analytics-web-srv/tests/audience_grants_tests.rs:334-364`.

The web app does a similar composition in its Mint dialog, but only for non-admins:
`MintIngestionKeyDialog.tsx:71` is `const prefix = !isAdmin ? me?.mint_prefix ?? null : null`, and
`:72` then composes `const composedNew = prefix ? \`${prefix}${newAudience}\` : newAudience` — an
admin's dialog never prefixes, even though the server returns `mint_prefix` unconditionally
(`audience_grants.rs:902`).

### The server needs nothing

The CLI sends a resolved audience *name*; the server never learns which flag produced it.
`mint_key` (`rust/analytics-web-srv/src/ingestion_keys.rs:437-460`) infers intent only from
whether a name was supplied at all:

```rust
let explicit = body.audience.as_deref().filter(|s| !s.is_empty()).is_some();
```

From there one lazy-create path already covers every case:

```
POST /api/ingestion-api-keys {name, audience}
  └─ AudienceMintPolicy::resolve_audience
       ├─ Ok  (caller holds a `mint` grant, or is admin)
       │    └─ admin + looks unclaimed?  -> try_claim_and_mint      (ingestion_keys.rs:411-430)
       │       otherwise                 -> ordinary insert, claimed: false
       └─ Err (non-admin, no mint grant)
            └─ explicit name + has email -> try_claim_and_mint      (ingestion_keys.rs:437-460)
                 ├─ in-lock EXISTS finds nothing -> claim + mint, claimed: true   (:666-672)
                 └─ EXISTS finds a row           -> 403 "audience \"X\" already exists
                                                    and the caller has no grant for it"
```

The CLI side is in place too: `run()` (`setup_telemetry.py:293-299`) already calls
`client.my_audiences()` unconditionally before minting, and that response carries `mint_prefix`,
`email`, `is_admin`, `audiences`, and `held_pairs`.

### Reserved names, quotas, and contention are unchanged

`public` and `MICROMEGAS_DEFAULT_AUDIENCE` can never be claimed (`ingestion_keys.rs:700-724`);
`max_claims_per_caller` still bounds claims; a concurrent claim of the same fresh name still
yields the transient `409 CLAIM_CONTENDED` that `WebClient.mint_ingestion_api_key` retries once.
The gate is specifically a **mint** grant — a read grant confers none (`rust/auth/src/policy.rs:499-502`).

## Design

### Flag surface

| Flag | Resolves to | Notes |
|---|---|---|
| `--user-audience SUFFIX` | `f"{mint_prefix}{SUFFIX}"` | Errors when `mint_prefix` is `None`. Identical for admin and non-admin. |
| `--audience NAME` | `NAME`, verbatim | No client-side mintable-set refusal any more. |
| neither | single personally-held mint audience | Unchanged logic; error text updated. |
| `--claim NAME` | `NAME`, verbatim | **Deprecated**, `argparse.SUPPRESS`ed, warns on stderr, removed next release. |

`--audience` and `--user-audience` stay mutually exclusive — they are two spellings of one thing.
The check stays a manual `parser.error` inside `resolve_audience` (not
`add_mutually_exclusive_group`) so the existing tests can keep driving `resolve_audience` with a
plain `Namespace` and a `FakeParser`.

### `resolve_audience` after the change

```python
def resolve_audience(args, parser, my_audiences):
    if args.claim is not None:                      # deprecated alias, one release
        if args.audience is not None or args.user_audience is not None:
            parser.error("--claim is a deprecated alias for --audience; pass only one")
        print("warning: --claim is deprecated; use --user-audience <name> "
              "(or --audience <name> for a verbatim name)", file=sys.stderr)
        args.audience = args.claim

    if args.audience is not None and args.user_audience is not None:
        parser.error("--audience and --user-audience are mutually exclusive; pick one")

    if args.user_audience is not None:
        if not args.user_audience:
            parser.error("--user-audience requires a non-empty name")
        mint_prefix = my_audiences.get("mint_prefix")
        if mint_prefix is None:
            email = my_audiences.get("email")
            if email is None:
                parser.error(
                    "--user-audience needs a caller-derived prefix and this caller has no "
                    "email to derive one from; a caller with no email cannot claim a fresh "
                    "audience at all, so ask an admin for a grant instead"
                )
            parser.error(
                "--user-audience needs a caller-derived prefix and this caller's email "
                "sanitizes to empty; pass the whole name with --audience <name> instead"
            )
        return f"{mint_prefix}{args.user_audience}"

    if args.audience is not None:
        return args.audience

    ...  # unchanged held_pairs resolution, error text updated (below)
```

Aliasing `--claim` onto `args.audience` before anything else keeps exactly one code path for a
verbatim name; the alias cannot drift from the flag it aliases.

**No client-side normalization of `SUFFIX`.** No `.strip()`, no case folding, no charset check —
only the empty-string rejection. `is_valid_audience` (`rust/auth/src/policy.rs:46-52`) is
deliberately non-normalizing, and the CLI silently rewriting a name it was handed is precisely the
#1535 bug. An invalid suffix therefore surfaces as the route's ordinary **400**, which is raised
by `resolve_audience` server-side *before* any key row is inserted — a rejected name never strands
a minted key. The empty-suffix case is rejected locally because it is the one input that is
*valid* server-side (`alice-` passes `is_valid_audience`) while obviously not being what the
caller meant.

**No fallback to the bare suffix when `mint_prefix` is `None`.** The web dialog falls back to the
unprefixed name (`MintIngestionKeyDialog.tsx:72`); the CLI errors instead. The flag's entire
contract is "namespaced under me", so quietly minting an un-namespaced global name under it would
be the same class of silent rewrite #1535 removed. `mint_prefix` is `None` for two different
reasons, and the error tells them apart: a client-credentials caller with no email at all can
never claim a fresh audience (server-side, the lazy claim needs an identity to write a
`user:<email>` grant row under), so its error points at asking an admin for a grant; an email like
`+++@example.com` whose local part sanitizes empty (`audience_grants_tests.rs:358-364`) still has
an identity, so its error names `--audience <name>` as the way forward. Both are role-independent
and rare.

### The `_cannot_mint_hint` text moves from a pre-flight refusal to a post-403 enrichment

Dropping the client-side guard is what makes `--audience` lazily creating, but the hint that guard
rendered — the caller's mintable audiences, a fresh-name suggestion, and the exact
`micromegas-grants` commands an admin would run — is the discoverability answer #1535 added and is
worth keeping. It moves to where the refusal now actually happens: the server's 403.

Its old lead line — "cannot mint audience 'X': it is not in this caller's mintable set" — is
dropped rather than re-sited verbatim: a 403 can now also arrive from `max_keys_per_caller`,
`max_claims_per_caller`, the reserved-name arm, or an over-long selector, none of which is "outside
the mintable set". The server's own message is already printed immediately above the hint (it's
the `HTTP 403: ...` text `run()` caught), so the hint appends only the mintable-audiences list, the
fresh-name suggestion, and the grant commands — the parts that stay useful no matter which 403
fired.

`run()` wraps the mint call:

```python
try:
    result = client.mint_ingestion_api_key(args.name, audience)
except RuntimeError as e:
    if str(e).startswith("HTTP 403"):
        raise RuntimeError(f"{e}\n{_mint_denied_hint(args.url, audience, my_audiences)}") from e
    raise
```

`_check_response` (`web_client.py:30-37`) raises `RuntimeError(f"HTTP {status}: {msg}")`, so the
status prefix is a reliable discriminator without adding a typed exception. `main()` already
prints `Error: {e}` and exits 1, so the enriched multi-line message lands unchanged.

The helper is rekeyed off the **resolved** audience rather than `args.audience`, since the denial
can now arrive from any of the three spellings:

```python
def _mint_denied_hint(url, audience, my_audiences): ...
```

and its fresh-name line becomes the new flag, with the same no-email branch as `--user-audience`'s
own error above:

- `mint_prefix` available → ``to use an audience of your own: --user-audience <name> (mints under `alice-<name>`)``
- `mint_prefix` is `None`, email present → `to use an audience of your own: --audience <new-name>`
- email is `None` → `this caller has no email, so it cannot claim a fresh audience of its own;
  ask an admin for a grant`

`_claim_suggestion` is renamed to `_fresh_audience_suggestion` and now takes `mint_prefix` and
`email`, since it chooses between those three lines.

### Zero-match error text (both flags omitted)

The admin `parser.error` at `:157-161` ("--audience is required for an admin caller") also offers
`--user-audience <name>` alongside `--audience <name>` — an admin resolves `mint_prefix` the same
as anyone else, so the same two flags apply. The two non-admin `parser.error`s at `:180-190` keep
their structure and swap their advice from `--claim <new-name>` to `--user-audience <name>`,
reporting the composed name when a `mint_prefix` is available.

## Implementation Steps

1. **`python/micromegas/micromegas/cli/setup_telemetry.py`**
   - Rewrite the module docstring (`:11-15`): two flags, one meaning each, both lazily creating.
   - Rename `_claim_suggestion` → `_fresh_audience_suggestion`; emit the `--user-audience` /
     `--audience <new-name>` / no-email trio described above.
   - Rewrite `_cannot_mint_hint` → `_mint_denied_hint(url, audience, my_audiences)`: drop the
     diagnostic lead line, keep the mintable-audiences/fresh-name/grant-command lines, keyed off
     the resolved audience, reading `audiences`/`mint_prefix`/`email` off the response dict instead
     of taking them as separate parameters.
   - Rewrite `resolve_audience` per the sketch above; delete the admin `--claim` rejection, the
     `email is None` claim precondition (the server owns it — a caller with no email hits the
     `Forbidden` arm at `ingestion_keys.rs:452-458`), and the `_cannot_mint_hint` pre-flight call.
     Update the admin and non-admin zero-match error texts per the Design section above.
   - `build_parser`: add `--user-audience NAME`; rewrite `--audience`'s help; change `--claim`'s
     help to `argparse.SUPPRESS`.
   - `run()`: wrap `mint_ingestion_api_key` in the 403 enrichment.
2. **`python/micromegas/tests/cli/test_setup_telemetry.py`** — see Testing Strategy.
3. **`python/micromegas/micromegas/web_client.py:196-199`** — `my_audiences`' docstring describes
   `mint_prefix` as backing a `--claim` suggestion; it now backs `--user-audience`'s composition,
   which *is* a name minted under. Reword.
4. **Docs** — see Documentation.
5. **`CHANGELOG.md`** — one `**Python:**` bullet under `## Unreleased` naming #1571, both flags,
   the `--claim` deprecation, and the fact that no server change was needed.

## Files to Modify

- `python/micromegas/micromegas/cli/setup_telemetry.py`
- `python/micromegas/tests/cli/test_setup_telemetry.py`
- `python/micromegas/micromegas/web_client.py`
- `mkdocs/docs/admin/api-keys.md`
- `mkdocs/docs/admin/authorization.md`
- `mkdocs/docs/query-guide/python-api.md`
- `mkdocs/docs/blog/posts/2026-09-03-record-your-ai-agent-share-on-your-terms.md`
- `CHANGELOG.md`
- `rust/analytics-web-srv/src/audience_grants.rs` (doc comment only)

No Rust *behavior* change, no `analytics-web-app` change.

## Trade-offs

**`--audience` for a fresh name now claims it instead of refusing.** This is the point of the
change, and it does give up one safety property: a non-admin typo like `--audience prdo` used to
be a hard client-side error and now claims `prdo` for that caller. Three things bound it — the
claim is visible on the Audience Access page and deletable by an admin, `max_claims_per_caller`
caps the damage, and `--user-audience` (the flag every doc now leads with) contains a typo inside
the caller's own namespace where it can neither collide with nor squat anything that matters.
Refusing instead would mean keeping the client-side guard, which is the wart being removed.

**Compose in the CLI rather than teaching the server a "prefixed" request field.** A new
`user_audience` field on `MintRequest` would move composition server-side, but it adds wire
surface, a second way for a request to name an audience, and a Rust change — for a concatenation
the response already carries the operand for, and that the web app already performs client-side.
Composing in the CLI keeps `mint_prefix` a plain suggestion value with no new wire surface.

## Decisions

- Flag name: `--user-audience`, per the issue — not `--my-audience` or `--audience-suffix`.
- The mint-policy admin short-circuit (`rust/auth/src/policy.rs:553`) and `mint_key`'s admin
  pre-check stay exactly as they are. This change touches only the interface asymmetry, not the
  privilege asymmetry; the issue's "Optional follow-up" (~80-120 Rust lines) is explicitly a
  separate change and is not a prerequisite.
- `--claim` ships deprecated-and-hidden for one release, then is removed. Recorded so the removal
  doesn't get re-argued: it is an exact alias, not a maintained second path.
- The `_cannot_mint_hint` text is kept, re-sited to the server's 403, rather than deleted with the
  guard that rendered it. The issue's scope line only asks to drop its `--claim` suggestion.
- The blog post's command block is updated in place. It is a live docs page, and leaving a
  soon-to-be-removed flag in the one command readers copy-paste is worse than editing a dated post.
- `--user-audience` composes `mint_prefix` for admins and non-admins alike, unlike
  `MintIngestionKeyDialog.tsx` (`:71`), which composes it only for non-admins. This divergence
  from the web dialog is intentional, not a follow-up to reconcile.

## Documentation

- **`mkdocs/docs/admin/api-keys.md:213-221`** — currently presents `--claim` as *the* self-service
  mint flow without noting it is unavailable to an admin caller. Rewrite around `--user-audience`,
  stating that the prefix is composed server-side from the caller's email and that the same
  command works for both roles. Drop the "the script suggests (but does not enforce) a namespace"
  sentence — with `--user-audience` the namespace is applied, not suggested.
- **`mkdocs/docs/admin/authorization.md:220-235`** — the three-command block: replace the
  `--claim "$USER-ci-runner"` example with `--user-audience ci-runner`, and drop the "namespacing
  is your convention" comment, which is no longer true for that flag.
- **`mkdocs/docs/admin/authorization.md:274`** — the route table describes `mint_prefix` as
  "a suggested namespace prefix for a fresh name (suggestion only; nothing mints under it)";
  reword to say a name is minted under it via `--user-audience`, matching the `web_client.py`
  docstring update.
- **`mkdocs/docs/query-guide/python-api.md:1042-1097`** — the CLI reference. Update the
  copy-paste example block's third command (`:1061-1065`) from `--claim "$USER-ci-runner"` to
  `--user-audience ci-runner`, dropping its "the namespacing convention lives in the name you
  pass" comment. Rewrite the flag list: `--user-audience` first (the recommended form, with the
  composition table), then `--audience` (verbatim, lazily creating, no longer a hard error for a
  name outside the mintable set), then "omitted entirely" — its auto-resolution logic is
  unchanged, but its closing advice (`:1095`, "plus a hint to `--claim` a fresh name or ask an
  admin") changes from `--claim` to `--user-audience`, and its final sentence ("An admin caller
  must always pass `--audience` explicitly") is reworded to offer `--user-audience` alongside
  `--audience`, matching the CLI's own admin zero-match error. Remove the `--claim` bullet.
- **`mkdocs/docs/query-guide/python-api.md:717-725`** — the `my_audiences()` `WebClient` bullet
  says `mint_prefix` is used only to *suggest* a fresh audience name and that nothing is minted
  under it; reword to match the `web_client.py` docstring update, since `--user-audience` now
  mints under it.
- **`mkdocs/docs/blog/posts/2026-09-03-record-your-ai-agent-share-on-your-terms.md:38,94`** —
  `--claim "$USER-claude"` → `--user-audience claude` at `:38`. The surrounding narrative ("mints
  you a personal ingestion key and claims an audience nobody else can read") stays true, and for a
  typical `alice@example.com` / `$USER=alice` the resolved name is unchanged (`alice-claude`). At
  `:94`, "The first two rows were written by the `--claim` above" becomes "...written by the
  `--user-audience` mint above".
- **`CHANGELOG.md`** — an `## Unreleased` bullet. No **Minor breaking change** clause is needed
  for the flag surface (`--claim` still works this release), but the deprecation is stated so the
  next release's removal has a reference.
- **`rust/analytics-web-srv/src/audience_grants.rs:764-766`** — reword `mint_prefix_for`'s doc
  comment to describe backing `--user-audience`'s composition instead of only `--claim`'s
  error-message suggestion.

## Testing Strategy

Every behavior here is reachable by calling `resolve_audience`/`run` with constructed inputs, so
it is all unit-tested in `python/micromegas/tests/cli/test_setup_telemetry.py` against the
existing `FakeClient`/`FakeParser`. No live-DB or live-service test is added — nothing here is a
bug witnessed in the wild, and the server paths this leans on
(`try_claim_and_mint`, the reserved-name and quota rejections, `CLAIM_CONTENDED`) already carry
their own Rust coverage in `rust/analytics-web-srv/tests/ingestion_keys_tests.rs`.

First, fix a fixture bug the new tests would otherwise inherit: every admin fixture in the file
sets `"mint_prefix": None` alongside `"email": "admin@example.com"`, which the real server never
returns — `mint_prefix_for` is called unconditionally at `audience_grants.rs:871` and would yield
`"admin-"`. Correct those fixtures, and add `"user_audience"` / keep `"claim"` in `make_args`'
defaults.

New/changed cases:

- `--user-audience laptop` with `mint_prefix="alice-"` → `alice-laptop`.
- **The same assertion with `is_admin=True`** and `mint_prefix="admin-"` → `admin-laptop`. This is
  the test that pins the issue's central claim: the flag is role-independent.
- `--user-audience` with `mint_prefix=None` → `SystemExit`, message names `--audience`.
- `--user-audience ""` → `SystemExit`.
- `--user-audience` never normalizes: `--user-audience Ci_Runner` → `alice-Ci_Runner` verbatim
  (guards against a future `.lower()`/`.strip()` creeping back in).
- `--audience prod` for a non-admin holding no grant on `prod` → returned verbatim, **no**
  `SystemExit`. This replaces `test_audience_outside_mintable_set_is_an_error_naming_grant_commands`,
  whose premise is inverted by this change.
- `--audience` + `--user-audience` together → `SystemExit`.
- `--claim laptop` → `"laptop"`, plus a deprecation warning on stderr (`capsys`).
- `--claim` + `--audience` together → `SystemExit`.
- `run()` end-to-end: a `FakeClient` whose `mint_ingestion_api_key` raises
  `RuntimeError("HTTP 403: audience 'prod' already exists ...")` → the raised message contains the
  original 403 text, the caller's mintable audiences, the `--user-audience` suggestion, and both
  concrete `micromegas-grants ... create prod mint 'user:alice@example.com'` and
  `... mint '*'` commands.
- `run()` with a non-403 `RuntimeError` (e.g. `"HTTP 500: ..."`) → propagates unchanged, no hint
  appended.
- `run()` with `--user-audience claude` → asserts the client saw `("mint", "laptop", "alice-claude")`,
  i.e. the *composed* name goes over the wire and the flag choice does not.
- `build_parser` defaults gain `args.user_audience is None`.

Existing tests for the both-omitted `held_pairs` resolution keep their assertions; only the two
that assert on `--claim <new-name>` substrings update to the new advice string.

Delete `test_claim_as_admin_is_an_error` and `test_claim_with_no_email_is_an_error` — both assert
`SystemExit` for a rejection this change removes (the admin `--claim` refusal and the
`email is None` precondition), and after the change both inputs resolve to `"ci"`/`"laptop"`
respectively instead of raising.

Delete `test_audience_outside_mintable_set_hint_uses_the_mint_prefix` and
`test_audience_outside_mintable_set_hint_degrades_with_no_mint_prefix` — both drive `--audience
prod` for a non-admin with no grant and assert `SystemExit`, which is exactly the client-side
refusal this change removes. Fold their two cases (`mint_prefix` present vs. absent) into the new
`run()` 403-hint test above, asserting the same `--user-audience`-present-vs.-absent hint text off
the 403 path instead of a `parser.error`.

In `test_claim_is_used_verbatim`, drop the `assert capsys.readouterr().err == ""` line — `--claim`
now prints a deprecation warning on stderr, which that assertion would fail.

Reword `test_run_non_admin_claim_does_not_call_create_audience_grant`'s docstring: it currently
says "`--audience laptop` … is now a hard error", which is no longer true once `--audience` lazily
creates; state instead that `--claim` is used here only to exercise the deprecated-alias path.

## Manual Verification

One end-to-end run, because nothing below the CLI is mocked in it — the real OIDC login, the real
`my_audiences` response's `mint_prefix`, and the real lazy claim inside the mint transaction.
Failure here would be immediately obvious to the next person who runs the command, which is why it
is a manual step rather than a live-service test.

1. Export an OIDC config first (e.g. `. ~/set_human_auth.sh` or set `MICROMEGAS_OIDC_CONFIG`
   directly) — `start_services.py` only starts with `--disable-ingestion-auth` (real
   mint/grant routes) when `MICROMEGAS_OIDC_CONFIG`/`MICROMEGAS_ANALYTICS_OIDC_CONFIG` is
   already set; otherwise it falls back to `--disable-auth`, under which the mint and
   audience-grant routes don't exist at all. Also `export
   MICROMEGAS_TELEMETRY_URL=http://127.0.0.1:9000` in the same shell you'll run
   `micromegas-setup-telemetry` from — `start_services.py` only sets it for the child
   processes it spawns, not for your shell, and `micromegas-setup-telemetry` errors out
   without it (or an explicit `--otlp-endpoint`). Then run
   `python3 local_test_env/ai_scripts/start_services.py --monolith`, with
   `MICROMEGAS_SELF_SERVICE_MINT=1` set for the monolith.
2. Take over the `admins` group, following the documented add-then-remove order
   (`mkdocs/docs/admin/groups.md:195-196`) so an admin caller remains after the wildcard is gone:
   while the wildcard still applies, seed a separate **operator** identity —
   `micromegas-groups --url http://127.0.0.1:3000 add admins user:<operator account>` — then
   `micromegas-groups --url http://127.0.0.1:3000 remove admins '*'`. A fresh DB's v10 migration
   seeds `admins` with a wildcard member, so until it's removed every authenticated caller is an
   admin and step 3 can't exercise a non-admin caller.
3. As a **non-admin** caller (an OIDC account that is neither the operator nor otherwise in
   `admins`): `micromegas-setup-telemetry --url http://127.0.0.1:3000 --name laptop --user-audience
   claude` → stderr reports `audience=<yourprefix>-claude` and `claimed audience
   <yourprefix>-claude`; stdout carries the three `OTEL_EXPORTER_OTLP_*` exports.
4. Re-run the identical command → mints a second key under the same audience, and this time prints
   **no** `claimed audience` line (the caller now holds the grant).
5. As the **operator** account, `micromegas-groups --url http://127.0.0.1:3000 add admins user:<the
   step-3 account>`, then run the byte-identical command from step 3 as that now-admin caller →
   same shape, resolved under the same prefix. This is the "one command, both roles" claim.

The 403-plus-hint path (a second caller denied on an already-claimed audience) is not manually
verified here: it is covered by the planned `run()` unit test above and is pinned server-side by
the existing live test
`live_mint_claims_a_fresh_audience_then_denies_a_second_caller`
(`rust/analytics-web-srv/tests/ingestion_keys_tests.rs:1036`).

## Open Questions

None.
