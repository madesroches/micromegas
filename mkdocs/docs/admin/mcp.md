# MCP Server

The MCP server lets agents that cannot run the `micromegas-query` CLI (hosted assistants, IDE agents without a shell) query micromegas over the [Model Context Protocol](https://modelcontextprotocol.io). It is a `/mcp` route served by `analytics-web-srv` (and so by the monolith's `web` role) using the Streamable HTTP transport in stateless mode.

The route validates the bearer token itself (OIDC token or analytics API key, the same chain as FlightSQL) and forwards it to FlightSQL, which stays the enforcement point: identity, audience scoping, deny list, admission, scan cap and audit all apply to every query.

## Enabling

The route is off by default. Set on `analytics-web-srv` (or the monolith):

| Variable | Default | Meaning |
|---|---|---|
| `MICROMEGAS_MCP_ENABLED` | `false` | Mount `/mcp`. When off, `/mcp` answers 404. |
| `MICROMEGAS_MCP_FLIGHTSQL_URL` | unset | FlightSQL target. When unset the default data source is used. If neither exists the route is not mounted. Callers never choose a target. |
| `MICROMEGAS_MCP_SCREEN_WRITES` | `false` | Register the screen and folder write tools. |
| `MICROMEGAS_MCP_DEFAULT_ROWS` | `1000` | Rows returned when a call gives no `limit`. |
| `MICROMEGAS_MCP_MAX_ROWS` | `10000` | Largest accepted `limit`. |
| `MICROMEGAS_MCP_MAX_BYTES` | `131072` | Byte cap on a rendered result. |
| `MICROMEGAS_MCP_ALLOWED_HOSTS` | `localhost,127.0.0.1,::1` | Comma-separated `Host` allowlist; requests for another host get 403. Set it to the public hostname. |
| `MICROMEGAS_MCP_ALLOWED_ORIGINS` | empty | Comma-separated `Origin` allowlist (with scheme). Empty disables Origin validation. No CORS headers are added to `/mcp`. |
| `MICROMEGAS_MCP_PUBLIC_URL` | origin of `MICROMEGAS_AUTH_REDIRECT_URI` + the MCP path | Full external URL of the MCP route, for deployments where it differs from the redirect URI's origin. Must end in the MCP path (`/mcp`); otherwise the route is not mounted. |
| `MICROMEGAS_MCP_SKILLS_DIR` | unset | Directory of extra skills (`<name>/SKILL.md`), read once at startup. Symlinks are not followed. |

Row and byte limits cannot exceed the compiled-in ceilings (10,000 rows, 1 MiB). The route is not mounted under `--disable-auth`.

The path is `{MICROMEGAS_BASE_PATH}/mcp`, for example `http://127.0.0.1:3000/mcp`.

## Client configuration

### OAuth sign-in

The 401 answer of `/mcp` carries a `resource_metadata` parameter pointing at `{base_path}/.well-known/oauth-protected-resource`, an unauthenticated document (RFC 9728) that names the authorization server: the first issuer of `MICROMEGAS_OIDC_CONFIG`, the same one the web login uses. A client that supports MCP authorization can then run the browser sign-in, PKCE and token refresh itself. The advertised `resource` URL is the origin of `MICROMEGAS_AUTH_REDIRECT_URI` plus the MCP path, or `MICROMEGAS_MCP_PUBLIC_URL` when set; it is never derived from the request `Host` header.

For Claude Code, give the URL and the app's client id in `.mcp.json`:

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

The same can be added with `claude mcp add --transport http --client-id <id> --callback-port 48080 ...` (and `claude mcp add-json` for the fields the flags do not cover). Then run `/mcp` in the client and sign in once.

- Providers such as Google, Entra and Okta do not offer dynamic client registration, so the client needs a pre-registered client id (the web app's, which is the issuer's `audience` in `MICROMEGAS_OIDC_CONFIG`), and `http://localhost:<callbackPort>/callback` must be registered as a redirect URI on that app.
- The bearer the client presents must pass the server's validation: a JWT from a configured issuer whose `aud` is a configured `audience`. Providers that issue JWT access tokens for a configured API audience (Entra custom API, Auth0, Okta) work when that audience is in `scopes`. Google's access tokens are opaque and fail validation; use the static header below on such a deployment.
- `authServerMetadataUrl` points at the issuer's OIDC discovery document because the client's fallback is RFC 8414 discovery only, which not every provider serves.
- The metadata route is found through the 401 challenge. Behind a proxy that serves the MCP route at a different external path or origin, set `MICROMEGAS_MCP_PUBLIC_URL`, and forward the external `.well-known/oauth-protected-resource` sibling of the MCP URL to `{base_path}/.well-known/oauth-protected-resource`. When MCP is not mounted, that path answers 404.
- A configured `Authorization` header disables OAuth for that server.

### Static header

Send an analytics API key (or an OIDC token) as a static `Authorization` header:

```json
{
  "mcpServers": {
    "micromegas": {
      "type": "http",
      "url": "https://analytics.example.com/mcp",
      "headers": { "Authorization": "Bearer <analytics key or OIDC token>" }
    }
  }
}
```

A missing or invalid token gets `401` with a `WWW-Authenticate` header. The OIDC audience of the token must be one FlightSQL accepts (the web app client id must be in its OIDC configuration). An OIDC token expires; an analytics key does not.

## Tools

Query tools (all read-only):

| Tool | Behavior |
|---|---|
| `prepare_query(sql, begin?, end?, limit?)` | Plans the query without running it: schema, estimated scan size, estimate precision and whether it is within the scan cap, when the server reports them. |
| `execute_query(sql, begin?, end?, limit?)` | Runs the query and returns a markdown table, cut at the row cap (`limit`, default and maximum above) and the byte cap, with a `truncated at N rows` line when cut. Takes the same arguments as `prepare_query`. |
| `list_views()` | View names with the first line of their description. |
| `describe_view(name)` | Description, columns and sort order of a view. |

`begin` and `end` are RFC 3339 timestamps or relative ages such as `1h` and `7d`. There is no paging: aggregate or filter instead.

Only a single `SELECT`-style query is accepted. DDL, DML, `SET`, several statements, the mutating table functions and UDFs (`retire_partitions`, `materialize_partitions`, `regenerate_partitions`, `deny_queries`, `retire_partition_by_file`, `retire_partition_by_metadata`, `remove_query_denial`) and every other admin-only function are refused even for admin callers. This check is a guardrail, not a security boundary: FlightSQL enforces permissions.

Screen tools, which manage the same screens and folders as the web app: `list_screen_types`, `get_screen_type_default`, `list_screens`, `get_screen`, `list_folders` (read-only), and, with `MICROMEGAS_MCP_SCREEN_WRITES=true`, `create_folder`, `create_screen`, `update_screen` (with an optional `expected_updated_at` guard against overwriting a concurrent edit) and `delete_screen`. Write tools require an OIDC identity: an analytics key gets the read tools only. A notebook's SQL is run through the guarded query tools; there is no "run notebook" tool.

## Guidance

The `initialize` result carries the mandatory query rules as server `instructions`, the tool descriptions repeat them, and the `micromegas-query` and `micromegas-notebooks` skills are served over the MCP skills extension (`io.modelcontextprotocol/skills`) for clients that support it.

## Attribution in the audit log

Every FlightSQL call made on behalf of an MCP request carries `x-client-type: mcp`, `x-client-agent` (the MCP client's `User-Agent`, or `unknown`), a fresh `x-request-id` and the caller's IP (from `X-Forwarded-For` / `X-Real-IP` when present, so a trusted proxy is assumed, as for FlightSQL). The audit record's `client` identifies `mcp`:

```sql
SELECT msg FROM log_entries WHERE target = 'flightsql_query_audit' ORDER BY time DESC LIMIT 10
```

Screen mutations log the MCP client agent on the web server's info log.
