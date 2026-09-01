# Python Client: Static-Token Auth Provider and Discoverable Profile Connect

Closes #1547 and #1546.

## Overview

Two gaps in the published Python client, both landing on the same two files. A user holding a
minted analytics API key has no shipped `AuthProvider` to pass to the `auth_provider=` parameter
the client's own docstring calls "recommended", and the profile-aware connect that reads
`~/.micromegas/config.json` is buried in `micromegas.cli.connection` where `help(micromegas.connect)`
never mentions it. This plan adds `StaticTokenAuthProvider` to `micromegas.auth`, teaches profiles
a static-key source (`api_key_file`), and lifts the profile connect out of the `cli` package to a
top-level `micromegas.connect_with_profile()`.

They are done together because both converge on `cli/connection.py`'s auth branch and
`cli/config.py`'s `ConnectionConfig`: splitting them means editing that branch twice and shipping a
`StaticTokenAuthProvider` that no profile can reach.

## Current State

**Auth providers** — `python/micromegas/micromegas/auth/` holds `oidc.py` only, exporting
`OidcAuthProvider` (browser/PKCE, refresh, token file) and `OidcClientCredentialsProvider`
(service account, `from_env()`). `auth/__init__.py:5` lists exactly those two in `__all__`.

The provider protocol is a single method, `get_token() -> str`. Both consumers turn it into the
same header:

- `DynamicAuthMiddleware.sending_headers()` (`flightsql/client.py:53-56`) →
  `{"authorization": b"Bearer <token>"}` on every FlightSQL call.
- `WebClient._headers()` (`web_client.py:20-25`) → the same `Authorization` header on every REST
  call.

So a static provider is a three-line class, and it works for `WebClient` for free.

**Server side confirms the wire shape** — `DbApiKeyAuthProvider::validate_request`
(`rust/auth/src/db_api_key.rs:251-254`) reads `parts.bearer_token()` and does a hash-indexed
lookup against `analytics_api_keys`. The minted `mmk_...` key travels verbatim as the bearer
token; nothing wraps or exchanges it. The Grafana plugin already consumes it exactly this way
(`mkdocs/docs/grafana/authentication.md:28-35`).

**Three connect paths exist today:**

| Path | Signature | Reads config? | Auth |
|------|-----------|---------------|------|
| `micromegas.connect` (`__init__.py:13`) | `(uri=None, preserve_dictionary=False)` | no | none |
| `micromegas.oidc_connection.connect` | `(uri, issuer, client_id, ...)` | no | OIDC, explicit args |
| `micromegas.cli.connection.connect` | `(profile=None, client_entrypoint=None)` | yes | OIDC or none |

The third is the one users want and the one they cannot find. Its whole body is 25 lines:
`resolve_connection(profile)`, then branch on `cfg.oidc_issuer and cfg.oidc_client_id` between
`oidc_connection.connect(...)` and a bare `FlightSQLClient(cfg.uri, client_entrypoint=...)`.

**Profile config** — `cli/config.py` is CLI-specific in its module path only. It already provides
`ConnectionConfig` (frozen, slots), `resolve_connection()` with env > profile > default precedence,
`resolve_active_profile()` with `--profile` > `MICROMEGAS_PROFILE` > `default_profile`,
`ProfileError`, `_validate_profile_name`, and per-profile `tokens-<name>.json`. It has no
static-key field. Its direct callers are `cli/connection.py`, `cli/grants.py:45`, and
`cli/import_keys.py:163`; `cli/setup_telemetry.py` calls it only indirectly, via
`import_keys.make_client` (`setup_telemetry.py:32`).

## Design

### 1. `StaticTokenAuthProvider`

New file `micromegas/auth/static_token.py`, exported from `auth/__init__.py`'s `__all__`.

```
class StaticTokenAuthProvider:
    def __init__(self, token: str)
    @classmethod
    def from_file(cls, path) -> "StaticTokenAuthProvider"
    def get_token(self) -> str
    def __repr__(self) -> str
```

- `__init__` stores the token stripped, and raises `ValueError` when it is not a string or is
  empty after stripping. A blank token would otherwise fail as an opaque server-side
  authentication error on the first query rather than at the call site.
- `from_file(path)` expands `~`, reads UTF-8, strips. `echo key > file` leaves a trailing
  newline, so stripping is the normal case, not a nicety. An empty-after-strip file raises
  `ValueError` naming the path; `OSError` from an unreadable path propagates unchanged —
  `connect_with_profile` (§4) is what translates both into a `ProfileError`.
