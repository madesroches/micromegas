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
`groups.py`, and `import_keys.py` (the latter also serving `setup_telemetry.py`), so all four web
CLIs resolve auth through one code path. It deliberately does **not** add `api_key_file` support:
the analytics web API validates OIDC tokens only, so a static analytics API key is not a credential
these tools can present (see Current State § "The analytics web API is OIDC-only"). What the helper
adds on that front is a diagnostic that says so; only `micromegas-screens` turns that diagnostic
into an error, while `grants`/`groups`/`import-keys` still discard it and build an unauthenticated
client, as they do today.

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
yields `None` — the right *outcome* given the server (next section), but indistinguishable to the
user from "no auth configured at all". `setup_telemetry.py` imports `import_keys.make_client`
wholesale (`setup_telemetry.py:32`).

### Resolution machinery already in place

`cli/config.py` has everything needed: `resolve_active_profile` (precedence `--profile` >
`MICROMEGAS_PROFILE` > `default_profile`), `default_token_file(name)` →
`~/.micromegas/tokens-<name>.json`, the `ProfileError` hierarchy, per-field env-var override via
`_pick`, and the "exactly one auth mechanism" check that raises when a profile resolves both
`api_key_file` and a complete OIDC pair (`config.py:186-187`, `_two_mechanism_message`). `ConnectionConfig` (`config.py:52-61`)
carries the resolved fields but not the resolved profile *name*.

