# MCP OIDC Discovery Plan

Follow-up to `tasks/completed/1759_mcp_server_plan.md`.

## Overview

The `/mcp` route accepts an OIDC token or analytics key as a static `Authorization` header, so
a user has to fetch and paste a token that expires. MCP clients such as Claude Code can instead
run the OAuth login themselves (browser, PKCE, refresh) when the server tells them which
authorization server to use. This adds that signal: an unauthenticated RFC 9728
protected-resource metadata document and a `resource_metadata` pointer in the 401 challenge. The
user then configures a URL and the app's client id, signs in once in the browser, and never
handles a token.

## Current State

- `mcp_auth.rs:15-26`: the 401 challenge is the constant `Bearer realm="micromegas"`; nothing
  tells a client where to authenticate.
- `web_server.rs:566` (`build_mcp_routes`) mounts `/mcp` behind `mcp_bearer_middleware`;
  disabled MCP answers an explicit 404 on that path only.
- The server already knows its login issuer and public origin: `OidcClientConfig`
  (`analytics-web-srv/src/auth/config.rs`) holds the first configured issuer, its client id (the
  issuer's `audience`) and `MICROMEGAS_AUTH_REDIRECT_URI`, which is required for the web app to
  start. `build_mcp_mount` (`web_server.rs:583`) does not receive it.
- Token validation (`auth/src/oidc.rs`) accepts any JWT from a configured issuer whose `aud`
  equals a configured `audience`: ID tokens (audience = client id) and JWT access tokens
  (audience = API). Opaque access tokens cannot validate.
- Claude Code (documented): checks `/.well-known/oauth-protected-resource`, then RFC 8414
  `/.well-known/oauth-authorization-server`; defaults to dynamic client registration and takes a
  pre-registered `clientId`, a fixed `callbackPort` (redirect `http://localhost:<port>/callback`),
  `scopes` and an `authServerMetadataUrl` override (https) in `.mcp.json`'s `oauth` object. It
  also has `headersHelper`. Not documented: whether the `resource` parameter is sent, which
  token type is presented, and how `resource_metadata` is parsed.

## Design

### Metadata document

`GET {base_path}/.well-known/oauth-protected-resource`, no authentication, mounted only when
MCP is mounted:

```json
{
  "resource": "https://analytics.example.com/mcp",
  "authorization_servers": ["<first configured issuer>"],
  "bearer_methods_supported": ["header"]
}
```

`scopes_supported` is left out: the right scopes are IdP-specific and are given by the client
(`oauth.scopes`). The issuer is the first configured one, as for the web login
(`OidcClientConfig`), so operators already order `issuers` for that purpose.

### Public URL

`resource` and the metadata URL need the externally visible origin, which the process cannot
derive from a request without trusting the `Host` header. Default: the origin of
`MICROMEGAS_AUTH_REDIRECT_URI` plus the MCP path. New optional `MICROMEGAS_MCP_PUBLIC_URL`
(full URL of the MCP route) overrides it for deployments where the two differ. A pure function `(redirect_uri, mcp_path, override) ->
public_url` resolves it; the metadata URL is derived from the public URL by swapping the trailing
`/mcp` for `/.well-known/oauth-protected-resource`. `McpOAuthMetadata::new(public_url, issuer)`
builds the resource URL, the metadata URL and the JSON body. Both are unit-testable.

### Challenge

Every 401 from the middleware becomes
`Bearer realm="micromegas", resource_metadata="<metadata url>"` (plus `error="invalid_token"`
when a token was present and rejected). The middleware state carries the metadata URL; with no
OAuth metadata (unit tests, future embedders) it keeps today's constant challenge.

### Route placement

The metadata route sits outside the bearer middleware, next to the `/mcp` route in
`build_mcp_routes`. It is under `{base_path}` and is found through the 401 challenge's
`resource_metadata` parameter; it is not at the RFC 9728 well-known locations, so clients that
ignore the header will not find it. When MCP is disabled the metadata path would otherwise fall through to the SPA
fallback (200 `index.html`), so the disabled branch registers an explicit 404 for it as well.

### Client setup (documented)

```json
{
  "mcpServers": {
    "micromegas": {
      "type": "http",
      "url": "https://analytics.example.com/mcp",
      "oauth": {
        "clientId": "<client id>",
        "callbackPort": 48080,
        "scopes": "<IdP scopes>",
        "authServerMetadataUrl": "<issuer>/.well-known/openid-configuration"
      }
    }
  }
}
```

(or `claude mcp add --transport http --client-id <id> --callback-port 48080 ...`, with
`add-json` for the fields the flags don't cover). Constraints the doc must state:

- Google, Entra, Okta and similar don't offer dynamic client registration, hence the
  pre-registered client id; `http://localhost:<port>/callback` must be registered on that app.
- The bearer Claude Code presents must pass the server's validation. Providers that issue JWT
  access tokens for a configured API audience (Entra custom API, Auth0, Okta) work with the
  API's audience in `scopes`. Google's access tokens are opaque and fail; a Google deployment
  keeps the static-header path.
- `authServerMetadataUrl` points at the issuer's OIDC discovery document because the documented
  fallback is RFC 8414 only, which not every provider serves.
- Analytics keys keep working as a static `headers` entry; a configured `Authorization` header
  disables OAuth for that server.

## Implementation Steps

1. `analytics-web-srv/src/mcp_auth.rs`: add `McpOAuthMetadata` (new, body, challenge builder) and
   a `metadata_handler`; give `mcp_bearer_middleware` state `{ provider, challenge }`.
2. `analytics-web-srv/src/web_server.rs`: add an `oauth: Option<McpOAuthMetadata>` field to
   `McpMount` (`web_server.rs:540-545`), filled by `build_mcp_mount`; `build_mcp_routes` mounts
   the metadata route and uses the OAuth challenge only when it is `Some`, else keeps today's
   constant challenge. Existing `McpMount` literals in `tests/mcp_routes_tests.rs` must set the
   field. Pass `&OidcClientConfig` into `build_mcp_mount`, taken
   from `auth_state` (`auth_state.as_ref().map(|s| &s.config)`, always `Some` when MCP mounts)
   rather than calling `from_env()` again; resolve the public URL (honour
   `MICROMEGAS_MCP_PUBLIC_URL`) and build `McpOAuthMetadata`; add the metadata route in
   `build_mcp_routes`; in the disabled branch keep the explicit 404 for `/mcp` and add one for
   the metadata path.
3. Tests (below).
4. Docs: rework the `## Client configuration` section of `mkdocs/docs/admin/mcp.md` with the
   OAuth setup above, keep the static header as the analytics-key option, document
   `MICROMEGAS_MCP_PUBLIC_URL`; update the MCP bearer paragraph in
   `mkdocs/docs/admin/authentication.md`; add the unauthenticated metadata route to the API
   Routes list in `mkdocs/docs/admin/web-app.md`.
5. `CHANGELOG.md` entry; SQL schemas unchanged.

## Files to Modify

- `rust/analytics-web-srv/src/mcp_auth.rs`
- `rust/analytics-web-srv/src/web_server.rs`
- `rust/analytics-web-srv/Cargo.toml` (`url.workspace = true`)
- `rust/analytics-web-srv/tests/mcp_routes_tests.rs`
- `mkdocs/docs/admin/mcp.md`, `mkdocs/docs/admin/authentication.md`,
  `mkdocs/docs/admin/web-app.md`
- `CHANGELOG.md`

## Trade-offs

- **Metadata document vs. only a doc'd `authServerMetadataUrl`.** The override alone would work
  for Claude Code, but other MCP clients rely on discovery; the document is ~30 lines and makes
  the server meet the MCP discovery requirement via the `resource_metadata` challenge.
- **Proxying or hosting an authorization server ourselves** (so Google-style providers work) was
  rejected: it makes micromegas an OAuth server, a large security surface for one provider's
  limitation.

## Decisions

- Public origin comes from configuration, never from the request `Host` header.
- The metadata route is unauthenticated by design (RFC 9728) and carries no secret: issuer URL
  and resource URL are already public.

## Testing Strategy

No-DB tests in `analytics-web-srv/tests/mcp_routes_tests.rs`:

- The public-URL resolution function yields the redirect-URI-derived URL by default and the
  override when set; `McpOAuthMetadata::new` produces the expected resource URL, metadata URL
  and body.
- A request without a token gets 401 whose `WWW-Authenticate` contains `resource_metadata`;
  with a rejected token it also contains `error="invalid_token"`.
- The metadata route answers 200 JSON without credentials when MCP is mounted, and 404 when it
  is not; the 404 test covers both the `/mcp` and the metadata path.

## Manual Verification

1. Register an app on the IdP with redirect `http://localhost:48080/callback`; start the monolith
   with `MICROMEGAS_MCP_ENABLED=true` and `MICROMEGAS_OIDC_CONFIG` set so auth is enabled (e.g.
   `start_services.py --monolith` with the OIDC env and an empty base path; without it
   `start_services.py` passes `--disable-auth` and `/mcp` 404s).
2. `curl -i http://127.0.0.1:3000/mcp` and check the `resource_metadata` parameter; `curl` that
   URL and check the JSON.
3. Add the server with the `.mcp.json` above (`url` set to `http://localhost:3000/mcp`, since the
   advertised resource host comes from the redirect URI, which is localhost), run `/mcp` in Claude Code, complete the browser
   sign-in, then run `list_views`. Expected: the tools work and no token was typed. The IdP-side
   behavior (token type presented, `resource` parameter, discovery fallback) is not documented by
   the client; this is the check. It needs a real IdP and a real client, so it is not automated.

## Open Questions

- Whether to add a `micromegas-token` command (prints the cached OIDC token) so Google
  deployments can use `headersHelper`. Not needed for providers with JWT access tokens.
