# Base Path Routing Bug

## CRITICAL REQUIREMENT

**The same Docker image MUST work with ANY base path without rebuilding.**

- NO build-time base path configuration (`NEXT_PUBLIC_BASE_PATH`)
- Base path is set via `MICROMEGAS_BASE_PATH` environment variable at runtime
- Server must rewrite ALL asset paths dynamically

## Problem

The analytics-web-srv needs to serve the Next.js frontend under a configurable base path (e.g., `/micromegas`) at runtime. The same Docker image should work with any base path without rebuilding.

## Status: FIXED (2025-12-11)

Three issues were fixed:

1. **Backend**: Trailing slash normalization wasn't working because `Router::layer()` runs AFTER routing
2. **Frontend**: AuthGuard was redirecting to `/login` without the base path, causing infinite loops
3. **Backend**: SPA fallback was always serving `index.html` instead of route-specific HTML files

## Root Cause

Next.js static export bakes asset paths at build time (`/_next/...`). We need to:
1. Rewrite asset paths in index.html to include base path (`/micromegas/_next/...`)
2. Inject runtime config script (`window.__MICROMEGAS_CONFIG__`)
3. Serve index.html for SPA routes (client-side routing)

The `serve_index_with_config` handler does all this correctly, but it's not being called for all paths.

## The Routing Challenge

Axum's `nest()` and `nest_service()` have a quirk:
- `nest("/micromegas", router)` matches `/micromegas/*` but NOT `/micromegas` exactly
- `NormalizePathLayer` strips trailing slashes AFTER routing (as a layer)
- So `/micromegas/` doesn't get normalized before route matching

### What Works (tested locally)

- `/micromegas` (no trailing slash) → `serve_index_with_config` → config injected ✓
- `/micromegas/processes` → SPA fallback → `serve_index_with_config` → config injected ✓
- `/micromegas/_next/*` → static files served correctly ✓

### What Doesn't Work

- `/micromegas/` (with trailing slash) → returns empty response or wrong handler

## Attempted Solutions

### 1. nest_service with fallback
```rust
let spa_fallback = get(serve_index_with_config).with_state(index_state);
let serve_dir = ServeDir::new(&args.frontend_dir).fallback(spa_fallback);
let app = app.nest_service(&base_path, serve_dir);
```
**Problem**: `nest_service` doesn't match `/micromegas` exactly, only `/micromegas/*`

### 2. Explicit route + nest_service
```rust
let app = app
    .route(&base_path, get(serve_index_with_config))
    .with_state(index_state)
    .nest_service(&base_path, serve_dir);
```
**Problem**: Panics with "Invalid route: Insertion failed due to conflict"

### 3. Separate router merged before nest
```rust
let base_path_exact = Router::new()
    .route(&base_path, get(serve_index_with_config))
    .with_state(index_state);
let app = app.merge(base_path_exact).nest(&base_path, frontend_routes);
```
**Problem**: Same conflict panic

### 4. Global fallback for unmatched routes
```rust
let app = app.nest(&base_path, frontend);
let app = app.fallback(get(serve_index_with_config).with_state(...));
```
**Problem**: Too broad - catches everything, not just `/micromegas`

## Files Involved

- `rust/analytics-web-srv/src/main.rs` - Server routing logic
- `analytics-web-app/src/app/page.tsx` - Home page redirect (uses runtime config)
- `analytics-web-app/src/lib/config.ts` - Runtime config accessor
- `analytics-web-app/src/lib/auth.tsx` - Auth calls (uses runtime config for API base)

## Key Code: serve_index_with_config

```rust
async fn serve_index_with_config(
    State(state): State<IndexState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let index_path = format!("{}/index.html", state.frontend_dir);
    let html = tokio::fs::read_to_string(&index_path).await?;

    // Rewrite asset paths to include base path
    let html = html
        .replace("\"/_next/", &format!("\"{}/_next/", state.base_path))
        .replace("\"/icon.svg", &format!("\"{}/icon.svg", state.base_path));

    // Inject runtime config
    let config_script = format!(
        r#"<script>window.__MICROMEGAS_CONFIG__={{basePath:"{}"}}</script>"#,
        state.base_path
    );
    let modified_html = html.replace("</head>", &format!("{config_script}</head>"));

    Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], modified_html))
}
```

## Solution Implemented (FIXED)

