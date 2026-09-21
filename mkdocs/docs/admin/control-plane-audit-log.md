# Control-Plane Mutation Audit Log

`analytics-web-srv` emits one structured JSON log line for every attempted mutation of the ABAC
control plane -- creating or deleting an audience grant, creating or deleting a group, adding or
removing a group member, and claiming a brand-new audience through self-service mint -- under the
dedicated `control_plane_audit` log target. A record is emitted for every attempt that reaches a
grant/group gate or handler, whether it was allowed or denied, so a rejected mutation is just as
visible as a successful one -- a record exists even when the request never reached the database.

This is the access-audit trail for [grants](authorization.md#the-grant-store) and
[groups](groups.md) administration -- the tables themselves only tell you the *current* state;
this log tells you who changed it, when, and whether the attempt succeeded.

## Best-effort, like every other log line

The record rides the same tracing sink every other log line does: fire-and-forget, and shed
under load like any other event. It is not a durable, guaranteed-delivery audit trail.

## Querying the audit log

The record lands in [`log_entries`](../query-guide/schema-reference.md#log_entries) like any
other log line, with `target = 'control_plane_audit'` and the JSON payload in `msg`. Always query
it with a bounded time range plus the `target` filter.

Parse `msg` with the
[JSON/JSONB functions](../query-guide/functions-reference.md#jsonjsonb-functions) (`jsonb_parse`,
`jsonb_get`, `jsonb_as_string`):

```sql
SELECT time, jsonb_parse(msg) AS j
FROM log_entries
WHERE target = 'control_plane_audit'
  AND time >= NOW() - INTERVAL '1 hour'
ORDER BY time DESC
LIMIT 20;
```

### Every mutation by one actor, in a window

```sql
WITH a AS (
  SELECT time, jsonb_parse(msg) AS j
  FROM log_entries
  WHERE target = 'control_plane_audit'
    AND time >= NOW() - INTERVAL '24 hours'
)
SELECT
  time,
  jsonb_as_string(jsonb_get(j, 'action'))  AS action,
  jsonb_as_string(jsonb_get(j, 'outcome')) AS outcome,
  jsonb_as_string(jsonb_get(j, 'audience')) AS audience,
  jsonb_as_string(jsonb_get(j, 'group'))   AS group_name,
  jsonb_as_string(jsonb_get(j, 'member'))  AS member
FROM a
WHERE jsonb_as_string(jsonb_get(j, 'actor')) = 'alice@example.com'
ORDER BY time DESC;
```

### Every non-`allowed` outcome, grouped by actor and `client_ip`

Denials and errors together -- useful for spotting a caller repeatedly probing a mutation they
have no authority for, or a client misconfigured against a store that isn't set up yet.

```sql
WITH a AS (
  SELECT time, jsonb_parse(msg) AS j
  FROM log_entries
  WHERE target = 'control_plane_audit'
    AND time >= NOW() - INTERVAL '24 hours'
)
SELECT
  jsonb_as_string(jsonb_get(j, 'actor'))     AS actor,
  jsonb_as_string(jsonb_get(j, 'client_ip')) AS client_ip,
  jsonb_as_string(jsonb_get(j, 'outcome'))   AS outcome,
  count(*) AS attempts
FROM a
WHERE jsonb_as_string(jsonb_get(j, 'outcome')) <> 'allowed'
GROUP BY actor, client_ip, outcome
ORDER BY attempts DESC;
```

### Every mutation touching one audience or group

```sql
WITH a AS (
  SELECT time, jsonb_parse(msg) AS j
  FROM log_entries
  WHERE target = 'control_plane_audit'
    AND time >= NOW() - INTERVAL '7 days'
)
SELECT
  time,
  jsonb_as_string(jsonb_get(j, 'actor'))   AS actor,
  jsonb_as_string(jsonb_get(j, 'action'))  AS action,
  jsonb_as_string(jsonb_get(j, 'outcome')) AS outcome,
  jsonb_as_string(jsonb_get(j, 'reason'))  AS reason
FROM a
WHERE jsonb_as_string(jsonb_get(j, 'audience')) = 'team-alpha'
   OR jsonb_as_string(jsonb_get(j, 'group')) = 'eng'
ORDER BY time DESC;
```

## Fields

Every caller-supplied field (`audience`, `axis`, `selector`, `group`, `member`, `reason`) is
truncated to 255 bytes with a trailing `...` marker if it's longer; a value ending in `...` may
be truncated rather than genuine data, and an equality filter on such a value can silently miss.

| Field | Type | Present | Description |
|-------|------|---------|--------------|
| `actor` | string | always | Caller email, else subject, else `"unauthenticated"` when no identity was available at all (a missing/misconfigured auth extension -- normally unreachable) |
| `is_admin` | bool | always | Whether `actor` was an administrator at the time of the attempt |
| `action` | string | always | `create_grant`, `delete_grant`, `claim_audience`, `create_group`, `delete_group`, `add_member`, or `remove_member` |
| `outcome` | string | always | `allowed`, `denied` (a caller-attributable authorization/validation refusal), or `error` (a server-side failure, e.g. the store isn't configured) |
| `client_ip` | string | always | The rightmost `X-Forwarded-For` entry, falling back to `X-Real-IP` and then the socket address -- the same resolution [`flightsql_query_audit`'s `client_ip`](../query-guide/query-audit-log.md#fields) uses. `unknown` if none is available |
| `audience` | string | grant/claim actions only | The audience named by the request |
| `axis` | string | grant actions only | `read` or `mint`. Absent on `claim_audience`, since a claim always writes both axes |
| `selector` | string | grant/claim actions only | The `*`/`user:<id>`/`group:<id>` selector named by the request |
| `group` | string | group actions only | The group name named by the request |
| `member` | string | `add_member`/`remove_member` only | The member selector named by the request |
| `created` | bool | `create_grant`/`add_member`, on `allowed` only | `false` when the row already existed (idempotent create), `true` when this call created it |
| `reason` | string | when `outcome` is not `allowed` | The denial/error message. A database error's own text is never included here -- it's replaced with a fixed `"internal database error"` string, since a raw `sqlx::Error` can carry SQL/connection detail |

## Notes

- **A denial at the gate carries no target fields.** `create_grant`/`delete_grant` and the four
  group mutation routes are gated by an extractor that runs before the request body/path
  parses, so a knob-off or non-admin denial there has no `audience`/`selector`/`group`/`member`
  to report -- only `actor`, `action`, `outcome`, and `client_ip`. The generic per-request
  observability log line records the method and URI at the same instant, so the two can be
  correlated by timestamp if the full request shape is needed.
- **Some denied attempts emit no record at all.** A self-service mint request rejected by the
  `MICROMEGAS_SELF_SERVICE_MINT` gate, and any grant/group request rejected by body or query
  deserialization (malformed JSON, an unknown field, a missing query parameter) before it reaches
  a gate or handler, produce no `control_plane_audit` record. The generic per-request log line is
  the only correlation point for that traffic.
- **This is a durability-agnostic write-adjacent log, not a transactional record.** It is emitted
  from the same request that performs the write, but the log emission itself and the database
  write are two independent fire-and-forget operations -- an emitted `allowed` record is strong
  evidence the write succeeded, not a guarantee.
