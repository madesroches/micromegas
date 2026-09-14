# micromegas-screens: Profile-Aware Auth Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1572

## Overview

`micromegas-screens` is the only CLI in the Python package that never reads
`~/.micromegas/config.json`. It resolves OIDC from two env vars, silently falls back to an
unauthenticated client when they're unset, and caches its tokens in the plain
`~/.micromegas/tokens.json` even on a machine where every other CLI has moved to a per-profile
`tokens-<profile>.json`. This plan routes it through `cli/config.py::resolve_connection` behind a
new shared `cli/web_auth.py` helper, adds `--profile` and an explicit `--no-auth` opt-out, and
makes "no auth mechanism resolved" a named error instead of a confusing downstream HTTP failure.

The same helper replaces the three near-identical `build_auth_provider` copies in `grants.py`,
`groups.py`, and `import_keys.py` (the latter also serving `setup_telemetry.py`), so a profile's
`api_key_file` starts working on those tools too — today it resolves to nothing and they connect
unauthenticated, a gap called out in the docs at `mkdocs/docs/query-guide/python-api.md:843-846`.

The analytics web app's HTTP URL stays where it is today (`micromegas-screens.json`'s `"server"`
key); only auth comes from the profile. See Trade-offs.

## Current State

### `screens.py::make_client` (`python/micromegas/micromegas/cli/screens.py:184-207`)

```python
def make_client(config):
    auth_provider = None
    issuer = os.environ.get("MICROMEGAS_OIDC_ISSUER")
    client_id = os.environ.get("MICROMEGAS_OIDC_CLIENT_ID")
    if issuer and client_id:
        client_secret = os.environ.get("MICROMEGAS_OIDC_CLIENT_SECRET")
        if client_secret:
            auth_provider = OidcClientCredentialsProvider.from_env()
        else:
            auth_provider = load_or_login(issuer=issuer, client_id=client_id)
    return WebClient(config["server"], auth_provider=auth_provider)
```

No `config.json`, no `--profile`, no `token_file` (so `load_or_login` defaults to
`~/.micromegas/tokens.json`, `oidc_connection.py:70-71`), no `audience`, no `scope`, and no error
when nothing resolves. Every client-building subcommand calls it as `make_client(config)`:
`cmd_import` (`screens.py:261`), `cmd_pull` (`:300`), `cmd_plan` (`:492`), `cmd_apply` (`:505`),
`cmd_list` (`:594`). `cmd_init` builds no client.

`main()` (`screens.py:645-713`) defines six subparsers and catches only `RuntimeError`.

### The three copies this duplicates

`grants.py:28-58`, `groups.py:26-56`, and `import_keys.py:142-174` all carry the same
`build_auth_provider` body, modulo `import_keys`' extra `parser` argument for `parser.error()`:

1. If `MICROMEGAS_OIDC_ISSUER` + `_CLIENT_ID` + `_CLIENT_SECRET` are all in the environment,
   `OidcClientCredentialsProvider.from_env()`.
2. Else `config.resolve_connection(profile=args.profile)`; if `oidc_issuer` and `oidc_client_id`
   both resolved, `oidc_connection.load_or_login(...)` with the profile's `token_file`, `audience`,
   and `scope`.
3. Else `None` — an unauthenticated `WebClient`, deliberately, for `--disable-auth` targets.

None of the three consults `conn.api_key_file`, so a profile whose only auth is a static key
yields `None`. `setup_telemetry.py` imports `import_keys.make_client` wholesale
(`setup_telemetry.py:32`).

### Resolution machinery already in place

`cli/config.py` has everything needed: `resolve_active_profile` (precedence `--profile` >
`MICROMEGAS_PROFILE` > `default_profile`), `default_token_file(name)` →
`~/.micromegas/tokens-<name>.json`, the `ProfileError` hierarchy, per-field env-var override via
`_pick`, and the "exactly one auth mechanism" check that raises when a profile resolves both
`api_key_file` and a complete OIDC pair (`config.py:186-187`, `_two_mechanism_message`). `ConnectionConfig` (`config.py:52-61`)
carries the resolved fields but not the resolved profile *name*.

`connection.py::connect_with_profile` (`python/micromegas/micromegas/connection.py:14-115`) already
implements the three-mechanism branch for FlightSQL: `api_key_file` → OIDC → no auth. The web CLIs
need the same shape plus the client-credentials branch, which FlightSQL's path doesn't have.

