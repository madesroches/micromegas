# Next.js to Vite Migration Plan

**Issue**: [#657](https://github.com/madesroches/micromegas/issues/657)
**Goal**: Eliminate build-time path baking by migrating to Vite with relative base path (`'./'`)

## Summary

The analytics-web-app is a pure client-side SPA (~3,000 lines, 25 components) using Next.js features that are easily replaced. Migration involves:
- Replace Next.js with Vite + React Router
- Use relative base path for deployment flexibility
- Simplify backend serving (no more path rewriting)

---

## Phase 1: Project Setup

### 1.1 Create Vite configuration
**Create**: `analytics-web-app/vite.config.ts`
```typescript
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import path from 'path'

export default defineConfig({
  plugins: [react()],
  base: './',
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  build: {
    outDir: 'dist',
    sourcemap: true,
  },
  server: {
    port: 3000,
    proxy: {
      '/api': 'http://localhost:8000',
      '/auth': 'http://localhost:8000',
      '/query': 'http://localhost:8000',
      '/perfetto': 'http://localhost:8000',
    },
  },
})
```

### 1.2 Create entry HTML
**Create**: `analytics-web-app/index.html`
```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Micromegas</title>
  <link rel="icon" type="image/svg+xml" href="/icon.svg">
  <link rel="preconnect" href="https://fonts.googleapis.com">
  <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
  <link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap" rel="stylesheet">
</head>
<body>
  <div id="root"></div>
  <script type="module" src="/src/main.tsx"></script>
</body>
</html>
```

### 1.3 Update package.json
**Modify**: `analytics-web-app/package.json`
- Remove: `next`, `eslint-config-next`
- Add: `vite`, `@vitejs/plugin-react`, `react-router-dom`
- Update scripts:
  ```json
  "scripts": {
    "dev": "vite",
    "build": "tsc && vite build",
    "preview": "vite preview",
    "lint": "eslint .",
    "type-check": "tsc --noEmit"
  }
  ```

### 1.4 Update tsconfig.json
**Modify**: `analytics-web-app/tsconfig.json`
- Remove Next.js plugin
- Change `"jsx": "preserve"` to `"jsx": "react-jsx"`

---

## Phase 2: Core Infrastructure

### 2.1 Create application entry point
**Create**: `analytics-web-app/src/main.tsx`

### 2.2 Create router configuration
**Create**: `analytics-web-app/src/router.tsx`

Routes to define:
| Path | Component |
|------|-----------|
| `/` | Redirect to `/processes` |
| `/login` | LoginPage |
| `/processes` | ProcessesPage |
| `/process` | ProcessPage |
| `/process_log` | ProcessLogPage |
| `/process_metrics` | ProcessMetricsPage |
| `/process_trace` | ProcessTracePage |
| `/performance_analysis` | PerformanceAnalysisPage |

### 2.3 Update config.ts
**Modify**: `analytics-web-app/src/lib/config.ts`
- Remove `NEXT_PUBLIC_*` references
- Use `import.meta.env.DEV` for development detection

---

## Phase 3: Component Migration

### 3.1 Navigation hooks replacement
| Next.js | React Router |
|---------|--------------|
| `useRouter()` | `useNavigate()` |
| `useSearchParams()` | `useSearchParams()` |
| `usePathname()` | `useLocation().pathname` |

### 3.2 Files to modify
- `src/components/AppLink.tsx` - Use `Link` from react-router-dom
- `src/components/AuthGuard.tsx` - Replace usePathname
- `src/components/layout/Sidebar.tsx` - Replace usePathname
- `src/hooks/useTimeRange.ts` - Replace navigation hooks

---

## Phase 4: Page Migration

### 4.1 Create routes directory
**Create**: `analytics-web-app/src/routes/`

### 4.2 Migrate pages
Move and update each page:
- `src/app/page.tsx` -> `src/routes/HomePage.tsx`
- `src/app/login/page.tsx` -> `src/routes/LoginPage.tsx`
- `src/app/processes/page.tsx` -> `src/routes/ProcessesPage.tsx`
- `src/app/process/page.tsx` -> `src/routes/ProcessPage.tsx`
- `src/app/process_log/page.tsx` -> `src/routes/ProcessLogPage.tsx`
- `src/app/process_metrics/page.tsx` -> `src/routes/ProcessMetricsPage.tsx`
- `src/app/process_trace/page.tsx` -> `src/routes/ProcessTracePage.tsx`
- `src/app/performance_analysis/page.tsx` -> `src/routes/PerformanceAnalysisPage.tsx`

Changes per file:
- Remove `'use client'` directive
- Update navigation imports

---

## Phase 5: Backend Updates

### 5.1 Simplify analytics-web-srv
**Modify**: `rust/analytics-web-srv/src/main.rs`
- Change frontend_dir default: `../analytics-web-app/out` -> `../analytics-web-app/dist`
- Remove `serve_js_chunk` and `serve_css_file` handlers (no `/_next/` rewriting needed)
- Simplify to serve `index.html` for all frontend routes with config injection

---

## Phase 6: Cleanup

### 6.1 Delete obsolete files
- `analytics-web-app/next.config.mjs`
- `analytics-web-app/next-env.d.ts`
- `analytics-web-app/.next/`
- `analytics-web-app/out/`
- `analytics-web-app/src/app/` (after verification)

### 6.2 Update .gitignore
- Remove `.next`
- Add `dist`

### 6.3 Testing checklist
- [ ] `yarn dev` starts frontend
- [ ] `yarn build` produces `dist/`
- [ ] All routes work
- [ ] Auth flow works
- [ ] Time range picker persists in URL
- [ ] Backend serves correctly with base path

---

## Progress Log

### Phase 1: Project Setup - DONE
- [x] Create vite.config.ts
- [x] Create index.html
- [x] Update package.json
- [x] Update tsconfig.json

### Phase 2: Core Infrastructure - DONE
- [x] Create src/main.tsx
- [x] Create src/router.tsx
- [x] Update src/lib/config.ts
- [x] Create src/vite-env.d.ts

### Phase 3: Component Migration - DONE
- [x] Update AppLink.tsx (use react-router-dom Link)
- [x] Update AuthGuard.tsx (replace usePathname)
- [x] Update Sidebar.tsx (replace usePathname)
- [x] Update useTimeRange.ts (replace navigation hooks)

### Phase 4: Page Migration - DONE
- [x] Create src/routes/ directory
- [x] Migrate LoginPage
- [x] Migrate ProcessesPage
- [x] Migrate ProcessPage
- [x] Migrate ProcessLogPage
- [x] Migrate ProcessMetricsPage
- [x] Migrate ProcessTracePage
- [x] Migrate PerformanceAnalysisPage
- [x] Remove 'use client' directives from components

### Phase 5: Backend Updates - DONE
- [x] Update frontend_dir default to dist
- [x] Simplify serve handlers (remove _next rewriting)

### Phase 6: Cleanup - DONE
- [x] Delete next.config.mjs
- [x] Delete next-env.d.ts
- [x] Delete src/app/ directory
- [x] Update .gitignore

### Phase 7: Documentation - DONE
- [x] Update analytics-web-app/README.md
- [x] Update CLAUDE.md
- [x] Update mkdocs/docs/admin/web-app.md

### Phase 8: Testing - DONE
- [x] Install dependencies
- [x] Run yarn build - SUCCESS
- [x] Verify dist/ output - index.html, assets/, icon.svg created
- [x] Fix ESLint config (removed Next.js plugins, added jest.config.js to ignore)
- [x] Run yarn lint - passes with warnings only (no errors)
- [x] Update jest.config.js for Vite (removed Next.js dependency)
- [x] Update test-setup.ts to mock react-router-dom instead of next/navigation
- [x] Exclude test files from TypeScript build

---

## Phase 7: Documentation Updates

### 7.1 analytics-web-app/README.md
- Line 19: Change "Next.js 15 + React 18" to "Vite + React 18 + React Router"
- Line 62: Change "Next.js dev server" to "Vite dev server"
- Line 89: Change "Next.js frontend" to "Vite frontend"

### 7.2 CLAUDE.md
- Line 60: Change "starts Next.js dev server" to "starts Vite dev server"

### 7.3 mkdocs/docs/admin/web-app.md
- Line 97: Change "Next.js on port 3000" to "Vite/React SPA on port 3000"
- Line 111: Change `--frontend-dir` default from `../analytics-web-app/out` to `../analytics-web-app/dist`

### 7.4 AI_GUIDELINES.md
- No changes needed (doesn't reference Next.js specifically)

---

## Critical Files

| File | Action |
|------|--------|
| `analytics-web-app/package.json` | Modify |
| `analytics-web-app/vite.config.ts` | Create |
| `analytics-web-app/index.html` | Create |
| `analytics-web-app/src/main.tsx` | Create |
| `analytics-web-app/src/router.tsx` | Create |
| `analytics-web-app/src/lib/config.ts` | Modify |
| `analytics-web-app/src/components/AppLink.tsx` | Modify |
| `analytics-web-app/src/hooks/useTimeRange.ts` | Modify |
| `rust/analytics-web-srv/src/main.rs` | Modify |
| `analytics-web-app/README.md` | Modify |
| `CLAUDE.md` | Modify |
| `mkdocs/docs/admin/web-app.md` | Modify |