- No `from_env()`. See Decisions.
- `__repr__` masks the token. A notebook echoes the last expression into saved output, and
  `StaticTokenAuthProvider(...)` as a cell's value must not write a live credential into a
  committed `.ipynb`. The token is held in `self._token` for the same reason. This is the one
  place in the class that needs a `why` comment.

### 2. Profile static-key source: `api_key_file`

`ConnectionConfig` gains one field:

```
api_key_file: Optional[str] = None
```

It holds the **path**, not the secret. Two consequences, both wanted: the frozen dataclass's
auto-generated `repr` can never leak a key (a `print(cfg)` while debugging is a realistic leak
path), and `resolve_connection` performs no file I/O, so it stays pure config resolution and the
three other callers (`grants`, `import_keys`, `setup_telemetry`) are unaffected by the file I/O
(they can still hit §3's new two-mechanism `ProfileError`, which each already catches). The file
is read exactly once, at connect time, by `StaticTokenAuthProvider.from_file()` — the
read/strip/validate logic lives in one place.

Resolution: from the active profile's (or flat config's) `api_key_file` key only. No environment
variable is introduced — see Decisions.

### 3. One profile, one auth mechanism

`resolve_connection` raises `ProfileError` when it resolves both a static key and a complete OIDC
pair (`oidc_issuer` **and** `oidc_client_id`). This is the single choke point that sees both, so
every consumer — the FlightSQL connect and all three `WebClient` tools — fails the same way,
instead of one tool silently preferring OIDC while another prefers the key.

Because OIDC values can arrive from `MICROMEGAS_OIDC_ISSUER` / `MICROMEGAS_OIDC_CLIENT_ID`, the
conflict is reachable without the profile itself naming an issuer. The message must therefore name
the real source of each side, e.g.:

```
profile 'prod' resolves two auth mechanisms: a static API key (profile key 'api_key_file')
and OIDC (issuer from MICROMEGAS_OIDC_ISSUER, client_id from profile key 'client_id').
A profile must use exactly one -- remove 'api_key_file', or unset the OIDC settings.
```

`resolve_active_profile` returns a `None` name for a flat config (no `profiles` map), so the
message leads with "config file" instead of `profile '<name>'` when the name is `None`, e.g.
`config file resolves two auth mechanisms: ...`.

Provenance is recomputed inside the error path (re-checking whether each `MICROMEGAS_OIDC_*` var is
set) rather than threaded through `_pick`'s return type — it is needed for the message and nowhere
else.

`ProfileError`'s class docstring (`cli/config.py:15-21`) currently promises "profile-selection
problems only ... never for downstream connection failures". It already also covers malformed
`profiles`/entry shapes, so the docstring is behind the code; extend it to cover profile
*content* problems — an unreadable or empty `api_key_file`, and a two-mechanism profile — while
keeping the exclusion of transport/auth failures from the resulting connection. This matters
because `cli/query.py:143` turns `ProfileError` into a clean `parser.error` instead of a traceback.

### 4. `micromegas/connection.py` and the top-level export

New module `micromegas/connection.py`:

```
def connect_with_profile(profile=None, client_entrypoint=None, preserve_dictionary=False)
```

Three branches, in order:

```
cfg = resolve_connection(profile=profile)          # raises ProfileError on a two-mechanism profile
if cfg.api_key_file:      -> FlightSQLClient(cfg.uri, auth_provider=StaticTokenAuthProvider.from_file(...),
                                              client_entrypoint=..., preserve_dictionary=...)
elif cfg.oidc_issuer and cfg.oidc_client_id:
                          -> oidc_connection.connect(..., client_entrypoint=..., preserve_dictionary=...)
else:                     -> FlightSQLClient(cfg.uri, client_entrypoint=..., preserve_dictionary=...)
```

Static key first, so a profile that names one never triggers a browser flow. The
`OSError`/`ValueError` from `from_file` is wrapped into `ProfileError` naming the `api_key_file`
key, the path, and the underlying error, per §3's widened `ProfileError` scope. `resolve_connection`
returns only a `ConnectionConfig` (no profile name), so the message cannot name the profile itself.

Two details to preserve:

- **Keep the `FlightSQLClient` and `oidc_connection` imports function-local.** The existing tests
  monkeypatch the module attributes `flightsql_client.FlightSQLClient` and
  `oidc_connection.connect` (`tests/cli/test_connection.py:31,73`) and depend on the name being
  looked up at call time. A module-level `from ... import FlightSQLClient` would bind the real
  class at import and silently break that test.