`StaticTokenAuthProvider` is already documented as usable with `WebClient`
(`auth/static_token.py:19-22`), and `WebClient._headers` (`web_client.py:22-27`) only ever calls
`auth_provider.get_token()`, so all three provider types drop in unchanged.

### Tests and docs

- `tests/cli/` holds `test_config.py`, `test_grants.py`, `test_groups.py`, `test_import_keys.py`,
  and an autouse `conftest.py` fixture that scrubs every `MICROMEGAS_*` var before each test.
  `test_config.py` passes `config_path=tmp_path/...` rather than patching `CONFIG_PATH`.
- `tests/test_screen_files.py` monkeypatches `screens_module.make_client` with `lambda config: ...`
  in three places (`:669`, `:698`, `:737`).
- `mkdocs/docs/web-app/notebooks/screens-as-code.md` documents the env-var-only auth model and a CI
  example built on the three `MICROMEGAS_OIDC_*` secrets.
- `mkdocs/docs/query-guide/python-api.md:843-846` records that only `micromegas-query` and
  `connect_with_profile()` honor `api_key_file`; `:914-918` records that `micromegas-screens` is
  not profile-aware.

## Design

### 1. `ConnectionConfig` learns its profile name

Add a trailing field to `config.py:52-61`:

```python
profile: Optional[str] = None
```

