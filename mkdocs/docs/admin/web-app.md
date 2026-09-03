# Analytics Web App Deployment

> **TLDR:** The web app is a dev/demo tool. For production use, query via FlightSQL directly.

## Quick Start (Dev)

```bash
cd analytics-web-app
python start_analytics_web.py
```

Opens on `http://localhost:3000` with backend on `:8000`. Automatically sets localhost defaults.

## Environment Variables

### Required

```bash
# OIDC provider configuration (same format as FlightSQL server)
export MICROMEGAS_OIDC_CONFIG='{
  "issuers": [
    {
      "issuer": "https://accounts.google.com",
      "audience": "your-client-id.apps.googleusercontent.com"
    }
  ]
}'

# CORS and OAuth callback
export MICROMEGAS_WEB_CORS_ORIGIN="http://localhost:3000"
export MICROMEGAS_AUTH_REDIRECT_URI="http://localhost:3000/auth/callback"

# OAuth state signing secret (IMPORTANT: must be same across all instances)
# Generate with: openssl rand -base64 32
export MICROMEGAS_STATE_SECRET="your-random-secret-here"

# URL prefix (use "/" for root, "/analytics" for a sub-path)
export MICROMEGAS_BASE_PATH="/"

# PostgreSQL database for the web app (screens, data sources, maps catalog)
export MICROMEGAS_APP_SQL_CONNECTION_STRING="postgres://user:pass@localhost/analytics_web"

# Key management (Admin -> Analytics API Keys / Ingestion API Keys / Groups);
# audience grants are managed from the Audience Access page (/audiences,
# open to every authenticated user -- see "Audience Access" below), the
# micromegas-grants CLI, or list_audience_grants() from any SQL client.
# REQUIRED whenever auth is enabled -- `analytics-web-srv` bails at startup
# naming this var if it's unset, since without it no session can resolve
# admin-ness or group grants at all: admin-ness itself now comes from the
# `admins` local group (see mkdocs/docs/admin/groups.md), resolved from this
# same connection. Only reachable as unset under `--disable-auth`, where
# these routes return a fixed 503 (`AUTH_DISABLED`) regardless. Backs ALL
# FOUR route groups -- analytics-web-srv is the sole admin HTTP surface for
# ingestion_api_keys, analytics_api_keys, audience_grants, and
# groups/group_members, writing directly to Postgres for each (see
# mkdocs/docs/admin/api-keys.md). Must point at a telemetry DB where the v10
# migration has already run (via ingestion or a lakehouse-role monolith) --
# v10 is required, not just v7, since the group store needs the
# groups/group_members tables added at v10 -- or every session fails with a
# retryable 503 until the migration runs.
export MICROMEGAS_SQL_CONNECTION_STRING="postgres://user:pass@localhost/telemetry"
```

### Optional

```bash
# Cookie settings (production)
export MICROMEGAS_COOKIE_DOMAIN=".example.com"
export MICROMEGAS_SECURE_COOKIES="true"  # HTTPS only

# Map assets (object store URI; see "Maps" below)
export MICROMEGAS_MAPS_OBJECT_STORE_URI="s3://my-bucket/maps/"
export MICROMEGAS_MAPS_MAX_UPLOAD_BYTES="268435456"  # 256 MiB default

# The deployment's default audience: what a mint/import request that supplies
# no `audience` gets, and what the ingestion edge stamps onto a credential
# with no bound audience of its own (see
# mkdocs/docs/admin/authorization.md#audience-stamping).
# Defaults to `public` when unset -- see
# mkdocs/docs/admin/api-keys.md#what-audience-does-a-key-carry.
export MICROMEGAS_DEFAULT_AUDIENCE="public"

# Self-service ingestion key mint -- off by default, admin-only mint until
# explicitly enabled. Lets a non-admin caller with a matching `mint` grant
# (or lazily claiming a brand-new audience) mint their own key, and gates
# GET .../audience-grants/my-audiences the same way -- see
# mkdocs/docs/admin/authorization.md#self-service-ingestion-key-mint.
export MICROMEGAS_SELF_SERVICE_MINT="false"

# Per-caller bounds once MICROMEGAS_SELF_SERVICE_MINT is on -- backstops
# against a runaway/abusive caller, not routine-use quotas.
export MICROMEGAS_SELF_SERVICE_MAX_CLAIMS_PER_CALLER="25"
export MICROMEGAS_SELF_SERVICE_MAX_KEYS_PER_CALLER="100"

# Disable auth (dev only) -- also disables all three key/grant-management
# route groups above (a fixed 503 answers them instead), not just cookie
# auth on the rest of the API.
analytics-web-srv --disable-auth
```

