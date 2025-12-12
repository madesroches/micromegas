# Perfetto Trace Split Button - Implementation Plan

## Status: COMPLETED

Implemented on 2025-12-12.

## Overview

Convert the "Download Perfetto Trace" button on the Performance Analysis screen into a split button (combo-box-button) that offers two actions:
- **Open in Perfetto** (default): Opens trace directly in ui.perfetto.dev
- **Download**: Downloads trace file (.pb) locally

## Reference Implementation

The existing Rust server has a working "Open in Perfetto" implementation:
- `rust/public/src/servers/perfetto/show_trace.html`

Key technique: Use `window.postMessage` to send trace data to Perfetto UI.

## Files to Create

| File | Purpose |
|------|---------|
| `analytics-web-app/src/components/ui/SplitButton.tsx` | Reusable split button with dropdown |
| `analytics-web-app/src/lib/perfetto.ts` | Perfetto integration utilities |

## Files to Modify

| File | Change |
|------|--------|
| `analytics-web-app/package.json` | Add @radix-ui/react-dropdown-menu |
| `analytics-web-app/src/lib/api.ts` | Modify generateTrace to optionally return buffer |
| `analytics-web-app/src/app/performance_analysis/page.tsx` | Replace button with SplitButton |

## Component Design

### SplitButton Component

Visual representation:
```
┌─────────────────────────┬───┐
│  Open in Perfetto       │ ▼ │
└─────────────────────────┴───┘
                          │
                          ▼
                    ┌───────────────┐
                    │ Download      │
                    └───────────────┘
```

Props:
```typescript
interface SplitButtonProps {
  primaryLabel: string
  primaryIcon?: React.ReactNode
  onPrimaryClick: () => void
  secondaryActions: Array<{
    label: string
    icon?: React.ReactNode
    onClick: () => void
  }>
  disabled?: boolean
  loading?: boolean
}
```

### API Changes

Modify `generateTrace` in `api.ts`:

```typescript
interface GenerateTraceOptions {
  returnBuffer?: boolean  // If true, return ArrayBuffer instead of downloading
}

export async function generateTrace(
  processId: string,
  request: GenerateTraceRequest,
  onProgress?: (update: ProgressUpdate) => void,
  options?: GenerateTraceOptions
): Promise<ArrayBuffer | void>
```

### Perfetto Integration (`src/lib/perfetto.ts`)

```typescript
export interface OpenPerfettoOptions {
  buffer: ArrayBuffer
  processId: string
  timeRange: { begin: string; end: string }
}

export async function openInPerfetto(options: OpenPerfettoOptions): Promise<void>
```

Implementation based on `show_trace.html`:
1. Calculate time range in nanoseconds for Perfetto URL params
2. Open `https://ui.perfetto.dev/#!/?visStart=${begin_ns}&visEnd=${end_ns}`
3. Ping/pong handshake with `postMessage('PING', perfetto_url)`
4. On PONG response, send trace buffer via `postMessage({ perfetto: { buffer, title } })`
5. Handle popup blockers gracefully with user feedback

## Implementation Steps

### Step 1: Add Dependency [DONE]
```bash
cd analytics-web-app && yarn add @radix-ui/react-dropdown-menu
```

### Step 2: Create SplitButton Component [DONE]
- Use Radix DropdownMenu for the dropdown portion
- Style to match existing button patterns (accent-link colors)
- Support loading state with spinner
- Chevron icon for dropdown trigger

### Step 3: Create Perfetto Utilities [DONE]
- Port `open_perfetto()` logic from `show_trace.html`
- Add timeout handling for popup blocker detection
- Return Promise that resolves when trace is sent to Perfetto

### Step 4: Refactor generateTrace API [DONE]
- Extract binary streaming logic
- Add `returnBuffer` option
- Keep backward compatibility (default behavior unchanged)

### Step 5: Update Performance Analysis Page [DONE]
- Replace current button with SplitButton
- Primary action: Open in Perfetto
- Secondary action: Download
- Update progress messages for "Opening in Perfetto..." vs "Downloading..."

### Step 6: Testing [DONE]
- Type-check passes
- Lint passes
- Build passes
- All existing tests pass

## Error Handling

### Popup Blocked
If `window.open()` returns null:
- Show error banner: "Popup blocked. Please allow popups for this site to open Perfetto."
- Offer "Download instead" as fallback

### Perfetto Timeout
If no PONG received within 10 seconds:
- Show error: "Could not connect to Perfetto UI. Try again or download the trace."

## UI States

| State | Primary Button | Dropdown |
|-------|----------------|----------|
| Idle | "Open in Perfetto" | Enabled |
| Generating | "Generating..." + spinner | Disabled |
| Opening | "Opening..." + spinner | Disabled |
| Downloading | "Downloading..." + spinner | Disabled |

## Backend Changes

None required - existing `/perfetto/{process_id}/generate` endpoint works for both use cases.

## Implementation Summary

### Files Created
- `analytics-web-app/src/components/ui/SplitButton.tsx` - Reusable split button with Radix dropdown
- `analytics-web-app/src/lib/perfetto.ts` - Perfetto integration with postMessage handshake

### Files Modified
- `analytics-web-app/package.json` - Added `@radix-ui/react-dropdown-menu@2.1.16`
- `analytics-web-app/src/lib/api.ts` - Added `GenerateTraceOptions` interface and `returnBuffer` option
- `analytics-web-app/src/app/performance_analysis/page.tsx` - Replaced button with SplitButton, added dual handlers
