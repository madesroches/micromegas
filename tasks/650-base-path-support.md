# Plan: Add BASE_PATH Support (GitHub Issue #650)

## Goal
Enable deploying analytics-web-srv behind a reverse proxy with custom URL prefix (e.g., `/micromegas/*`) using a single container image that works for any base path.

## Configuration
- **Environment variable only**: `MICROMEGAS_BASE_PATH` (optional, defaults to empty)
- **Runtime injection**: Backend injects config into `index.html` at serve time
- **No rebuild needed**: Same container works for any base path

## Implementation

### 1. Backend: Runtime Config Injection (`rust/analytics-web-srv/src/main.rs`)

**Read env var:**
```rust
let base_path = std::env::var("MICROMEGAS_BASE_PATH")
    .unwrap_or_default()
    .trim_end_matches('/')
    .to_string();
```

**Add custom index.html handler** that:
1. Reads `index.html` from disk
2. Injects `<script>window.__MICROMEGAS_CONFIG__={basePath:"...",apiBase:"...",authBase:"..."}</script>` before `</head>`
3. Serves modified HTML

**Prefix all routes** with base_path:
- `{base_path}/auth/*`
- `{base_path}/analyticsweb/*`

**Serve static assets** under base path using `Router::nest()`.

### 2. Backend: Cookie Path (`rust/analytics-web-srv/src/auth.rs`)

Add `base_path: String` to `AuthState` struct and use it for cookie path (defaults to `/` if empty).

### 3. Frontend: Runtime Config (`analytics-web-app/src/lib/config.ts` - new file)

```typescript
interface RuntimeConfig {
  basePath: string
  apiBase: string
  authBase: string
}

declare global {
  interface Window {
    __MICROMEGAS_CONFIG__?: RuntimeConfig
  }
}

export function getConfig(): RuntimeConfig {
  if (typeof window !== 'undefined' && window.__MICROMEGAS_CONFIG__) {
    return window.__MICROMEGAS_CONFIG__
  }
  // Development fallback
  return {
    basePath: '',
    apiBase: process.env.NODE_ENV === 'development' ? 'http://localhost:8000/analyticsweb' : '/analyticsweb',
    authBase: process.env.NODE_ENV === 'development' ? 'http://localhost:8000' : '',
  }
}
```

### 4. Frontend: Update API Calls

**`analytics-web-app/src/lib/api.ts`:**
```typescript
import { getConfig } from './config'
// Replace: const API_BASE = ...
// With: const API_BASE = getConfig().apiBase (called at usage time, not module load)
```

**`analytics-web-app/src/lib/auth.tsx`:**
```typescript
import { getConfig } from './config'
// Replace: const API_BASE = ...
// With: const API_BASE = getConfig().authBase (called at usage time, not module load)
```

## Files to Modify

| File | Changes |
|------|---------|
| `rust/analytics-web-srv/src/main.rs` | Read env var, inject config into index.html, prefix routes, nest static files |
| `rust/analytics-web-srv/src/auth.rs` | Add `base_path` to AuthState, update cookie path |
| `analytics-web-app/src/lib/config.ts` | **NEW** - runtime config reader |
| `analytics-web-app/src/lib/api.ts` | Use `getConfig().apiBase` instead of compile-time constant |
| `analytics-web-app/src/lib/auth.tsx` | Use `getConfig().authBase` instead of compile-time constant |

## Deployment Example

```bash
# Same container, different deployments:
MICROMEGAS_BASE_PATH="/micromegas" docker run ...
MICROMEGAS_BASE_PATH="/analytics" docker run ...
MICROMEGAS_BASE_PATH="" docker run ...  # default, no prefix
```

## Backwards Compatibility

- Empty `MICROMEGAS_BASE_PATH` (default) = current behavior
- No changes needed to existing deployments