## Admin hub

**Admin** (`/admin`) is open to **every authenticated user**, not just admins — `AuthGuard` on
this route carries no `requireAdmin`. It renders a role-filtered card grid: an admin sees all
nine cards; a non-admin sees only the two that have a real non-admin capability —
**Ingestion API Keys** (mint only — see [Web app admin pages](api-keys.md#web-app-admin-pages))
and **Audience Access** (see [Audience Access](#audience-access)). The other seven — Data Sources,
Export Screens, Import Screens, Maps, Analytics API Keys, Query Deny List, Groups — have no
non-admin capability at all and stay hidden from a non-admin, same as their pages stay gated by
`<AuthGuard requireAdmin>`.

While the `admins` local group still holds a wildcard (`*`) member (see
[Groups](groups.md)), the hub renders an unmissable warning banner — fetched only when
`user.is_admin`, which under the wildcard is everyone, so the warning reaches whoever it applies
to.

The sidebar's bottom-left "Admin" icon is shown to every authenticated user for the same reason;
following it to `/admin` never dead-ends a non-admin who has a legitimate reason to be there (an
old bookmark, a link an admin pasted, or the natural place to look for "Ingestion API Keys").

## Groups

**Admin → Groups** (`/admin/groups`) manages local group membership, including the reserved
`admins` group — see [Groups](groups.md) for the full model, the routes, and the CLI. The page
shows every group with its member count; selecting one shows its members as chips (`*`
highlighted, `group:` chips linking to the nested group) with a remove control and an **Add
member** dialog whose kind toggle (everyone / user / group) mirrors the Audience Access page's
own grant dialog.

## Maps

Map cells render GLB assets fetched from a server-side object store. Set `MICROMEGAS_MAPS_OBJECT_STORE_URI` to a prefix the web-app process can read **and write** — admins upload and delete GLBs through **Admin → Maps**, which calls `PUT`/`DELETE` on `/api/maps/blob/{filename}`. If the variable is unset, the maps endpoints return 503 and the dropdown is empty.

**IAM / credentials.** The process credentials need the equivalent of `s3:GetObject`, `s3:PutObject`, `s3:DeleteObject`, and `s3:ListBucket` (or GCS / local-fs equivalents) scoped to the configured prefix. Read-only credentials are sufficient only if you keep populating maps out-of-band (`aws s3 cp ...`) and don't expose the admin page.

**Upload cap.** `MICROMEGAS_MAPS_MAX_UPLOAD_BYTES` bounds the per-request body for uploads (default 256 MiB). The cap is enforced before the body is buffered. The handler gzips on upload and the read path serves bytes verbatim with `Content-Encoding: gzip`. Bare uploads (no metadata) are served uncompressed.

**Catalog discovery.** The catalog is derived at request time by listing every `*.glb` object directly under the configured prefix — there is no `maps.json` to keep in sync. Display names are derived client-side: the `.glb` extension is stripped (`Arena_North.glb` → `Arena_North`).

**Out-of-band uploads.** Ops can drop `.glb` files directly into the configured prefix (no UI required), but the upload must set `Content-Type: model/gltf-binary` and `Content-Encoding: gzip` if pre-compressed. The Admin → Maps page is the recommended path because it handles the compression and metadata in one shot.

**GLB authoring contract.** The renderer expects each GLB to satisfy these invariants — there is no fallback path:

- **Z-up** — the only hard axis constraint; the renderer pins `scene.up = (0, 0, 1)` and has no axis-conversion step. Handedness and units must simply match between the GLB and the event data (no specific units required, no auto-centering, no scaling).
- **Exactly one perspective camera** referenced from `scenes[0]`, used to seed the initial camera (position, orientation, fov, near, far). Camera roll is dropped when seeding the orbit controller.
- **`KHR_lights_punctual`** — directional lights live inside the scene tree and render automatically.
- **`MM_ambient_light`** vendor extension — `{ color: [r, g, b], intensity: number }` at the root extensions; the renderer reads it directly from `gltf.parser.json.extensions`.

GLBs missing the camera log a console error and fall back to the default seed framing (likely mis-framed); GLBs missing `MM_ambient_light` log a console error and render without ambient illumination. These are visible failure modes that signal a non-conforming GLB.

> Because the contract uses Z-up, the GLBs are technically out of spec for glTF 2.0 (which mandates Y-up). External viewers — Blender, online glTF validators, Windows 3D Viewer — will render them rotated. The micromegas web-app is the only intended consumer.

**URI grammar.** Same shape as `MICROMEGAS_OBJECT_STORE_URI` (passed through `object_store::parse_url_opts`):

| Backend | Example |
|---|---|
| Local dev | `file:///home/you/lake/maps/` |
| AWS prod | `s3://my-bucket/maps/` |
| GCS | `gs://my-bucket/maps/` |

`start_analytics_web.py` defaults this to `<MICROMEGAS_OBJECT_STORE_URI>/maps/` — i.e. a `maps/` sibling of the telemetry lake — so a single lake root supplies both telemetry blobs and map assets for local dev.

## Query Deny List

**Admin → Query Deny List** (`/admin/query-deny-list`) is a front end for the three SQL functions
described in [Admin Functions → Query Deny List](functions-reference.md#query-deny-list) —
`list_query_denials()`, `deny_queries(match_expr, reason)`, `remove_query_denial(rule_id)` —
issued through the same `useStreamQuery` → `POST /api/query-stream` path every other SQL-driven
page in this app uses, against whichever data source the admin has selected. There is no
dedicated REST route and no second copy of the rule store: the screen manages the deny list of
the deployment it's pointed at.

The screen shows a table of rules (match expression, reason, creator, created time, and a
relative "last hit" — "4 s ago" reads as "still firing", "3 weeks ago" as "probably safe to
remove") plus a **Deny a Query** dialog: an expression textarea, insert-chips for the common
predicates, a link to the match-context reference, and a required reason field. A compile error
from the server (e.g. a non-boolean expression, or one with no column reference) is shown inline
against the expression, not as a page-level banner.

Like every admin route, this one is gated by `AuthGuard requireAdmin` client-side and by
`flight-sql-srv`'s own admin check server-side — the client-side guard is UX only; a non-admin
who hand-typed the SQL would get "function not found" regardless.

**One interaction with the existing web-query guard.** `analytics-web-srv`'s `BLOCKED_FUNCTIONS`
substring check (`stream_query.rs`) rejects any web query merely *mentioning* `retire_partitions`,
`retire_partition_by_metadata`, or `retire_partition_by_file` — including inside a deny
expression's own string literal, e.g. `deny_queries('sql LIKE ''%retire_partitions%''', '...')`.
That guard is deliberately left as-is (narrowing it to call position would open a comment-based
bypass), so a deny rule that needs to mention one of those names has to be created from
`micromegas-query` or a notebook instead of this screen.

## Audience Access

**Audience Access** (`/audiences`) is open to **every authenticated user**, not just admins —
`AuthGuard` on this route carries no `requireAdmin`. It answers "what can I read, and why?",
lets a user share what they can already see with other users and groups, remove their own
access, revoke a share they created, and mint an ingestion key into an audience they may mint
into. Admins see the whole store and keep every power they have today (any selector including
`*`; delete any row); a non-admin sees a scoped, fewer-controls version of the same page.

Reachable from a new **Audience access** item in the header user menu (everyone) and from an
**Audience Access** card on the [Admin hub](#admin-hub) page (also every authenticated user).

**Reads go through SQL for auditing, REST for the page itself.** The page's own list calls the
small, unpaginated `GET {base_path}/api/audience-grants/visible` route (below) against this
deployment's own store — not `list_audience_grants()` and a data source, since this page's
writes are fixed to one store and its read has to be too. The caller-scoped
`list_audience_grants()` SQL table function (see
[Admin Functions Reference](functions-reference.md#list_audience_grants)) is how
`micromegas-query` and other SQL clients audit the store ad hoc; it is registered for every
authenticated caller, never admin-gated, and applies the same held-pair visibility rule
`/visible` does for a non-admin — except that it cannot see (and so cannot apply)
`analytics-web-srv`'s self-service knob the way `/visible` does, and so is always as wide open
for a non-admin as the knob-on case. See
[Self-service ingestion key mint](authorization.md#self-service-ingestion-key-mint)
for the knob and [the grant store](authorization.md#the-grant-store)
for the write policy the page's Share/Remove/Revoke controls drive.

**Unavailable under `--disable-auth`.** The page's `/visible` and `/my-audiences` reads (and
every write) 503 with `AUTH_DISABLED` in that mode, same as the admin key-management pages (see
[`api-keys.md`](api-keys.md#web-app-admin-pages)); the page detects this and renders a single
explanatory panel instead of the normal list, with no Add grant / Share / Mint / delete controls.

**What a non-admin sees and may do**, once `MICROMEGAS_SELF_SERVICE_MINT` is on:

- **See** every grant on each `(audience, axis)` pair they hold a matching grant on — including
  other principals' rows on that same pair, which is what lets them answer "who else can see
  this."
- **Share** a pair they hold, with a `user:<id>` or `group:<id>` selector only — never `*`, and
  never a pair held only through a `*` row (the page never offers a control that would always
  fail server-side).
- **Remove their own** direct `user:<their email>` row, or **revoke** any row they themselves
  created.
- **Mint** an ingestion key into an existing mintable audience, or claim a brand-new one — the
  same self-service mint the CLI (`micromegas-setup-telemetry`) already exposes, now with a
  browser dialog.

**`public` ships with seeded Read and Mint rows, visible on this page like any other grant.**
Schema v9 seeds `('public', 'read', '*')` and `('public', 'mint', '*')` — an admin sees both under
`public`'s Read and Mint columns, attributed to `default` rather than a colleague's identity;
`public` is not a special case with an empty Mint column, it is two ordinary rows. A non-admin
who holds no other grant on that `(audience, axis)` pair sees only the rows' *effect* — `public`
shows up in the audiences they can read and mint — because the held-pair rule strips `*` from the
caller's own bound selectors, not from the rows returned; a non-admin who does hold another grant
on the same pair (e.g. an admin-created `('public', 'read', 'user:<id>')` row) sees the seeded
row too, same as any other row on a pair they hold. To open a *custom* deployment default
(`MICROMEGAS_DEFAULT_AUDIENCE` set to
something other than `public`) the same way, an admin uses the **Add grant** dialog with Axis =
Mint and Selector = **Everyone** on that audience — the same recipe the seeded `public` row is an
instance of.

With the knob off, the page still renders — it shows only the caller's own rows and disables
Share/Remove/Revoke/Mint, with a note explaining why.

**`AuthGuard` is UX only, here as everywhere else.** The route itself carries no server-side
gate beyond ordinary authentication; every actual authorization decision (the held-pair
visibility rule, the per-pair hold check on create, the own-row/created-by check on delete, the
self-service knob) is enforced server-side, in SQL or in the REST handlers, regardless of what
the client renders.

## Production Notes

**CORS Origin must match OAuth redirect URI origin:**
```bash
MICROMEGAS_WEB_CORS_ORIGIN="https://analytics.example.com"
MICROMEGAS_AUTH_REDIRECT_URI="https://analytics.example.com/auth/callback"
```

**Deploying behind a reverse proxy with path prefix:**
```bash
# Example: ALB routes /analytics/* to the web app
MICROMEGAS_BASE_PATH="/analytics"
MICROMEGAS_WEB_CORS_ORIGIN="https://example.com"
MICROMEGAS_AUTH_REDIRECT_URI="https://example.com/analytics/auth/callback"
```

Routes become: `/analytics/api/health`, `/analytics/api/query-stream`, `/analytics/auth/*`, etc.
The same container image works for any base path - no rebuild needed.

## API Routes

Without `MICROMEGAS_BASE_PATH` (or with `"/"`):
- `GET /api/health` — Health check (public)
- `GET /api/ready` — Readiness check (public)
- `POST /api/query-stream` — Execute SQL query (streaming NDJSON)
- `GET /api/screen-types` — List screen types
- `GET /api/screen-types/{type_name}/default` — Default config for a screen type
- `GET /api/screens`, `POST /api/screens` — List / create screens
- `GET /api/screens/{name}`, `PUT`, `DELETE` — Get / update / delete screen
- `GET /api/data-sources`, `POST /api/data-sources` — List / create data sources
- `GET /api/data-sources/{name}`, `PUT`, `DELETE` — Get / update / delete data source
- `GET /api/maps/catalog` — List map assets
- `GET /api/maps/blob/{filename}`, `PUT`, `DELETE` — Fetch / upload / delete map GLB
- `GET`/`POST /api/analytics-api-keys`, `POST /api/analytics-api-keys/import`, `DELETE /api/analytics-api-keys/{key_id}` — List/mint/import/revoke analytics API keys (503 under `--disable-auth`, otherwise `MICROMEGAS_SQL_CONNECTION_STRING` is required at startup — see [API Keys](api-keys.md))
- `GET`/`POST /api/ingestion-api-keys`, `POST /api/ingestion-api-keys/import`, `DELETE /api/ingestion-api-keys/{key_id}` — List/mint/import/revoke ingestion API keys, written directly to Postgres (503 under `--disable-auth`, otherwise `MICROMEGAS_SQL_CONNECTION_STRING` is required at startup — see [API Keys](api-keys.md))
- `GET`/`POST /api/audience-grants`, `DELETE /api/audience-grants?audience=&axis=&selector=`, `GET /api/audience-grants/visible`, `GET /api/audience-grants/my-audiences` — Audience grant CRUD and the caller-scoped reads (see [Authorization](authorization.md#audiences-and-grants))
- `GET`/`POST /api/groups`, `DELETE /api/groups/{name}`, `GET`/`POST /api/groups/{name}/members`, `DELETE /api/groups/{name}/members?member=` — Group CRUD and membership management, admin-only (see [Groups](groups.md))
- `GET /auth/login` — Initiate OAuth login
- `GET /auth/callback` — OAuth callback
- `POST /auth/refresh` — Refresh tokens
- `POST /auth/logout` — Logout
- `GET /auth/me` — Current user info

With `MICROMEGAS_BASE_PATH="/analytics"`, all routes are prefixed (e.g., `/analytics/api/health`).

**Configure OAuth redirect in your identity provider:**
- Add the redirect URI to allowed callbacks
- For Google: Cloud Console → APIs & Services → Credentials

## Architecture

- **Frontend**: Vite/React SPA on port 3000 (dev) or served by backend (prod)
- **Backend**: Rust (`analytics-web-srv`) on port 3000 (default); the dev start script uses 8000 to avoid conflicting with the Vite dev server
- **Auth**: OIDC (ID tokens via httpOnly cookies)
- **Data**: FlightSQL queries to analytics service

Backend proxies FlightSQL with user's ID token. No direct data access.

## Command Line Options

```bash
analytics-web-srv [OPTIONS]

Options:
  -p, --port <PORT>              Server port [default: 3000]
      --frontend-dir <DIR>       Frontend build directory [default: ../analytics-web-app/dist]
      --disable-auth             Disable authentication (dev only)
  -h, --help                     Print help
```

Example:
```bash
analytics-web-srv --port 8000 --frontend-dir ./dist --disable-auth
```

**Warning:** `--disable-auth` removes authentication middleware. Do not use in production.
