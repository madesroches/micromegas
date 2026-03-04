# Auto-Refresh for Web App Screens

**Issue:** [#892](https://github.com/madesroches/micromegas/issues/892)

## Overview

Add a Grafana-style auto-refresh feature. A dropdown next to the refresh button lets users pick an interval (off, 5s, 10s, 30s, 1m, 5m, 15m, 30m, 1h). When active, screens re-execute at that interval using the existing `refreshTrigger` mechanism.

The feature lives at the ScreenPage/Header level so all screen types (notebook, table, log, metrics, process list) benefit — not just notebooks.

## Current State

- `ScreenPage` (`analytics-web-app/src/routes/ScreenPage.tsx:90`) manages `refreshTrigger` state. Incrementing it causes the renderer to re-execute.
- `Header` (`analytics-web-app/src/components/layout/Header.tsx:91-97`) renders a refresh button that calls `onRefresh` → `handleRefresh` → `setRefreshTrigger(n+1)`.
- `useCellExecution` (`analytics-web-app/src/lib/screen-renderers/useCellExecution.ts:306-312`) watches `refreshTrigger` and calls `executeFromCell(0)` on change.
- All renderers accept `refreshTrigger` via `ScreenRendererProps`.
- `ScreenConfig` has an index signature `[key: string]: unknown` so adding `refreshIntervalMs` requires no type changes.
- `PageLayout` passes `onRefresh` and `timeRangeControl` to `Header`.

## Design

### Refresh Interval Picker

A dropdown button attached to the right of the existing refresh button in the header toolbar. It shows a chevron/caret; clicking it opens a popover with interval presets.

**Presets:** Off, 5s, 10s, 30s, 1m, 5m, 15m, 30m, 1h

When an interval is active:
- The refresh icon spins continuously (CSS animation)
- The interval label (e.g. "5s") appears next to the icon
- Clicking the refresh icon itself still triggers an immediate manual refresh
- Clicking the dropdown allows changing or disabling the interval

### State Management

```
ScreenPage
├── refreshIntervalMs: number (0 = off, stored in screenConfig)
├── useRefreshInterval(intervalMs, onTick) → manages setInterval
└── onTick → setRefreshTrigger(n+1) (existing mechanism)
```

The interval value persists in `screenConfig.refreshIntervalMs` so it saves with the screen. On load, the saved interval resumes automatically.

### Hook: `useRefreshInterval`

```typescript
function useRefreshInterval(
  intervalMs: number,
  onTick: () => void
): void
```

- When `intervalMs > 0`, starts a `setInterval` that calls `onTick`
- Cleans up on unmount or when `intervalMs` changes
- Uses a ref for `onTick` to avoid resetting the interval when the callback identity changes

### Props Flow

```
ScreenPage
  → PageLayout (new props: refreshIntervalMs, onRefreshIntervalChange)
    → Header (new props: refreshIntervalMs, onRefreshIntervalChange)
      → RefreshIntervalPicker (new component)
```

`PageLayout` and `Header` gain two optional props:
- `refreshIntervalMs?: number`
- `onRefreshIntervalChange?: (ms: number) => void`

When not provided (non-screen pages), the refresh button renders as before.

### Component: `RefreshIntervalPicker`

Small self-contained component. Renders:
- The refresh icon (spinning when active)
- A chevron button that opens the interval dropdown
- A popover/dropdown with the preset list
- A checkmark next to the active interval

See `mockup_header.html` and `mockup_dropdown.html` for visual reference.

## Implementation Steps

### Step 1: Create `useRefreshInterval` hook
- **New file:** `analytics-web-app/src/hooks/useRefreshInterval.ts`
- Simple hook: `setInterval` with cleanup, ref-based callback

### Step 2: Create `RefreshIntervalPicker` component
- **New file:** `analytics-web-app/src/components/layout/RefreshIntervalPicker.tsx`
- Use `@radix-ui/react-dropdown-menu` for the interval dropdown (already a dependency, used in `SplitButton.tsx`)
- Spinning icon state, interval label
- Reuse existing styling patterns (bg-theme-border, hover states, rounded-md)

### Step 3: Update Header to include the picker
- **Modify:** `analytics-web-app/src/components/layout/Header.tsx`
- Header has **two** refresh button code paths that both need updating:
  1. Inside the `timeRangeControl` block (line 91-97) — refresh button joined with zoom buttons
  2. Standalone `onRefresh` branch (lines 100-106) — refresh button rendered alone
- When `onRefreshIntervalChange` is provided, replace the refresh button in **both** paths with `RefreshIntervalPicker`
- Keep the simple refresh button as fallback for non-screen pages

### Step 4: Thread props through PageLayout
- **Modify:** `analytics-web-app/src/components/layout/PageLayout.tsx`
- Add `refreshIntervalMs` and `onRefreshIntervalChange` to `PageLayoutProps`
- Pass through to `Header`

### Step 5: Wire up in ScreenPage
- **Modify:** `analytics-web-app/src/routes/ScreenPage.tsx`
- Read `refreshIntervalMs` from `screenConfig` (default 0)
- Create handler to update config: `handleRefreshIntervalChange(ms)` → `handleScreenConfigChange`
- Call `useRefreshInterval(intervalMs, handleRefresh)`
- Pass interval props to `PageLayout`

## Files to Modify

| File | Change |
|------|--------|
| `analytics-web-app/src/hooks/useRefreshInterval.ts` | **New** — interval hook |
| `analytics-web-app/src/components/layout/RefreshIntervalPicker.tsx` | **New** — dropdown component |
| `analytics-web-app/src/components/layout/Header.tsx` | Integrate picker next to refresh button |
| `analytics-web-app/src/components/layout/PageLayout.tsx` | Thread new props |
| `analytics-web-app/src/routes/ScreenPage.tsx` | Wire interval state + hook |

## Mockups

- `mockup_header.html` — Header bar showing active auto-refresh state (interval label + spinning icon)
- `mockup_dropdown.html` — Dropdown open with interval presets

## Trade-offs

**Screen-level vs notebook-only:** Putting auto-refresh at the ScreenPage/Header level means all screen types benefit with zero extra work per renderer. The `refreshTrigger` mechanism already exists for all of them. Downside: the interval setting is in the generic `ScreenConfig` rather than typed in `NotebookConfig`, but the index signature handles this cleanly.

**URL param vs config-only:** Could store interval in a URL param (`?refresh=5s`) for shareability. Decided against — auto-refresh is a personal workflow preference, not something you'd typically want to share via link. Persisting in config (saved with screen) is sufficient.

**Pause during editing:** Considered pausing auto-refresh while the user is editing a cell. Deferred — Grafana doesn't do this either, and the user can simply set interval to "Off". Can add later if needed.

## Testing Strategy

- **Unit test** `useRefreshInterval`: verify interval fires, cleanup on unmount, callback ref stability
- **Unit test** `RefreshIntervalPicker`: renders presets, calls onChange, shows active state
- **Manual test**: open a notebook, set 5s interval, verify cells re-execute every 5s, verify icon spins, verify saving persists the interval, verify reloading resumes the interval

## Open Questions

None — the design leverages existing infrastructure (`refreshTrigger`, `ScreenConfig` index signature) with minimal new surface area.
