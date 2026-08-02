---
name: micromegas-query
description: Query micromegas observability data (logs, metrics, spans) using SQL. Use when the user wants to explore telemetry data, investigate errors, check performance, or analyze system behavior.
argument-hint: "<SQL query or natural language question about observability data>"
context: fork
shell: bash
allowed-tools: Bash(pip install micromegas), Bash(pip install --upgrade micromegas), Bash(micromegas-query *), Bash(python3 -c "import os; from micromegas.cli.config import resolve_connection; c = resolve_connection(); print(c.uri); print(c.oidc_issuer); print(c.oidc_client_id); print(os.path.exists(c.token_file))"), Bash(python -c "import os; from micromegas.cli.config import resolve_connection; c = resolve_connection(); print(c.uri); print(c.oidc_issuer); print(c.oidc_client_id); print(os.path.exists(c.token_file))"), Bash(py -3 -c "import os; from micromegas.cli.config import resolve_connection; c = resolve_connection(); print(c.uri); print(c.oidc_issuer); print(c.oidc_client_id); print(os.path.exists(c.token_file))"), PowerShell(python3 -c "import os; from micromegas.cli.config import resolve_connection; c = resolve_connection(); print(c.uri); print(c.oidc_issuer); print(c.oidc_client_id); print(os.path.exists(c.token_file))"), PowerShell(python -c "import os; from micromegas.cli.config import resolve_connection; c = resolve_connection(); print(c.uri); print(c.oidc_issuer); print(c.oidc_client_id); print(os.path.exists(c.token_file))"), PowerShell(py -3 -c "import os; from micromegas.cli.config import resolve_connection; c = resolve_connection(); print(c.uri); print(c.oidc_issuer); print(c.oidc_client_id); print(os.path.exists(c.token_file))"), PowerShell(pip install micromegas), PowerShell(pip install --upgrade micromegas), PowerShell(micromegas-query *), Read, Write(~/.micromegas/config.json), Edit(~/.micromegas/config.json), Glob, Grep, WebFetch(https://micromegas.info/*), WebFetch(https://datafusion.apache.org/*)
---

The user's query or question: $ARGUMENTS

## MANDATORY RULES — read before writing ANY query

1. **Every query MUST have `--begin` (and optionally `--end`).** No exceptions — not for `SELECT DISTINCT`, not for process lookups, not for exploratory queries. Default to `--begin 1h` when in doubt; narrow further when possible. The **only** exception: `--all` may be used for lightweight aggregate-only queries (`COUNT(*)`, `COUNT(DISTINCT ...)`, `MIN/MAX`) that return a single row or a small fixed number of rows. Never use `--all` with queries that return raw rows or unbounded GROUP BY result sets.
2. **Every query MUST have a `LIMIT` clause** unless the user explicitly asks for all rows or the query is a pure aggregate (COUNT, SUM, etc.) guaranteed to return a small result set. Default to `LIMIT 20`. For GROUP BY queries, still add LIMIT (e.g. `LIMIT 50`).
3. **Use `--begin`/`--end` CLI flags for time filtering, never `WHERE time >= ...` in SQL.** The CLI flags enable server-side partition pruning, making queries much faster.
4. **Use `view_instance()` for single-process/stream queries.** Much better performance than filtering with WHERE.
5. **`thread_spans` has no global view** — must use `view_instance('thread_spans', stream_id)` or `process_spans(process_id, 'thread')`.
6. **`async_events` has no global view** — must use `view_instance('async_events', process_id)` or `process_spans(process_id, 'async')`.

Violating these rules can crash the analytics service by exhausting its memory.

## Setup

Before running the user's actual query, verify the environment with two cheap, no-network checks:
a CLI-presence check, and a Python config probe. Run both — they test different things, since
`micromegas-query` (a console script, typically on `PATH` via pipx/poetry/venv) and the `micromegas`
module importable by the ambient `python3`/`python`/`py -3` interpreter can each be present without
the other.

First, check the CLI is on `PATH`:
```
micromegas-query --help
```

Second, use an ordinary shell check (use `Bash` on Linux/macOS, or on Windows without Git Bash,
`PowerShell`) for the config probe. Try `python3` first; if that command is not found, fall back to
`python`, then `py -3`:

```
python3 -c "import os; from micromegas.cli.config import resolve_connection; c = resolve_connection(); print(c.uri); print(c.oidc_issuer); print(c.oidc_client_id); print(os.path.exists(c.token_file))"
```

This call is total — it never fails just because nothing is configured yet, since a missing
config resolves to the default `grpc://localhost:50051` — and it never triggers a browser login
by itself (see the Interactive SSO note below). Read the two checks together:

- **CLI missing but module importable** (or vice versa) — this is a `PATH`/environment mismatch
  between the interpreter running the probe and the one `micromegas-query` was installed for, not
  "not installed". Don't reinstall; instead locate the right interpreter/environment (e.g. the one
  that owns the `pip`/`pipx`/`poetry` install) or ask the user which environment to use.