- **`preserve_dictionary` is new and reaches all three branches.** Today the profile connect
  cannot request dictionary preservation at all — a real gap for notebook users.
- **`client_entrypoint` also reaches all three branches, including the new static-key one.**
  `cli/query.py` passes `client_entrypoint="cli-query"` through this function; dropping it on the
  static-key branch would silently lose attribution for that auth mode.

`micromegas/cli/connection.py` collapses to a re-export:

```
from micromegas.connection import connect_with_profile as connect
```

`cli/query.py:141` (`connection.connect(...)`) keeps working untouched.

`micromegas/__init__.py` adds `connect_with_profile` and re-exports `ProfileError`, so a caller can
catch the error without importing from a `cli` submodule — the same discoverability complaint the
issue raises about `connect`. No `__all__` is introduced (adding one would change the existing
`from micromegas import *` surface).

**No import cycle.** `micromegas/connection.py` imports `micromegas.cli.config` at module level;
`cli/__init__.py` is a docstring only and `cli/config.py` imports stdlib only. `oidc_connection.py`
already does the same shape (`from micromegas.auth import OidcAuthProvider` at module level while
being imported from `__init__.py`), so the pattern has precedent in this package.

### 5. Docstring cross-references

Issue #1546's minimum bar, and cheap:

- `micromegas.connect()` — state plainly that it takes a single URI, never reads the config file,
  and never authenticates; point at `connect_with_profile()` for named profiles and auth.
- `connect_with_profile()` — full args, one example per auth mode, pointers back to `connect()`
  (plain URI) and `oidc_connection.connect()` (explicit-args OIDC).
- `FlightSQLClient.__init__`'s `auth_provider` line (`flightsql/client.py:215-217`) — name
  `StaticTokenAuthProvider` alongside `OidcAuthProvider`.
- `flightsql/attribution.py:68-73` enumerates by name every public entry point that accepts
  `client_entrypoint` and asserts the top-level `connect()` does not. Add
  `connect_with_profile(client_entrypoint=...)` to the list and make the parenthetical contrast
  the two top-level functions instead of implying there is one.

## Implementation Steps

**Phase 1 — provider**
1. Add `micromegas/auth/static_token.py` per §1.
2. Export `StaticTokenAuthProvider` from `micromegas/auth/__init__.py` (import + `__all__`).
3. Add `tests/auth/test_static_token.py`; append `"tests/auth/test_static_token.py"` to
   `HERMETIC_TEST_ARGS` in `build/python_ci.py` so CI collects it.

**Phase 2 — config**
4. Add `api_key_file` to `ConnectionConfig` and resolve it in `resolve_connection`
   (`cli/config.py`).
5. Add the two-mechanism `ProfileError` to `resolve_connection`, with source-naming message.
6. Extend `ProfileError`'s docstring to cover profile-content errors.
7. Extend `tests/cli/test_config.py`.

**Phase 3 — connect**
8. Add `micromegas/connection.py` with `connect_with_profile`, moving `cli/connection.py`'s body
   and adding the static-key branch plus `preserve_dictionary`.
9. Reduce `micromegas/cli/connection.py` to the aliasing re-export.
10. Export `connect_with_profile` and `ProfileError` from `micromegas/__init__.py`.
11. Extend `tests/cli/test_connection.py`, including giving `FakeFlightSQLClient.__init__`
    `preserve_dictionary=False` and `auth_provider=None` parameters, capturing both, so it keeps
    accepting the call and the static-key test can assert on them.

**Phase 4 — docstrings and docs**
12. Docstring edits per §5, including `flightsql/attribution.py`.
13. `mkdocs/docs/query-guide/python-api.md` edits per Documentation.
14. Cross-reference from `mkdocs/docs/admin/api-keys.md`; one-line pointer in
    `python/micromegas/README.md`; rewrite `mkdocs/docs/admin/authentication.md`'s "Python Client
    with API Keys" and `mkdocs/docs/query-guide/python-api-advanced.md`'s "Static Headers
    (Deprecated)" per Documentation.
15. `CHANGELOG.md` `## Unreleased` entry citing #1546 and #1547.

**Phase 5 — verify**
16. `poetry run black python/micromegas`, then `python3 ../../build/python_ci.py` (from
    `python/micromegas/`) to run the hermetic suite the same way CI does.
17. Optional manual pass (see Testing Strategy).

## Files to Modify

Create:
- `python/micromegas/micromegas/auth/static_token.py`
- `python/micromegas/micromegas/connection.py`
- `python/micromegas/tests/auth/test_static_token.py`