`connection.py::connect_with_profile` (`python/micromegas/micromegas/connection.py:14-115`) already
implements the three-mechanism branch for FlightSQL: `api_key_file` → OIDC → no auth. The web CLIs
need a *different* ladder, not a shared one: client credentials (which FlightSQL's path doesn't
have) and no static-key branch (which the web server can't validate). The two stay separate on
purpose.

`WebClient._headers` (`web_client.py:22-27`) only ever calls `auth_provider.get_token()` and sends
the result as a bearer token, so both OIDC provider types drop in unchanged.

### The analytics web API is OIDC-only

This is the constraint that keeps `api_key_file` out of this change. `analytics-web-srv` builds its
auth from `OidcAuthProvider`/`OidcConfig::from_env()` and nothing else (`auth/state.rs:7-16`,
`auth/config.rs:40`): the crate contains no `ProviderBuilder`, no `with_db_key_store`, and no
`ApiKeyTable`. Its own source records this — "nothing in `analytics-web-srv` runs a
`DbApiKeyAuthProvider`" (`analytics_keys.rs:290`). The web app *mints* analytics API keys; it never
*validates* them. In the monolith, the provider carrying `ApiKeyTable::Analytics` is built under
`roles.flightsql` and handed to the FlightSQL server (`monolith/src/main.rs:227-234`, `:321`); the
web role at `:360-378` gets its own OIDC-only config.

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

1. All three of `MICROMEGAS_OIDC_ISSUER`/`_CLIENT_ID`/`_CLIENT_SECRET` set in the environment →
   `OidcClientCredentialsProvider.from_env()`, byte-for-byte today's non-interactive CI branch.
2. `conn = config.resolve_connection(config_path=config_path, profile=profile)` — raises
   `ProfileError` for an unknown/unselected profile, a malformed entry, or a config naming two auth
   mechanisms. Callers surface it; this function never swallows it.
3. `conn.oidc_issuer and conn.oidc_client_id` → `load_or_login(issuer, client_id, client_secret,
   token_file=conn.token_file, audience=conn.oidc_audience, scope=conn.oidc_scope)`.
4. Otherwise `(None, diagnostic)`.

`diagnostic` shape, built from `conn.profile`:

```
profile 'prod' resolves no auth mechanism: no OIDC issuer/client_id (set 'client_id' and
'issuers[0].issuer' in the profile, or MICROMEGAS_OIDC_ISSUER and MICROMEGAS_OIDC_CLIENT_ID)
```

and, when `conn.profile is None` (flat config or no config file), the same sentence with
"`~/.micromegas/config.json`" in place of `profile 'prod'` and a trailing "set 'client_id' and
'issuers[0].issuer', or add a 'profiles' map and pass --profile to select a named profile".

When `conn.api_key_file` resolved but the OIDC pair did not, the diagnostic names the actual
problem rather than claiming nothing was configured — this is the case a user is most likely to hit
after configuring a working `micromegas-query` profile:

```
profile 'prod' configures 'api_key_file', but the analytics web API validates OIDC tokens only --
a static analytics API key works with micromegas-query (FlightSQL), not with this tool. Set
'client_id' and 'issuers[0].issuer' on a profile for this server.
```

No provider is constructed on that path and the key file is never read, so a profile pointing at an
unreadable key file still produces this message rather than an `OSError`.

`load_or_login` and `OidcClientCredentialsProvider` are imported function-local, not at module
scope — matching all three bodies being replaced and `connection.py`. `oidc_connection` is imported
as a module (`import micromegas.oidc_connection as oidc_connection`, per `connection.py:93`), not
via `from`, so a test can monkeypatch the `oidc_connection.load_or_login` attribute. A module-level
`from`-import would (a) pull `flightsql/client.py`'s module-scope `pyarrow`/`pyarrow.flight` imports
into every web CLI's startup path via `oidc_connection.py:10`'s own module-level FlightSQL import,
and (b) bind a local name that a module-attribute monkeypatch can't intercept, silently letting a
real browser login fire in tests.

### 3. `screens.py`

- `make_client(config, args)`:

```python
def make_client(config, args):
    if args.no_auth:
        return WebClient(config["server"], auth_provider=None)
    try:
        auth_provider, diagnostic = web_auth.resolve_web_auth(profile=args.profile)
    except ProfileError as e:
        raise ProfileError(f"{e}; pass --no-auth to target a server started with --disable-auth")
    if auth_provider is None:
        raise ProfileError(f"{diagnostic}; pass --no-auth to target a server started with --disable-auth")
    return WebClient(config["server"], auth_provider=auth_provider)
```

  A `ProfileError` raised inside `resolve_web_auth` (from `resolve_connection`, e.g. "no profile
  selected") gets the same `--no-auth` hint appended as the function's own diagnostic, so the hint
  is visible on every path that ends in `ProfileError`, not just the `(None, diagnostic)` one.

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
- `main()`'s handler catches `ValueError` alongside `RuntimeError` (`ProfileError` subclasses
  `ValueError` (`config.py:14`), so this covers it; it also covers `config.load_config` raising a
  plain `ValueError` on a malformed `~/.micromegas/config.json`, a path screens can now reach for
  the first time via `resolve_web_auth`).

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
`ProfileError`. Their observable behavior is unchanged by this refactor — same branches, same
order, same `None` for a `--disable-auth` target; they do **not** gain `--no-auth` or the strict
error in this change (see Trade-offs).

Their docstrings (and `setup_telemetry.py`'s module docstring, which repeats the same ladder for
its imported `import_keys.make_client`) shrink from spelling out the branch ladder to a pointer at
`web_auth.resolve_web_auth`, now the one place that ladder is described.

## Implementation Steps

1. **`cli/config.py`**: add the trailing `profile: Optional[str] = None` field to
   `ConnectionConfig` and populate it from the resolved name in `resolve_connection`. Update
   `ProfileError`'s docstring to also name `screens.py::make_client` as a raiser, for "no auth
   mechanism resolved".
2. **`cli/web_auth.py`** (new): implement `resolve_web_auth` per Design §2, including the
   diagnostic builder, keeping the `load_or_login`/`OidcClientCredentialsProvider` imports
   function-local and importing `oidc_connection` as a module.
3. **`cli/screens.py`**: import `web_auth`, and import `ProfileError` by name (unqualified, since
   `make_client`'s `config` parameter shadows the `config` module inside that function). Rewrite
   `make_client(config, args)`, update the five call sites, add the `client_args` parent parser to
   the five client subcommands, and catch `ValueError` (which subsumes `ProfileError`) alongside
   `RuntimeError` in `main()`. Drop the now-unused `os` import if nothing else in the module uses it.
4. **`cli/grants.py`, `cli/groups.py`, `cli/import_keys.py`, `cli/setup_telemetry.py`**: replace the
   three `build_auth_provider` bodies with the delegation; drop the now-unused `import os` from
   `grants.py` and `groups.py` only (the OIDC imports are function-local and vanish with the body;
   `import_keys.py` still needs `os` for `read_keyring`). Shrink the `build_auth_provider`
   docstrings in `grants.py`, `groups.py`, and `import_keys.py` (`import_keys.py` keeps its note
   about translating `ProfileError` via `parser.error()`), and `setup_telemetry.py`'s module
   docstring, from the full branch ladder to a pointer at `web_auth.resolve_web_auth`.
5. **Tests**: new `tests/cli/test_web_auth.py`; new `tests/cli/test_screens_auth.py`; fix the three
   `lambda config:` monkeypatches in `tests/test_screen_files.py` to `lambda config, args:`.
6. **Docs**: `screens-as-code.md` auth section and its five subcommand usage lines, the two
   `python-api.md` passages, and a
   `CHANGELOG.md` **Unreleased** entry.

## Files to Modify

- `python/micromegas/micromegas/cli/config.py`
- `python/micromegas/micromegas/cli/web_auth.py` *(new)*
- `python/micromegas/micromegas/cli/screens.py`
- `python/micromegas/micromegas/cli/grants.py`
- `python/micromegas/micromegas/cli/groups.py`
- `python/micromegas/micromegas/cli/import_keys.py`
- `python/micromegas/micromegas/cli/setup_telemetry.py`
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
  `grants`/`groups`/`import-keys`, where it would actually remove a required flag. The cost to
  accept knowingly: nothing checks that the selected profile belongs to the server in
  `micromegas-screens.json`, so `apply --profile prod` in a staging checkout authenticates against
  prod's IdP and pushes to staging. That is already true of `--url` + `--profile` on the other
  three CLIs, and a token minted for one issuer is rejected by a server trusting another, so the
  realistic failure is a 401 rather than a cross-environment write.
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
  resolved `client_secret`, so no existing browser-login setup silently changes grant type — and it
  is evaluated *first*, before `resolve_connection`, so a complete env credential never fails on
  profile selection. This keeps `grants`/`groups`/`import-keys` byte-compatible with today on a box
  that has a `profiles` map and exported OIDC env vars.
- Accepted regression: on a machine that has a `profiles` map and no profile selected (no
  `--profile`, no `MICROMEGAS_PROFILE`, no `default_profile`), `micromegas-screens` now fails with
  the "no profile selected" `ProfileError` from `resolve_connection`, where it previously either
  logged in off a complete-enough OIDC env triple or silently built an unauthenticated client —
  including the no-env-vars-at-all case, the exact local-dev-against-a-`--disable-auth`-monolith
  invocation `--no-auth` exists for. `make_client` appends the same `--no-auth` hint to this error
  as to its own diagnostic, so every such failure names the fix. This is the same rule
  `micromegas-query` already enforces and documents (`python-api.md:797`); the fix is to select a
  profile, set `default_profile`, or pass `--no-auth`. The full env triple (CI) is unaffected by
  step 1's ordering. A box with no `~/.micromegas/config.json` or a flat config is also unaffected
  by step 1's ordering, but is not unaffected by this change overall: it hits the same new strict
  "no auth resolved" error (previously an unauthenticated client) and needs `--no-auth`.
- `api_key_file` stays out of the web path, and a profile that resolves one gets a diagnostic
  naming the server-side reason rather than the generic "no auth mechanism" sentence. See
  Current State § "The analytics web API is OIDC-only".
- `--no-auth` is spelled to echo the server's `--disable-auth` flag in its help text; no env-var
  equivalent is added.
- `micromegas-screens.json` gains no `profile` key — a file-level default would invert the
  documented `--profile` > `MICROMEGAS_PROFILE` precedence; addable later.

## Documentation

- `mkdocs/docs/web-app/notebooks/screens-as-code.md` — rewrite **Authentication**: profile
  resolution and `--profile` (typed after the subcommand), the per-profile token cache, the
  explicit failure when nothing resolves, and `--no-auth` for a `--disable-auth` server. Keep the
  existing CI example (the env triple still works, and is still checked first) and note it needs no
  profile. State that `api_key_file` is not an option here and why, pointing at
  `micromegas-query` for the static-key workflow. Also update the **Commands** section's usage
  lines for the five client subcommands (`import`, `pull`, `plan`, `apply`, `list`) to add
  `[--profile NAME] [--no-auth]`, leaving `init`'s usage line unchanged. Every example in this
  rewrite must type the two flags after the subcommand, matching Design §3's flag placement.
- `mkdocs/docs/query-guide/python-api.md` — rewrite `:843-846` to give the *reason* rather than
  listing which CLIs happen to honor `api_key_file`: the analytics web API validates OIDC tokens
  only, so `api_key_file` is a FlightSQL credential (`micromegas-query`, `connect_with_profile()`).
  Of the `WebClient` CLIs, only `micromegas-screens` reports an `api_key_file`-only profile as an
  error; `micromegas-grants`, `-groups`, `-import-keys`, and `-setup-telemetry` still connect
  unauthenticated on that path. Delete the "not profile-aware" paragraph at `:914-918` outright,
  with no replacement sentence — screens now falls under the existing token-cache paragraph
  directly above it.
- `CHANGELOG.md` — one **Unreleased** entry under the existing `**Python:**` heading covering the new
  `--profile`/`--no-auth` flags on `micromegas-screens`, the per-profile token cache, the strict
  error with its `api_key_file`-specific diagnostic, and the accepted regression above. No
  behavior change to advertise for `grants`/`groups`/`import-keys` — that part is a refactor.

## Testing Strategy

All unit tests, no live service — `resolve_web_auth` is pure resolution over a `config_path` the
test writes into `tmp_path`, and the three provider constructors are the seams to patch. The
`tests/cli/conftest.py` autouse fixture already scrubs `MICROMEGAS_*`, so env-var cases are set
explicitly with `monkeypatch.setenv`.

`tests/cli/test_web_auth.py`:

- Profile whose only auth is `api_key_file` → `(None, diagnostic)`, the diagnostic naming
  `api_key_file` and saying the web API is OIDC-only; the key file is never opened (point it at a
  path that does not exist, and assert no `OSError`).
- Profile with `issuers`/`client_id`, no env secret → `oidc_connection.load_or_login` (monkeypatched
  on the `oidc_connection` module attribute) receives `token_file=~/.micromegas/tokens-<profile>.json`,
  plus the profile's issuer, client id, and audience.
- All three `MICROMEGAS_OIDC_*` set → `OidcClientCredentialsProvider.from_env` (monkeypatched as a
  classmethod, so no OIDC discovery request is made) is the branch taken, and `load_or_login` is
  not called.
- Env secret set but issuer/client_id coming from the profile → the `load_or_login` branch, *not*
  client-credentials (pins the Decisions entry above).
- `MICROMEGAS_OIDC_ISSUER` overrides the profile's issuer in the `load_or_login` call.
- Profile with both `api_key_file` and a complete OIDC pair → `ProfileError` propagates.
- Full env triple set **and** a `profiles` map with no profile selected → client-credentials, no
  `ProfileError`. This pins step 1's ordering, which is what keeps `grants`/`groups`/`import-keys`
  behavior-compatible; an implementation that resolved the config first would fail here.
- Nothing configured (missing config file, no profile selected) → `(None, diagnostic)`, the generic
  diagnostic naming both the profile keys and the env vars.
- A `profiles` map whose selected profile resolves neither OIDC nor `api_key_file` →
  `(None, diagnostic)`, the diagnostic containing the profile name.

`tests/cli/test_screens_auth.py`: these `make_client` cases monkeypatch
`screens.web_auth.resolve_web_auth` to return a canned `(None, diagnostic)` or `(provider, None)`
rather than exercising real config resolution — `test_web_auth.py` already covers that via
`config_path=tmp_path` — so they never touch a developer's actual `~/.micromegas/config.json`.

- `make_client` with `resolve_web_auth` returning `(None, diagnostic)` and `no_auth=False` →
  `ProfileError` whose message contains the diagnostic and `--no-auth`.
- Same with `no_auth=True` → a `WebClient` with `auth_provider is None` and
  `base_url == config["server"]`, and `resolve_web_auth` is not called at all (`make_client`'s
  `--no-auth` short-circuit).
- `resolve_web_auth` returning `(provider, None)` → the provider is passed through to the
  `WebClient` and the `server` URL is honored.
- `resolve_web_auth` raising `ProfileError` (e.g. no profile selected) → `make_client` re-raises a
  `ProfileError` whose message contains the original text plus the `--no-auth` hint.
- Parser wiring, parametrized over `(subcommand, extra_argv)` for `import`/`pull`/`plan`/`apply`/
  `list` (`import`'s `names` positional is `nargs="+"`, so its case supplies a dummy screen name):
  `main()` builds its parser and dispatches internally, so these tests monkeypatch `sys.argv` to
  `["micromegas-screens", cmd, *extra_argv, "--profile", "prod"]` (e.g. `extra_argv=["some-screen"]`
  for `import`, `[]` otherwise) and monkeypatch the module-level `cmd_*` function `main()` will call
  (`set_defaults(func=cmd_*)` binds it at parse time, so the patch is in place before dispatch) to
  capture the `Namespace` it receives; assert `args.profile == "prod"` and `args.no_auth is False`,
  and that `--no-auth` sets it True — this is what catches a subcommand accidentally left off
  `parents=[client_args]`, which would otherwise only fail at runtime with an `AttributeError`.
- `init` rejects `--profile` (it contacts no server): monkeypatch `sys.argv` to
  `["micromegas-screens", "init", "https://example.com", "--profile", "prod"]` (supplying `init`'s
  required `server_url` positional so the test fails only on the unrecognized `--profile`, not on a
  missing positional) and assert `main()` raises `SystemExit(2)` (argparse's exit code for an
  unrecognized argument).

`tests/cli/test_grants.py` / `test_groups.py` / `test_import_keys.py` keep passing unchanged (the
`build_auth_provider` seam is preserved) — none of them exercise the delegated body, so
`test_web_auth.py`'s env-triple-ordering case is the only guard on these three CLIs' behavior
preservation.

## Manual Verification

One check, for the piece no unit test covers: that a real browser login writes and then reuses the
per-profile token cache across two different CLIs.

1. With a `~/.micromegas/config.json` profile `local` carrying a real `client_id`/`issuers`, run
   `micromegas-query --profile local --all "SELECT 1"` and complete the browser login. Expect
   `~/.micromegas/tokens-local.json` to exist.
2. In a screens directory, run `micromegas-screens list --profile local`. Expect the inventory to
   print with **no** browser window — the issue's symptom 3, which no unit test can demonstrate
   because it depends on the real OIDC round-trip and the real IdP's refresh token.