- **`ModuleNotFoundError: No module named 'micromegas'`, and the CLI is also missing** — the
  package isn't installed. Run:
  ```
  pip install micromegas
  ```
- **Any other import error naming something inside `micromegas`** (e.g.
  `No module named 'micromegas.cli.config'`) — an older `micromegas` version is already installed
  that predates this config module. `pip install micromegas` alone is a no-op in this case; run:
  ```
  pip install --upgrade micromegas
  ```
- **Prints four lines with no error** — the package is present and importable. Read the four
  printed values (`uri`, `oidc_issuer`, `oidc_client_id`, token-file-exists) as follows:
  - A printed `uri` of `grpc://localhost:50051` is ambiguous — it may be a deliberate local
    deployment, or nothing may be configured yet. Don't treat it as reason to overwrite an
    existing config on its own; ask the user first if you're unsure whether it's configured.
  - If `oidc_issuer` and `oidc_client_id` are both `None`, no OIDC is configured — no further
    setup is implied for auth; either the URI is enough (plain connection) or the user still needs
    to provide connection details (see below).

If the user hasn't told you where to connect, ask them to provide:
- Their analytics service URI (e.g. `https://analytics.example.com:443`)
- If their deployment uses OIDC authentication: the issuer URL, client ID, and audience

(Scope is not part of this flow — see the OIDC scope note below.)

Once the user provides the values, read `~/.micromegas/config.json` first (it may already have
content to preserve) and merge in the new values rather than clobbering the file, writing:
```json
{
  "uri": "<value from user>",
  "client_id": "<value from user, only if OIDC>",
  "issuers": [{ "issuer": "<value from user>", "audience": "<value from user>" }]
}
```
This takes effect immediately for every subsequent `micromegas-query` call in this session or any
future one — no shell profile edit or new terminal required, since the library reads this file on
every invocation.