Modify:
- `python/micromegas/micromegas/auth/__init__.py`
- `python/micromegas/micromegas/cli/config.py`
- `python/micromegas/micromegas/cli/connection.py`
- `python/micromegas/micromegas/__init__.py`
- `python/micromegas/micromegas/flightsql/client.py` (docstring only)
- `python/micromegas/micromegas/flightsql/attribution.py` (docstring only)
- `python/micromegas/tests/cli/test_config.py`
- `python/micromegas/tests/cli/test_connection.py`
- `build/python_ci.py` (add `"tests/auth/test_static_token.py"` to `HERMETIC_TEST_ARGS`)
- `python/micromegas/README.md`
- `mkdocs/docs/query-guide/python-api.md`
- `mkdocs/docs/query-guide/python-api-advanced.md`
- `mkdocs/docs/admin/api-keys.md`
- `mkdocs/docs/admin/authentication.md`
- `CHANGELOG.md`

## Decisions

- `cli/config.py` stays put; only `ProfileError` is re-exported.
- `cli.connection.connect` stays as an alias rather than updating its call sites.
- Static keys reach a profile through `api_key_file` **only**. No new environment variable, under
  any name. The obvious spelling, `MICROMEGAS_ANALYTICS_API_KEY`, is one character from the
  deprecated server-side keyring `MICROMEGAS_ANALYTICS_API_KEYS` (`cli/import_keys.py:30`) and a
  literal prefix of the live `MICROMEGAS_ANALYTICS_API_KEY_CACHE_TTL_SECONDS` / `..._CACHE_SIZE`
  knobs (`mkdocs/docs/admin/api-keys.md:355`); a collision-free alternative
  (`MICROMEGAS_ANALYTICS_TOKEN`) was then considered and also declined, to keep one spelling per
  setting. A caller who wants env-var sourcing — dotenv included — writes
  `StaticTokenAuthProvider(os.environ["..."])` explicitly. Do not add such a variable later without
  revisiting this.
- No bundled CLI auto-loads a `.env` file, and `python-dotenv` does not become a dependency,
  optional or otherwise. A CLI whose connection target depends on a file in the current working
  directory is surprising, and the shell already solves it.
- No inline `api_key` string in `config.json`. The file carries no permission guarantee and is
  routinely pasted into issues and chats; a path to a `0600` file is the only config-file spelling.
- A profile resolving both a static key and OIDC is a `ProfileError`, not a precedence rule.
  Accepted consequence: a static key cannot be used as an ad-hoc override on top of an OIDC
  profile — that combination errors.
- Top-level `micromegas.connect()` does not gain a `profile=` parameter.
- `connect_with_profile` accepts `client_entrypoint` (the CLI already passes `"cli-query"` through
  this path) and defaults it to `None` so auto-detection yields `jupyter`/`script` for library
  callers.
- File permissions on `api_key_file` are documented (`0600` expected) but not enforced: a mode
  check has no portable meaning across platforms, and a warning users cannot act on is noise.
- The top-level function is named `connect_with_profile`, per the issue's own suggestion.
- The admin HTTP tools (`micromegas-grants`, `micromegas-import-keys`, `micromegas-setup-telemetry`)
  ignore `api_key_file`; a key-only profile yields an unauthenticated `WebClient`, unchanged from
  today's non-OIDC profile.
- `micromegas-screens` is documented as not profile-aware and stays out of scope for this plan.

## Documentation

`mkdocs/docs/query-guide/python-api.md`:
- `### Connection` (line 15) — add a "Connecting via a named profile" block showing
  `micromegas.connect_with_profile("prod")`, and state that plain `connect()` never reads the
  config file.
- `### FlightSQLClient(...)` (line 281) — add a `StaticTokenAuthProvider` example beside the
  existing `OidcAuthProvider` one, and mention it in the `auth_provider` bullet (line 311).
- `## Connection Configuration` (line 279) — new subsection on static analytics API keys: where a
  key comes from (mint via `POST /api/analytics-api-keys` or the Admin page), that it is sent
  verbatim as a bearer token, and the `0600` file expectation.
- Environment-variable table (line ~753) — **no new row** for the static key. Deliberate; the
  Decisions entry above is the record of why.
- Config-file key table (line ~780) — add `api_key_file`.
- **Named profiles** (line ~792) — document `api_key_file` with a JSON example, the one-mechanism
  rule, and the error text.