populated from the name `resolve_connection` already computes (`config.py:171`, `resolve_active_profile`'s first return value). Appended last, so
nothing positional breaks. It exists so an error message can name the profile that failed to
resolve auth, without a second `load_config`/`resolve_active_profile` round-trip in the caller.

### 2. New `python/micromegas/micromegas/cli/web_auth.py`

One function, the single place any `WebClient`-based CLI resolves auth:

```python
def resolve_web_auth(profile=None, config_path=None):
    """Return `(auth_provider, diagnostic)` for a WebClient.

    `auth_provider` is None exactly when no mechanism resolved, and
    `diagnostic` is then a sentence naming what was missing; when a provider
    resolves, `diagnostic` is None. Deciding whether an unauthenticated
    client is acceptable is the caller's policy, not this function's.
    """
```

Body, in order:

1. `conn = config.resolve_connection(config_path=config_path, profile=profile)` — raises
   `ProfileError` for an unknown/unselected profile, a malformed entry, or a config naming two auth
   mechanisms. Callers surface it; this function never swallows it.
2. `conn.api_key_file` → `StaticTokenAuthProvider.from_file(...)`, wrapping `OSError`/`ValueError`
   in `ProfileError` with `connection.py:78-81`'s exact wording.
3. All three of `MICROMEGAS_OIDC_ISSUER`/`_CLIENT_ID`/`_CLIENT_SECRET` set in the environment →
   `OidcClientCredentialsProvider.from_env()`, byte-for-byte today's non-interactive CI branch.
4. `conn.oidc_issuer and conn.oidc_client_id` → `load_or_login(issuer, client_id, client_secret,
   token_file=conn.token_file, audience=conn.oidc_audience, scope=conn.oidc_scope)`.
5. Otherwise `(None, diagnostic)`.

Step 3 stays keyed on the **env triple** rather than on `conn.oidc_client_secret`, even though
those are the same value today (`ConnectionConfig.oidc_client_secret` is `_pick(
"MICROMEGAS_OIDC_CLIENT_SECRET")` with no profile fallback, `config.py:184`). Keying on the env
triple keeps the trigger literally unchanged: a profile-configured public client whose IdP requires
a secret (Google, per `mkdocs/docs/admin/authentication.md:255`) keeps its interactive browser
flow instead of silently switching to a client-credentials grant the moment someone exports a
secret. Step 3 runs after step 2 only because step 1 has already rejected any config that resolves
both an API key and a complete OIDC pair, so at most one of steps 2-4 can match.

`diagnostic` shape, built from `conn.profile`:

```
profile 'prod' resolves no auth mechanism: no 'api_key_file', and no OIDC issuer/client_id
(set 'client_id' and 'issuers[0].issuer' in the profile, or MICROMEGAS_OIDC_ISSUER and
MICROMEGAS_OIDC_CLIENT_ID)
```

and, when `conn.profile is None` (flat config or no config file), the same sentence with
"`~/.micromegas/config.json`" in place of `profile 'prod'` and a trailing "pass --profile to select
a named profile".

### 3. `screens.py`

- `make_client(config, args)`:

```python
auth_provider, diagnostic = web_auth.resolve_web_auth(profile=args.profile)
if auth_provider is None and not args.no_auth:
    raise ProfileError(f"{diagnostic}; pass --no-auth to target a server started with --disable-auth")
return WebClient(config["server"], auth_provider=auth_provider)
```

  The five call sites become `make_client(config, args)`.
- Shared flags on a parent parser, applied only to the subcommands that build a client:

```python
client_args = argparse.ArgumentParser(add_help=False)
client_args.add_argument("--profile", help="Named connection profile from ~/.micromegas/config.json")
client_args.add_argument("--no-auth", action="store_true",
                         help="Target a server started with --disable-auth")
...
p_import = subparsers.add_parser("import", parents=[client_args], ...)
```

  `parents=[client_args]` on `import`/`pull`/`plan`/`apply`/`list`, not on `init` (no client, no
  server contact) and not on the top-level parser. Defining the same flag on both the main parser
  and a subparser is an argparse trap — the subparser's `None` default overwrites the main parser's
  parsed value in the shared `Namespace` — so the flags live in exactly one place, and are typed
  after the subcommand (`micromegas-screens apply --profile prod`), alongside `--auto-approve` and
  `--color`.
- `main()`'s handler catches `config.ProfileError` alongside `RuntimeError` (`ProfileError`
  subclasses `ValueError`, so today's handler misses it).

### 4. `grants.py` / `groups.py` / `import_keys.py`

Each keeps its module-level `build_auth_provider` name and signature — they're the seam the
existing tests monkeypatch — and the body collapses to a delegation that discards the diagnostic,
preserving today's permissive "`None` means unauthenticated" behavior on these three:

```python
def build_auth_provider(args):
    provider, _diagnostic = resolve_web_auth(profile=args.profile)
    return provider
```

`import_keys.build_auth_provider(args, parser)` keeps its `parser.error()` translation of
`ProfileError`. These three gain `api_key_file` support as a side effect of sharing the helper;
they do **not** gain `--no-auth` or the strict error in this change (see Trade-offs).

```
screens.py  grants.py  groups.py  import_keys.py ──┐
                                                   ├─→ web_auth.resolve_web_auth()
                          setup_telemetry.py ──────┘            │
                                                                ▼
                                            config.resolve_connection(profile=...)
                                                    │
             ┌──────────────────────┬───────────────┴────────────┬───────────────┐
             ▼                      ▼                            ▼               ▼
      api_key_file →        env OIDC triple →           issuer+client_id →     (None,
   StaticTokenAuthProvider  OidcClientCredentials       load_or_login with   diagnostic)
                            Provider.from_env()        per-profile token_file
```

## Implementation Steps

1. **`cli/config.py`**: add the trailing `profile: Optional[str] = None` field to
   `ConnectionConfig` and populate it from the resolved name in `resolve_connection`.
2. **`cli/web_auth.py`** (new): implement `resolve_web_auth` per Design §2, including the
   diagnostic builder.
3. **`cli/screens.py`**: rewrite `make_client(config, args)`, update the five call sites, add the
   `client_args` parent parser to the five client subcommands, and catch `ProfileError` in
   `main()`. Drop the now-unused `os` import if nothing else in the module uses it.
4. **`cli/grants.py`, `cli/groups.py`, `cli/import_keys.py`**: replace the three
   `build_auth_provider` bodies with the delegation; delete their now-dead `os`/OIDC imports.
5. **Tests**: new `tests/cli/test_web_auth.py`; new `tests/cli/test_screens_auth.py`; fix the three
   `lambda config:` monkeypatches in `tests/test_screen_files.py` to `lambda config, args:`.
6. **Docs**: `screens-as-code.md` auth section, the two `python-api.md` passages, and a
   `CHANGELOG.md` **Unreleased** entry.

## Files to Modify

- `python/micromegas/micromegas/cli/config.py`
- `python/micromegas/micromegas/cli/web_auth.py` *(new)*
- `python/micromegas/micromegas/cli/screens.py`
- `python/micromegas/micromegas/cli/grants.py`
- `python/micromegas/micromegas/cli/groups.py`
- `python/micromegas/micromegas/cli/import_keys.py`
- `python/micromegas/tests/cli/test_web_auth.py` *(new)*
- `python/micromegas/tests/cli/test_screens_auth.py` *(new)*
- `python/micromegas/tests/test_screen_files.py`
- `mkdocs/docs/web-app/notebooks/screens-as-code.md`
- `mkdocs/docs/query-guide/python-api.md`
- `CHANGELOG.md`

## Trade-offs

- **Web-app URL: keep `"server"` in `micromegas-screens.json`, don't add a profile key.** The
  issue leaves this open. A profile's `uri` is a gRPC FlightSQL endpoint and can't double as the
  web app's HTTP URL, so a single `--profile` selecting both would require a new optional profile
  key. Against it: `micromegas-screens.json` is a committed, repo-wide file whose `server` belongs
  next to `managed_by` as a property of the screens repo, not of the user running it; and every
  other `WebClient` CLI already takes its URL explicitly (`--url`, required) with `--profile`
  meaning auth only. Adding a profile-level web URL would be a second way to say the same thing
  with no rule for which wins. Auth-only is also the smaller change and doesn't foreclose the
  other: a `web_url` profile key could later become the default for `--url` on
  `grants`/`groups`/`import-keys`, where it would actually remove a required flag.
- **Strict "no auth resolved" error on `screens` only.** `grants`/`groups`/`import-keys` keep their
  silent `None`. Flipping them too would be the consistent thing, but it breaks every existing
  local-dev invocation against a `--disable-auth` monolith until the user adds `--no-auth`, and
  those three are out of this issue's scope. The shared helper already returns the diagnostic they'd
  need, so adopting it there later is a one-line change per CLI plus a flag.
- **Shared helper returns `(provider, diagnostic)` rather than raising on "nothing resolved".**
  Resolution is mechanism, "is unauthenticated acceptable here?" is policy, and the two callers
  disagree about the policy today. Returning the diagnostic instead of an
  `allow_unauthenticated=True/False` parameter keeps the decision — and the wording of the final
  error, which differs per CLI — at the call site.
- **Keep `build_auth_provider` as a thin wrapper in the three CLIs** instead of calling
  `resolve_web_auth` directly from their `make_client`. It's one extra line each, and it preserves
  the monkeypatch seam their existing tests use.

## Decisions

- The client-credentials branch stays keyed on all three `MICROMEGAS_OIDC_*` env vars, not on the
  resolved `client_secret`, so no existing browser-login setup silently changes grant type.
- Accepted regression: on a machine that has a `profiles` map, an env-var-only `micromegas-screens`
  invocation (no `--profile`, no `MICROMEGAS_PROFILE`, no `default_profile`) now fails with the
  "no profile selected" `ProfileError` from `resolve_connection`, where it previously worked off
  the env vars alone. This is the same rule `micromegas-query` already enforces and documents
  (`python-api.md:797`); the fix is to select a profile or set `default_profile`. CI that only ever
  exports the OIDC env vars on a box with no `~/.micromegas/config.json` is unaffected.
- `api_key_file` support reaching `grants`/`groups`/`import-keys`/`setup-telemetry` is a deliberate
  part of this change, not incidental scope — without it, the new strict error on `screens` would
  reject a static-key profile that the tool could in fact authenticate with.
- `--no-auth` is spelled to echo the server's `--disable-auth` flag in its help text; no env-var
  equivalent is added.

## Documentation

- `mkdocs/docs/web-app/notebooks/screens-as-code.md` — rewrite **Authentication**: profile
  resolution and `--profile`, the per-profile token cache, `api_key_file` as a CI alternative, the
  explicit failure when nothing resolves, and `--no-auth` for a `--disable-auth` server. Keep the
  existing CI example (the env triple still works) and note it needs no profile.
- `mkdocs/docs/query-guide/python-api.md` — rewrite `:843-846` (all `WebClient` CLIs now honor
  `api_key_file`; the remaining caveat is that `--no-auth` exists only on `micromegas-screens`),
  and delete the "not profile-aware" paragraph at `:914-918`, replacing it with the token-cache
  sentence that now applies.
- `CHANGELOG.md` — one **Unreleased** entry under a `**Python CLI:**` heading covering the new
  `--profile`/`--no-auth` flags, the per-profile token cache, the strict error, `api_key_file`
  reaching the other three CLIs, and the accepted regression above.

## Testing Strategy

All unit tests, no live service — `resolve_web_auth` is pure resolution over a `config_path` the
test writes into `tmp_path`, and the three provider constructors are the seams to patch. The
`tests/cli/conftest.py` autouse fixture already scrubs `MICROMEGAS_*`, so env-var cases are set
explicitly with `monkeypatch.setenv`.

`tests/cli/test_web_auth.py`:

- Profile with `api_key_file` → a `StaticTokenAuthProvider` whose `get_token()` is the file's
  stripped contents; `diagnostic is None`.
- `api_key_file` pointing at a missing/empty file → `ProfileError` naming `api_key_file`.
- Profile with `issuers`/`client_id`, no env secret → `load_or_login` (monkeypatched) receives
  `token_file=~/.micromegas/tokens-<profile>.json`, plus the profile's issuer, client id, and
  audience.
- All three `MICROMEGAS_OIDC_*` set → `OidcClientCredentialsProvider.from_env` (monkeypatched as a
  classmethod, so no OIDC discovery request is made) is the branch taken, and `load_or_login` is
  not called.
- Env secret set but issuer/client_id coming from the profile → the `load_or_login` branch, *not*
  client-credentials (pins the Decisions entry above).
- `MICROMEGAS_OIDC_ISSUER` overrides the profile's issuer in the `load_or_login` call.
- Profile with both `api_key_file` and a complete OIDC pair → `ProfileError` propagates.
- Nothing configured (missing config file) → `(None, diagnostic)`, diagnostic naming both the
  profile keys and the env vars; with a named profile, the diagnostic contains the profile name.

`tests/cli/test_screens_auth.py`:

- `make_client` with nothing resolvable and `no_auth=False` → `ProfileError` whose message contains
  the diagnostic and `--no-auth`.
- Same with `no_auth=True` → a `WebClient` with `auth_provider is None` and
  `base_url == config["server"]`.
- A resolved provider is passed through to the `WebClient` and the `server` URL is honored.
- Parser wiring, parametrized over `import`/`pull`/`plan`/`apply`/`list`: parsing
  `[cmd, "--profile", "prod"]` yields `args.profile == "prod"` and `args.no_auth is False`, and
  `--no-auth` sets it True — this is what catches a subcommand accidentally left off
  `parents=[client_args]`, which would otherwise only fail at runtime with an `AttributeError`.
- `init` rejects `--profile` (it contacts no server).

`tests/cli/test_grants.py` / `test_groups.py` / `test_import_keys.py` keep passing unchanged (the
`build_auth_provider` seam is preserved); add one case to `test_grants.py` asserting a profile with
only `api_key_file` now yields a `StaticTokenAuthProvider` rather than `None`.

## Manual Verification

One check, for the piece no unit test covers: that a real browser login writes and then reuses the
per-profile token cache across two different CLIs.

1. With a `~/.micromegas/config.json` profile `local` carrying a real `client_id`/`issuers`, run
   `micromegas-query --profile local --all "SELECT 1"` and complete the browser login. Expect
   `~/.micromegas/tokens-local.json` to exist.
2. In a screens directory, run `micromegas-screens --profile local list`. Expect the inventory to
   print with **no** browser window — the issue's symptom 3, which no unit test can demonstrate
   because it depends on the real OIDC round-trip and the real IdP's refresh token.
3. Against a monolith started with `--disable-auth` and no OIDC config anywhere, run
   `micromegas-screens list`. Expect a one-line error naming the missing settings and `--no-auth`,
   and `micromegas-screens --no-auth list` to succeed.

## Open Questions

- Should `micromegas-screens init` record a suggested profile name in `micromegas-screens.json`
  (e.g. `"profile": "prod"`) so a repo can declare which profile matches its `server`? It's
  convenient, but it inverts the documented selection precedence (a file-level default that loses
  to `MICROMEGAS_PROFILE` would need its own rule, since `resolve_active_profile`'s `profile`
  argument outranks the env var). Left out; easy to add later without breaking anything.
