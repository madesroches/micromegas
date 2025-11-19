# Gateway Configuration

## Environment Variables

| Variable | Description | Example |
|----------|-------------|---------|
| `MICROMEGAS_FLIGHTSQL_URL` | FlightSQL backend endpoint (required) | `grpc://127.0.0.1:50051` |
| `MICROMEGAS_GATEWAY_HEADERS` | Header forwarding config (optional) | See below |

## Header Forwarding

Configure which HTTP headers are forwarded to FlightSQL:

```bash
export MICROMEGAS_GATEWAY_HEADERS='{
  "allowed_headers": ["Authorization", "X-Request-ID"],
  "allowed_prefixes": ["X-Custom-"],
  "blocked_headers": ["Cookie"]
}'
```

| Field | Description |
|-------|-------------|
| `allowed_headers` | Exact header names to forward (case-insensitive) |
| `allowed_prefixes` | Forward all headers matching prefix (e.g., `X-Custom-*`) |
| `blocked_headers` | Headers to block (overrides allows) |

**Default headers (if not configured):**
- `Authorization`, `X-Request-ID`, `X-User-ID`, `X-User-Email`, `User-Agent`
- Blocks: `Cookie`, `Set-Cookie`, `X-Client-IP`

**Security:**
- `X-Client-IP` is always blocked (gateway sets from actual connection)
- Blocked headers checked first, then exact matches, then prefix matches
- All matching is case-insensitive

## Origin Tracking

Gateway automatically sets these headers:

| Header | Description | Example |
|--------|-------------|---------|
| `x-client-type` | Client type + `+gateway` suffix | `web+gateway` |
| `x-request-id` | Generated UUID if not provided | `550e8400-...` |
| `x-client-ip` | Real client IP (prevents spoofing) | `192.168.1.100` |

## Examples

**Default configuration:**
```bash
export MICROMEGAS_FLIGHTSQL_URL=grpc://127.0.0.1:50051
cargo run --bin http-gateway-srv
```

**Custom headers:**
```bash
export MICROMEGAS_GATEWAY_HEADERS='{
  "allowed_headers": ["Authorization", "X-Tenant-ID"],
  "allowed_prefixes": ["X-Custom-"],
  "blocked_headers": ["Cookie"]
}'
export MICROMEGAS_FLIGHTSQL_URL=grpc://127.0.0.1:50051
cargo run --bin http-gateway-srv
```

## Server Options

```bash
cargo run --bin http-gateway-srv -- --listen-endpoint-http 0.0.0.0:3000
```

Default listen address: `0.0.0.0:3000`

## Request Limits

- Maximum SQL size: 1 MB
- Empty SQL queries rejected (400 Bad Request)

## Reference

- Implementation: `rust/http-gateway/src/config.rs`
- Origin metadata: `rust/public/src/servers/http_gateway.rs`