- `### micromegas-logout` (line 846) — a static-key profile caches no token, so logout is a no-op
  for it; revoking such a key is server-side (`DELETE /api/analytics-api-keys/{key_id}`).
- Line ~343 ("The top-level `micromegas.connect()` helper does not expose a `client_entrypoint`
  parameter …") — update to name `connect_with_profile` alongside `FlightSQLClient`,
  `oidc_connection.connect()`, and the now-aliased `cli.connection.connect()`, mirroring the
  `attribution.py` docstring edit above.

`mkdocs/docs/admin/api-keys.md` — from "Minting an analytics key over HTTP" (line ~249), point at
the Python static-key path so a freshly minted key has an obvious Python consumer.
`mkdocs/docs/grafana/authentication.md` already covers the Grafana consumer and needs no change.

`mkdocs/docs/admin/authentication.md` — rewrite "Python Client with API Keys" (line ~891) to build
a `StaticTokenAuthProvider` instead of passing a raw `headers=` dict, and retarget the deprecation
warning to name `StaticTokenAuthProvider`.

`mkdocs/docs/query-guide/python-api-advanced.md` — add a `StaticTokenAuthProvider` example to
"Advanced Connection Patterns" (line ~8) beside `OidcAuthProvider` and
`OidcClientCredentialsProvider`; retarget "Static Headers (Deprecated)" (line ~49) to name
`StaticTokenAuthProvider` instead of just saying "Use `auth_provider` instead".

`python/micromegas/README.md` — its two `micromegas.connect()` examples stay correct (they target a
local server); add one line pointing at `connect_with_profile` for a remote or authenticated
deployment.

`CHANGELOG.md` — one `## Unreleased` entry under a Python client bullet covering both issues: the
new provider, the new `api_key_file` profile key and its one-mechanism rule, the new top-level
`connect_with_profile` (noting `preserve_dictionary` reaches the profile path for the first time),
and that `micromegas.connect()` is unchanged.

## Testing Strategy

`tests/auth/test_static_token.py` (new):
- `get_token()` round-trips the token; surrounding whitespace is stripped.
- Empty, whitespace-only, and non-string tokens raise `ValueError`.
- `from_file` reads a key with a trailing newline and strips it; an empty file raises `ValueError`
  naming the path; a missing path raises `OSError`.
- `repr()` does not contain the token.
- Fed to `DynamicAuthMiddleware`, `sending_headers()` yields
  `{"authorization": b"Bearer <key>"}` — the property that makes the class useful. Follow
  `tests/test_flightsql_headers.py`'s style.

`tests/cli/test_config.py` (extend; `tests/cli/conftest.py` already scrubs every `MICROMEGAS_*`
var per test):
- `api_key_file` from a profile lands in `ConnectionConfig.api_key_file` verbatim, unexpanded (`~`
  expansion is `from_file`'s job, covered by the `tests/auth/test_static_token.py` bullets).
- A profile with both `api_key_file` and `issuers` + `client_id` raises `ProfileError`; the message
  names both sources.
- `api_key_file` in the profile plus `MICROMEGAS_OIDC_ISSUER`/`_CLIENT_ID` in the environment
  raises `ProfileError` naming the environment variables.
- `api_key_file` alone resolves cleanly, leaving every `oidc_*` field `None`.
- A flat config (no `profiles` map) with `api_key_file` behaves identically.
- A flat config with both `api_key_file` and `issuers` + `client_id` raises `ProfileError` with a
  message leading `config file resolves two auth mechanisms: ...`, not `profile 'None'`.

`tests/cli/test_connection.py` (extend; reuse the existing monkeypatched `config.CONFIG_PATH` and
fake-client pattern):
- A static-key profile builds a client whose captured `auth_provider.get_token()` returns the key
  file's contents, and whose captured `client_entrypoint` is forwarded too.
- `preserve_dictionary=True` is forwarded on each of the three branches.
- `micromegas.connect_with_profile is micromegas.cli.connection.connect` — the alias holds.
- An `api_key_file` pointing at a missing or empty file raises `ProfileError`.

Suite: `poetry run pytest tests/auth tests/cli -q` from `python/micromegas` (the integration tests
under `tests/auth` skip without a live identity provider). Format with `poetry run black`.

Optional manual pass, since no automated test exercises a real key end to end: mint an analytics
key through the Admin page, write it to `~/.micromegas/local.key` with mode `0600`, add a profile
naming that file, and run
`micromegas-query --profile local-key "SELECT 1" --all`. Requires a deployment with a populated
`analytics_api_keys` table — a `--disable-auth` local monolith accepts anything and proves nothing.