**Key Insight**: Axum's `Router::layer()` runs AFTER routing, so any middleware added via `.layer()` cannot modify the request path before routing. This is why custom middleware or `NormalizePathLayer` via `.layer()` doesn't work.

See: https://github.com/tokio-rs/axum/discussions/2377

### The Fix: Wrap the Router with NormalizePathLayer

```rust
use tower::Layer;
use tower_http::normalize_path::NormalizePathLayer;

// Build router with explicit base path route + nest
let app = app.route(
    &base_path,
    get(serve_index_with_config).with_state(index_state),
);
let app = app.nest(&base_path, frontend);

// Add CORS layer to the router (this can use .layer())
let app = app.layer(cors_layer);

// IMPORTANT: NormalizePathLayer must WRAP the router, not be added via .layer()
let app = NormalizePathLayer::trim_trailing_slash().layer(app);

// Use ServiceExt for serving
axum::serve(
    listener,
    ServiceExt::<axum::extract::Request>::into_make_service_with_connect_info::<std::net::SocketAddr>(app),
).await?;
```

### Why this works
1. `NormalizePathLayer::trim_trailing_slash().layer(app)` wraps the entire router
2. This makes the normalization run BEFORE routing happens
3. `/micromegas/` becomes `/micromegas` before the router sees it
4. The explicit `.route(&base_path, ...)` handles the exact `/micromegas` path
5. The `.nest(&base_path, frontend)` handles all `/micromegas/*` paths
6. The nested router does NOT include "/" to avoid conflict with the explicit route

### Important Notes
- The nested frontend router must NOT have a `route("/", ...)` because that would conflict with the explicit base path route
- The `tower` crate must be added to dependencies for the `Layer` trait
- Use `ServiceExt::into_make_service_with_connect_info` for serving the wrapped router

### Tests Added
See `rust/analytics-web-srv/tests/routing_tests.rs` for comprehensive tests covering:
- Exact base path match (`/micromegas`)
- Trailing slash normalization (`/micromegas/`)
- Nested paths (`/micromegas/index.html`, `/micromegas/_next/*`)
- SPA fallback for client-side routes
- Query string preservation (`/micromegas/?foo=bar`)

## Route-Specific HTML File Fix (Dec 11, 2025)

The SPA fallback was always serving `index.html` regardless of the requested route. But Next.js static export creates route-specific HTML files:
- `login.html` - pre-rendered login page
- `processes.html` - pre-rendered processes page
- etc.

When `/micromegas/login` was requested:
1. `ServeDir` couldn't find a file at `/login` or `/login/index.html`
2. Fell back to `serve_index_with_config` which always served `index.html`
3. `index.html` contains the root page which redirects to `/processes`
4. This caused an infinite redirect loop

### Fix
Modified `serve_index_with_config` to check for route-specific HTML files:
```rust
// Extract path after base path (e.g., /micromegas/login -> login)
let path_after_base = request_path.strip_prefix(&state.base_path)...;

// Try to find {path}.html (e.g., login.html)
let html_file = format!("{}.html", path_after_base);
let html = match tokio::fs::read_to_string(&html_path).await {
    Ok(content) => content,
    Err(_) => // Fall back to index.html for truly dynamic routes
};
```

## Frontend Fixes (Dec 11, 2025)

The infinite reload loop was also caused by the frontend's AuthGuard redirecting to `/login` without the base path. This caused:
1. User visits `/micromegas/processes`
2. AuthGuard checks auth, finds unauthenticated
3. AuthGuard redirects to `/login` (missing base path!)
4. Server doesn't serve `/login` → 404 or falls through
5. Loop continues

### Fixes Applied

**`analytics-web-app/src/components/AuthGuard.tsx`**:
- Changed from `router.push('/login?...')` to `window.location.href = \`${basePath}/login?...\``
- Now uses runtime config to get the base path

**`analytics-web-app/src/app/login/page.tsx`**:
- Added `getConfig()` import
- Fixed return URL handling to prepend base path if needed
- Default return URL now goes to `${basePath}/processes` instead of `/`

## Files Changed

### Backend (Rust)
- `rust/analytics-web-srv/src/main.rs` - Fixed routing with NormalizePathLayer wrapping; serve_index_with_config now serves route-specific HTML files
- `rust/analytics-web-srv/Cargo.toml` - Added `tower` dependency
- `rust/analytics-web-srv/tests/routing_tests.rs` - New test file for routing