**OIDC scope**: there's no `config.json` field for `oidc_scope` — it's only read from the
`MICROMEGAS_OIDC_SCOPE` environment variable, with no config-file fallback. The library already
defaults it to `"openid email profile offline_access"` when unset, which covers the common case.
If the user needs a non-default scope (e.g. Azure's `api://{client_id}/.default`), tell them to
set `MICROMEGAS_OIDC_SCOPE` themselves in their own shell profile — that's outside this skill's
automated config flow.

**Interactive SSO**: the setup probe above makes no network calls and can never trigger a login.
Only the *first* real `micromegas-query` call can, and only when `oidc_issuer` and
`oidc_client_id` are both configured — `connect()` takes the OIDC path only when both are truthy.
When that's the case, check the probe's token-file-exists output:
- If it's `False`, no cached token exists yet, so that first `micromegas-query` call may open a
  browser for login and block waiting for the user. Ask the *user* to run that first call
  themselves, in their own session, rather than attempting it yourself and hanging.
- If it's `True`, a cached token already exists and `load_or_login()` will try it before ever
  considering a browser — go ahead and run the query yourself. However, if that token has expired
  or been revoked, `load_or_login()` silently falls back to the same blocking browser login (look
  for `Token refresh failed` / `Re-authenticating...` in the query's output). If you see that
  output, or the call blocks/times out, stop — don't wait it out — and ask the *user* to run the
  query themselves in their own session (optionally after `micromegas-logout` to clear the stale
  token first).

**Connection errors**: a connection error targeting `127.0.0.1:50051` (the default) means the URI
never resolved to the intended service, not that authentication failed — this matters because it
can appear interleaved with token-refresh output that suggests the wrong cause. Fix the `uri`
value in `~/.micromegas/config.json` (or the `MICROMEGAS_ANALYTICS_URI` env var) rather than
troubleshooting auth.

Verify setup: if the probe reported no OIDC configured (`oidc_issuer`/`oidc_client_id` both
`None`), or a cached token already exists (token-file-exists is `True`), run:
```
micromegas-query "SELECT 1" --begin 1h
```
Otherwise (OIDC is configured and no token exists yet), this is the same first-call case covered
by the Interactive SSO note above — ask the *user* to run that verification query themselves.

## CLI syntax

```
micromegas-query [--file <path|->] ["<SQL>"] --begin <time> [--end <time>] [--format table|csv|json] [--max-colwidth N]
```

- `--begin` is **always required** except when using `--all` for lightweight aggregate-only queries (see MANDATORY RULES).
- `--file` accepts a `.sql` file path or `-` for stdin, as an alternative to the inline SQL positional argument. Using `--file` avoids awkward shell quoting when queries contain JSONPath expressions (e.g., `$[*].attributes[*]?(@.key=="Branch").value`).
- Time formats: relative (`1h`, `30m`, `7d`) or RFC 3339
- Use `--begin`/`--end` for time filtering, never `WHERE time >= ...` in SQL (see MANDATORY RULES).
- Use `--format csv` or `--format json` when processing results programmatically (csv for flat data, json when properties/nested fields matter)
- `--max-colwidth 0` for unlimited column width

## Available views

Views are listed below. **Do not run DESCRIBE queries** — all schemas are documented here.

**Ignore internal tables:** Tables prefixed with `__` and suffixed with `__partitions` (e.g. `__log_entries__partitions`) are internal partition tables — never query them directly.

**Log levels:** 1=Fatal, 2=Error, 3=Warn, 4=Info, 5=Debug, 6=Trace

**Type notes:** Types below are simplified for readability. `Dictionary(Utf8)` columns are dictionary-encoded (`Dictionary(Int32, Utf8)`) and behave like `Utf8` in SQL. `properties`/`process_properties` shown as `Binary` are actually `Dictionary(Int32, Binary)` — always read them via `property_get()` / the JSONB functions rather than treating them as raw bytes.

### `processes` — process metadata (global)

| Column | Type |
|--------|------|
| process_id | Utf8 |
| exe | Utf8 |
| username | Utf8 |
| realname | Utf8 |
| computer | Utf8 |
| distro | Utf8 |
| cpu_brand | Utf8 |
| tsc_frequency | Int64 |
| start_time | Timestamp(ns, UTC) |
| start_ticks | Int64 |
| insert_time | Timestamp(ns, UTC) |
| parent_process_id | Utf8 |
| properties | Binary (use `property_get()`) |
| last_update_time | Timestamp(ns, UTC) |
| last_block_end_ticks | Int64 |
| last_block_end_time | Timestamp(ns, UTC) |

### `streams` — stream metadata (global)

| Column | Type |
|--------|------|
| stream_id | Utf8 |
| process_id | Utf8 |
| dependencies_metadata | Binary |
| objects_metadata | Binary |
| tags | List(Utf8) |
| properties | Binary (use `property_get()`) |
| insert_time | Timestamp(ns, UTC) |
| format | Utf8 |
| last_update_time | Timestamp(ns, UTC) |

### `blocks` — block metadata with joined stream/process info (global)

Common columns below. The view also exposes the full set of joined `streams.*` and `processes.*` fields (e.g. `streams.dependencies_metadata`, `streams.insert_time`, `streams.format`, `processes.start_time`, `processes.tsc_frequency`, `processes.realname`, `processes.distro`, `processes.cpu_brand`, `processes.parent_process_id`, …).

| Column | Type |
|--------|------|
| block_id | Utf8 |
| stream_id | Utf8 |
| process_id | Utf8 |
| begin_time | Timestamp(ns, UTC) |
| begin_ticks | Int64 |
| end_time | Timestamp(ns, UTC) |
| end_ticks | Int64 |
| nb_objects | Int32 |
| object_offset | Int64 |
| payload_size | Int64 |
| insert_time | Timestamp(ns, UTC) |
| streams.tags | List(Utf8) |
| streams.properties | Binary (use `property_get()`) |
| processes.exe | Utf8 |
| processes.username | Utf8 |
| processes.computer | Utf8 |
| processes.properties | Binary (use `property_get()`) |

### `log_entries` — log messages (global + per-process via view_instance)

| Column | Type |
|--------|------|
| process_id | Dictionary(Utf8) |
| stream_id | Dictionary(Utf8) |
| block_id | Dictionary(Utf8) |
| insert_time | Timestamp(ns, UTC) |
| exe | Dictionary(Utf8) |
| username | Dictionary(Utf8) |
| computer | Dictionary(Utf8) |
| time | Timestamp(ns, UTC) |
| target | Dictionary(Utf8) |
| level | Int32 |
| msg | Utf8 |
| properties | Binary (use `property_get()`) |
| process_properties | Binary (use `property_get()`) |

### `log_stats` — pre-aggregated log counts by minute (global)

| Column | Type |
|--------|------|
| time_bin | Timestamp(ns, UTC) |
| process_id | Dictionary(Utf8) |
| level | Int32 |
| target | Dictionary(Utf8) |
| count | Int64 |

### `measures` — numeric metrics (global + per-process via view_instance)

High-frequency numeric metrics. Use `view_instance('measures', process_id)` for single-process queries. Keep `--begin` tight — a few hours at most.

| Column | Type |
|--------|------|
| process_id | Dictionary(Utf8) |
| stream_id | Dictionary(Utf8) |
| block_id | Dictionary(Utf8) |
| insert_time | Timestamp(ns, UTC) |
| exe | Dictionary(Utf8) |
| username | Dictionary(Utf8) |
| computer | Dictionary(Utf8) |
| time | Timestamp(ns, UTC) |
| target | Dictionary(Utf8) |
| name | Dictionary(Utf8) |
| unit | Dictionary(Utf8) |
| value | Float64 |
| properties | Binary (use `property_get()`) |
| process_properties | Binary (use `property_get()`) |

### `thread_spans` — sync spans (NO global view, view_instance only)

**No global view — must use `view_instance('thread_spans', stream_id)` or `process_spans(process_id, 'thread')`.** Always use tight `--begin`/`--end` and `LIMIT`.

| Column | Type |
|--------|------|
| id | Int64 |
| parent | Int64 |
| depth | UInt32 |
| hash | UInt32 |
| begin | Timestamp(ns, UTC) |
| end | Timestamp(ns, UTC) |
| duration | Int64 (nanoseconds) |
| name | Dictionary(Utf8) |
| target | Dictionary(Utf8) |
| filename | Dictionary(Utf8) |
| line | UInt32 |

### `async_events` — async span events (NO global view, view_instance only)

**No global view — access via `view_instance('async_events', process_id)` or `process_spans(process_id, 'async')`.**

| Column | Type |
|--------|------|
| stream_id | Dictionary(Utf8) |
| block_id | Dictionary(Utf8) |
| time | Timestamp(ns, UTC) |
| event_type | Dictionary(Utf8) |
| span_id | Int64 |
| parent_span_id | Int64 |
| depth | UInt32 |
| hash | UInt32 |
| name | Dictionary(Utf8) |
| filename | Dictionary(Utf8) |
| target | Dictionary(Utf8) |
| line | UInt32 |

## Key functions

### Table functions
- `view_instance(view_name, id)` — scoped queries for a single process or stream, better performance
- `process_spans(process_id, 'thread'|'async'|'both')` — all spans for a process with `stream_id` and `thread_name` columns. **Extremely dense** — always use tight `--begin`/`--end` and `LIMIT`.
- `list_partitions()` — list available data partitions with metadata
- `expand_histogram(h)` — expands a histogram into rows of `(bin_center, count)` for visualization
- `jsonb_each(jsonb)` — expand JSONB object/array into rows of `(key, value)`. `value` is raw JSONB (Binary) — wrap it with `jsonb_as_string()`/`jsonb_as_f64()`/`jsonb_format_json()` to read it.

### Property functions
- `property_get(properties, key)` — extract property values
- `properties_to_jsonb(properties)` — convert properties to JSONB format

### JSONB functions
- `jsonb_get(jsonb, key)` — extract value by key
- `jsonb_as_string(jsonb)` — cast JSONB to string
- `jsonb_as_f64(jsonb)` — cast JSONB to float
- `jsonb_as_i64(jsonb)` — cast JSONB to integer
- `jsonb_format_json(jsonb)` — JSONB to readable JSON string
- `jsonb_parse(string)` — parse JSON string to JSONB
- `jsonb_object_keys(jsonb)` — list keys of a JSONB object
- `jsonb_path_query_first(jsonb, path)` — first JSONPath match
- `jsonb_path_query(jsonb, path)` — all JSONPath matches as array

### Histogram functions
- `make_histogram(start, end, bins, values)` — aggregate: creates a histogram from numeric values
- `sum_histograms(h)` — aggregate: combines multiple histograms by summing bins
- `quantile_from_histogram(h, quantile)` — estimate percentile (0.0–1.0, e.g. 0.5=median, 0.95=p95)
- `variance_from_histogram(h)` — distribution variance
- `count_from_histogram(h)` — total number of values
- `sum_from_histogram(h)` — sum of all values

### Discovering UDFs
For micromegas extension functions, use the **Key functions** section above or the
[functions reference](https://micromegas.info/docs/query-guide/functions-reference/) rather than
querying for them — `information_schema.routines` lists them, but its `description` and
`syntax_example` columns are only populated for DataFusion built-ins, since DataFusion derives
them from `documentation()`, which no micromegas UDF implements. For DataFusion built-in scalar or
aggregate functions, use the
[DataFusion scalar](https://datafusion.apache.org/user-guide/sql/scalar_functions.html) /
[aggregate](https://datafusion.apache.org/user-guide/sql/aggregate_functions.html) function docs
(see **Reference documentation** below).

## Common query patterns

### Recent errors
```
micromegas-query "SELECT time, target, msg FROM log_entries WHERE level <= 2 ORDER BY time DESC LIMIT 20" --begin 1h
```

### Error trends via log_stats
```
micromegas-query "SELECT date_trunc('hour', time_bin) as hour, SUM(count) as total FROM log_stats WHERE level <= 2 GROUP BY 1 ORDER BY 1" --begin 1d
```

### Process discovery
```
micromegas-query "SELECT process_id, exe, computer, start_time FROM processes ORDER BY start_time DESC LIMIT 20" --begin 1d
```

### Logs for a specific process
```
micromegas-query "SELECT time, level, target, msg FROM view_instance('log_entries', '<process_id>') ORDER BY time DESC LIMIT 30" --begin '<start_time>' --end '<end_time>'
```

### Log volume by target
```
micromegas-query "SELECT target, SUM(count) as total FROM log_stats GROUP BY target ORDER BY total DESC LIMIT 20" --begin 1d
```

### Metric exploration
```
micromegas-query "SELECT DISTINCT name, unit FROM measures LIMIT 50" --begin 1h
```

### Metric values for a process
```
micromegas-query "SELECT time, name, value FROM view_instance('measures', '<process_id>') WHERE name = '<metric_name>' ORDER BY time LIMIT 50" --begin '<start_time>' --end '<end_time>'
```

### Span analysis with process_spans
```
micromegas-query "SELECT name, AVG(duration)/1e6 as avg_ms, COUNT(*) FROM process_spans('<process_id>', 'thread') GROUP BY name ORDER BY avg_ms DESC LIMIT 20" --begin '<start_time>' --end '<end_time>'
```

### Histogram percentiles
```
micromegas-query "SELECT name,
       quantile_from_histogram(make_histogram(0, 100000000, 50, duration), 0.5)/1e6 as p50_ms,
       quantile_from_histogram(make_histogram(0, 100000000, 50, duration), 0.95)/1e6 as p95_ms,
       quantile_from_histogram(make_histogram(0, 100000000, 50, duration), 0.99)/1e6 as p99_ms,
       COUNT(*)
FROM process_spans('<process_id>', 'thread')
GROUP BY name ORDER BY p95_ms DESC LIMIT 20" --begin '<start_time>' --end '<end_time>'
```

### Expand histogram for visualization
```
micromegas-query "SELECT bin_center/1e6 as ms, count
FROM expand_histogram((SELECT make_histogram(0, 100000000, 50, duration) FROM process_spans('<process_id>', 'thread') WHERE name = '<span_name>'))" --begin '<start_time>' --end '<end_time>'
```

### Property extraction
```
micromegas-query "SELECT time, msg, property_get(process_properties, 'thread-name') as thread FROM log_entries WHERE level <= 3 LIMIT 20" --begin 1h
```

### Log properties as JSON
```
micromegas-query "SELECT time, msg, jsonb_format_json(properties) as props FROM log_entries WHERE level <= 2 LIMIT 10" --begin 1h
```

## Performance tips

- See **MANDATORY RULES** above — `--begin` and `LIMIT` are required on every query.
- **Keep the time range as tight as possible** — match `--begin` (and `--end`) to what the question actually requires. Default to `--begin 1h` for recent data; use `--begin 1d` for daily analysis; only widen if the user asks for a longer window or the initial query returns nothing useful.
- Use `view_instance()` for single-process/stream queries — much faster than global views with WHERE filters.
- Use `log_stats` for volume analysis instead of counting `log_entries`.
- Select only the columns you need — avoid `SELECT *` on large views.
- Filter on time first, then other fields.
- **Queries without `--begin` scan ALL historical data** and can exhaust server memory, crashing the analytics service for all users.

## Workflow: narrowing time ranges

**Never use a broad time window when querying data for a specific process.** Always resolve the process's actual lifetime first, then use that to set precise `--begin`/`--end` flags.

1. **Get the process lifetime** from the `processes` table:
   ```
   micromegas-query "SELECT process_id, start_time, last_update_time FROM processes WHERE process_id = '<pid>' LIMIT 1" --begin 1d
   ```

2. **Convert the timestamps** to RFC 3339 and use them as `--begin`/`--end`:
   ```
   micromegas-query "SELECT time, level, target, msg FROM view_instance('log_entries', '<pid>') ORDER BY time DESC LIMIT 30" \
     --begin '<start_time>' --end '<last_update_time>' --format table --max-colwidth 150
   ```

**Why this matters:** The `--begin`/`--end` flags drive server-side partition pruning. A query with `--begin 7d` on a single process will scan 7 days of partitions just to find a few seconds of data. A precise window hits only the relevant partition(s), making the query orders of magnitude faster.

## Reference documentation

- Micromegas docs: https://micromegas.info/docs/
- Micromegas extensions: https://micromegas.info/docs/query-guide/functions-reference/
- SQL syntax: https://datafusion.apache.org/user-guide/sql/
- Scalar functions: https://datafusion.apache.org/user-guide/sql/scalar_functions.html
- Aggregate functions: https://datafusion.apache.org/user-guide/sql/aggregate_functions.html