### Frontend (TypeScript)
- `analytics-web-app/src/components/AuthGuard.tsx` - Use runtime basePath for login redirect
- `analytics-web-app/src/app/login/page.tsx` - Use runtime basePath for return URL

## Environment

- `MICROMEGAS_BASE_PATH=/micromegas` (required, must start with `/`)
- `MICROMEGAS_WEB_CORS_ORIGIN=https://...` (required)

## Internal Link Fixes (Dec 11, 2025)

Links in the frontend were hardcoded without the base path (e.g., `href="/process"`). Added `appLink()` helper to prepend runtime base path.

### Files Changed
- `analytics-web-app/src/lib/config.ts` - Added `appLink()` and `getLinkBasePath()` helpers
- `analytics-web-app/src/app/processes/page.tsx` - Fixed link to `/process`
- `analytics-web-app/src/app/process/page.tsx` - Fixed links to `/processes`, `/process_log`, etc.
- `analytics-web-app/src/app/process_log/page.tsx` - Fixed link to `/processes`
- `analytics-web-app/src/app/process_metrics/page.tsx` - Fixed link to `/processes`
- `analytics-web-app/src/app/performance_analysis/page.tsx` - Fixed link to `/processes`
- `analytics-web-app/src/app/process_trace/page.tsx` - Fixed link to `/processes`
- `analytics-web-app/src/components/ErrorBoundary.tsx` - Fixed router.push to `/login`

## JS Bundle Rewriting (Dec 11, 2025) - IMPLEMENTED

**Problem**: Next.js bakes many `/_next/` paths into JS bundles at build time:
- Webpack public path: `.p="/_next/"`
- Data fetching: `"/_next/data/"`
- Static assets: `"/_next/static/"`
- Image optimization: `"/_next/image"`

This means dynamic chunk imports and asset loading fail when using a runtime base path.

**Solution**: Intercept ALL requests to `/_next/static/chunks/*.js` and rewrite ALL `/_next/` references.

### Implementation in main.rs

Added `serve_js_chunk` handler that rewrites ALL JS files (not just webpack-*.js):

```rust
/// Serve JS chunks, rewriting all /_next/ paths for runtime base path support.
async fn serve_js_chunk(
    Path(filename): Path<String>,
    State(state): State<WebpackState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let file_path = format!("{}/_next/static/chunks/{filename}", state.frontend_dir);
    let content = tokio::fs::read(&file_path).await
        .map_err(|e| (StatusCode::NOT_FOUND, format!("Chunk not found: {e}")))?;

    let js = String::from_utf8_lossy(&content);

    // Rewrite all /_next/ references to include base path
    // This handles:
    // - .p="/_next/" (webpack public path)
    // - "/_next/data/" (data fetching)
    // - "/_next/static/" (static assets)
    // - "/_next/image" (image optimization)
    let modified_js = js
        .replace(r#""/_next/"#, &format!(r#""{}/_next/"#, state.base_path))
        .replace(r#"'/_next/"#, &format!(r#"'{}/_next/"#, state.base_path));

    // Also handle .p= assignments that may have different formats
    let modified_js = modified_js
        .replace(r#".p="/_next/"#, &format!(r#".p="{}/_next/"#, state.base_path));

    Ok(([(header::CONTENT_TYPE, "application/javascript; charset=utf-8")], modified_js.into_bytes()))
}
```

### Key Changes from Previous Version
- Now rewrites ALL JS files in chunks/, not just webpack-*.js
- Uses simple string replacement instead of complex pattern matching
- Handles both double-quoted and single-quoted `/_next/` strings
- Explicitly handles `.p=` webpack public path assignment

### Route Setup
```rust
let frontend = Router::new()
    .route("/index.html", get(serve_index_with_config))
    .with_state(index_state.clone())
    .route("/_next/static/chunks/{filename}", get(serve_js_chunk).with_state(webpack_state))
    .fallback_service(serve_dir);
```

### Build Instructions
Build frontend WITHOUT `NEXT_PUBLIC_BASE_PATH`:
```bash
cd analytics-web-app && yarn build
```

This creates a generic build. The server rewrites:
1. HTML asset paths (in `serve_index_with_config`)
2. Webpack public path (in `serve_js_chunk`)

## Testing

```bash
# Run backend routing tests
cd rust && cargo test --package analytics-web-srv --test routing_tests

# Local test
MICROMEGAS_BASE_PATH=/micromegas \
MICROMEGAS_WEB_CORS_ORIGIN=http://localhost \
cargo run --bin analytics-web-srv -- --disable-auth --frontend-dir /path/to/frontend

# Test endpoints (all should return HTML with __MICROMEGAS_CONFIG__)
curl http://localhost:3000/micromegas
curl http://localhost:3000/micromegas/
curl http://localhost:3000/micromegas/processes
curl http://localhost:3000/micromegas/login
```

## CSS Font URL Fix (Dec 11, 2025)

**Problem**: CSS files contain font URLs like `url(/_next/static/media/...)` which are baked at build time without the base path. This causes 404 errors for fonts.

**Solution**: Added `serve_css_file` handler that rewrites font URLs:
```rust
async fn serve_css_file(
    Path(filename): Path<String>,
    State(state): State<WebpackState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let content = tokio::fs::read_to_string(&file_path).await?;
    // Rewrite font URLs from url(/_next/...) to url({base_path}/_next/...)
    let modified_css = content.replace("url(/_next/", &format!("url({}/_next/", state.base_path));
    Ok(([(header::CONTENT_TYPE, "text/css; charset=utf-8")], modified_css))
}
```

## RSC Prefetch 404s - FIXED (Dec 11, 2025)

**Problem**: Next.js client makes prefetch requests to `/index.txt?_rsc=...` and `/processes.txt?_rsc=...` which don't exist in static export.

**Root Cause**:
1. Next.js bakes prefetch URLs at build time without base path
2. RSC prefetch `.txt` files don't exist in static export mode (they require a Next.js server)

**Solution**: Created `AppLink` component that disables prefetching:
```tsx
// src/components/AppLink.tsx
export function AppLink({ href, children, className, title, ...props }: AppLinkProps) {
  return (
    <Link href={appLink(href)} prefetch={false} className={className} title={title} {...props}>
      {children}
    </Link>
  )
}
```

The `AppLink` component:
1. Prepends the runtime base path to href via `appLink()`
2. Disables prefetching with `prefetch={false}` to avoid 404s

### Files Changed
- `src/components/AppLink.tsx` - New component
- `src/app/processes/page.tsx` - Use AppLink
- `src/app/process/page.tsx` - Use AppLink
- `src/app/process_log/page.tsx` - Use AppLink
- `src/app/process_metrics/page.tsx` - Use AppLink
- `src/app/process_trace/page.tsx` - Use AppLink
- `src/app/performance_analysis/page.tsx` - Use AppLink
- `src/components/layout/Sidebar.tsx` - Use AppLink
- `src/components/layout/Header.tsx` - Use AppLink + getLinkBasePath for logout

## Current Status: BLOCKED - Recommend Vite Migration

**Status**: Client-side navigation CSS 404s persist despite multiple fix attempts. Stopping further investigation.

**Recommendation**: Migrate from Next.js to Vite + React. See GitHub issue #657.

### What works:
- [x] Backend routing with NormalizePathLayer
- [x] Route-specific HTML file serving
- [x] Frontend AuthGuard basePath fix
- [x] Internal link fixes with appLink() helper
- [x] JS bundle rewriting (ALL chunks)
- [x] CSS font URL rewriting
- [x] RSC prefetch disabled via AppLink component
- [x] Initial page load (refresh) - all paths correct

### What doesn't work:
- [ ] Client-side navigation - CSS 404s to `/_next/static/css/...` (missing base path)

### Why we're stopping:
Next.js bakes paths in too many places. Each fix reveals another source of hardcoded paths:
1. HTML tag attributes → fixed
2. RSC inline data in HTML → fixed
3. RSC `.txt` payload files → fixed locally, still fails in prod
4. React DOM stylesheet loading → paths come from unknown source

This is a fundamental architectural mismatch: Next.js assumes build-time path configuration, we need runtime configuration.

## RSC Inline Data Path Fix (Dec 11, 2025)

**Problem**: Client-side navigation was causing 404s for CSS and icon.svg. The paths were embedded in RSC (React Server Components) inline script data, not in regular HTML tags.

Example RSC patterns in HTML that need rewriting:
```html
<script>self.__next_f.push([1,":HL[\"/_next/static/css/...","style"]"])</script>
<script>self.__next_f.push([1,"\"href\":\"/_next/static/css/...\""])</script>
<script>self.__next_f.push([1,"\"href\":\"/icon.svg\""])</script>
```

**Solution**: Extended `serve_index_with_config` to rewrite additional patterns:
```rust
let html = html
    // HTML attribute paths
    .replace("\"/_next/", &format!("\"{}/_next/", state.base_path))
    .replace("\"/icon.svg", &format!("\"{}/icon.svg", state.base_path))
    // RSC hint lists: :HL["/_next/..."]
    .replace(":HL[\"/_next/", &format!(":HL[\"{}/_next/", state.base_path))
    // RSC JSON paths: "href":"/_next/..." and "href":"/icon.svg"
    .replace("\"href\":\"/_next/", &format!("\"href\":\"{}/_next/", state.base_path))
    .replace("\"href\":\"/icon.svg\"", &format!("\"href\":\"{}/icon.svg\"", state.base_path));
```

## RSC Payload Files Fix (Dec 11, 2025)

**Problem**: Client-side navigation still caused CSS 404s. Investigation revealed that Next.js fetches `.txt` RSC payload files (e.g., `process.txt`, `login.txt`) during navigation. These files contain asset paths that weren't being rewritten.

**Root Cause**:
- HTML files served through `serve_index_with_config` → paths rewritten ✓
- `.txt` RSC payload files served through `ServeDir` → paths NOT rewritten ✗
- When navigating from page A to page B, Next.js fetches `B.txt` with raw `/_next/` paths

**Solution**: Modified `serve_index_with_config` to also handle `.txt` files:
```rust
async fn serve_index_with_config(...) -> Result<Response<String>, ...> {
    let path_after_base = request.uri().path()
        .strip_prefix(&state.base_path).unwrap_or(request_path)
        .trim_start_matches('/');

    // Check if this is an RSC payload request (.txt file)
    if path_after_base.ends_with(".txt") {
        return serve_rsc_payload_internal(&state, path_after_base).await;
    }
    // ... HTML handling continues
}

async fn serve_rsc_payload_internal(state: &IndexState, path: &str) -> Result<Response<String>, ...> {
    let content = tokio::fs::read_to_string(&format!("{}/{path}", state.frontend_dir)).await?;
    let modified = content
        .replace(r#"["/_next/"#, &format!(r#"["{}/_next/"#, state.base_path))
        .replace(r#""/icon.svg""#, &format!(r#""{}/icon.svg""#, state.base_path));
    // Return as text/plain
}
```

**Note**: Axum doesn't allow `/{filename}.txt` routes (can't mix parameter and literal in one segment). The solution handles `.txt` detection inside the existing SPA fallback handler.

## Known Issues (Dec 11, 2025) - UNRESOLVED

**Persistent issue**: CSS 404s during client-side navigation
```
GET https://telemetry.dev.scout.ubisoft.com/_next/static/css/06c2882df2f27f6d.css 404
```

Stack trace points to React DOM stylesheet loading code (`sQ`, `oq`, `ik` in `4bd1b696-*.js`), not RSC. The CSS path is being constructed from an unknown source that we haven't identified.

### Attempted fixes (reverted):
- RSC `.txt` payload file rewriting - worked locally but not in production
- RSC inline data rewriting in HTML - partially helped

### Root cause unknown:
The CSS path `/_next/static/css/...` is being used by React DOM to load stylesheets during client-side navigation. This path does NOT include the `/micromegas` base path. The source of this path remains unidentified after extensive investigation.

### Recommendation:
Migrate to Vite + React (issue #657) rather than continuing to patch Next.js.

## Debugging Tips

If paths are still wrong after deployment:
1. Check browser Network tab for 404s
2. For JS issues: View source of the JS file, search for `/_next/` - should show `{base_path}/_next/`
3. For CSS issues: View source of CSS file, search for `url(/_next/` - should show `url({base_path}/_next/`
4. For HTML issues: View page source, search for `/_next/` in script/link tags - should show `{base_path}/_next/`

To verify rewriting is working:
```bash
# Check if JS files are being rewritten
curl -s https://hostname/micromegas/_next/static/chunks/main-abc123.js | grep -o '"[^"]*_next/[^"]*"' | head -5

# Should output paths starting with /micromegas/_next/, NOT just /_next/
```
