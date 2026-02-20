# Fix fade-on-idle for notebook cell metadata

## Problem

After cells execute in a notebook (e.g. user hits the refresh button next to time range), the cell metadata (names, status text, group headers) should reveal, stay visible for ~4 seconds, then fade out. On hover, metadata should reveal and stay visible for 4 seconds after the mouse leaves.

## Root cause

Two issues combined:

1. **React batching swallows status changes for fast cells.** On re-execution, `executeFromCell` calls `setCellStates({status: 'loading'})` then awaits execution, then `setCellStates({status: 'success'})`. For fast cells (WASM queries, variables), React 18 batches these updates so the component only ever sees `success → success` — no status change is detected by `useFadeOnIdle`.

2. **Hover had no fade delay.** When the mouse left a cell, the CSS transition from opacity:1 → opacity:0 started immediately with no delay.

### Confirmed by tracing

Added `console.log` to `useFadeOnIdle` to trace status transitions. On refresh:
- `cell:data` (slow remote fetch): `success → loading → success` — hook detects change, reveal works
- `cell:ingestion`, `cell:flightsql`, `cell:daemon_task_latency` (fast WASM): **no status change logged** — React batched loading+success, hook never fires
- `cell:mysource` (variable, no execute method): goes directly to `success` without `loading`

## Solution

Three changes across three files:

### 1. Reset cell statuses before re-execution (`useCellExecution.ts`)

In `executeFromCell`, reset all affected cells to `idle` before the execution loop. This guarantees every cell goes through `idle → loading → success`, giving `useFadeOnIdle` a real status change to detect — even when React batches the fast `loading → success` transition.

### 2. Reveal on any non-idle status change (`useFadeOnIdle.ts`)

Simplified the hook: reveal on any status change away from `idle`. During `loading`, stay revealed. For terminal states (`success`, `error`, `blocked`), keep `revealed` for 200ms (enough for the CSS 150ms fade-in to complete), then remove the class and let CSS handle the delayed fade-out.

### 3. CSS transition-delay for 4s fade-out wait (`globals.css`)

Added `4s` transition-delay to the base `.fade-on-idle` rule. This handles the 4-second visibility window for both:
- **JS-driven reveals**: after `revealed` class is removed, CSS waits 4s then fades over 1s
- **Hover reveals**: after mouse leaves, CSS waits 4s then fades over 1s

## Files changed

| File | Change |
|------|--------|
| `src/lib/screen-renderers/useCellExecution.ts` | Reset cell statuses to `idle` at start of `executeFromCell` |
| `src/hooks/useFadeOnIdle.ts` | Simplified hook: reveal on any non-idle change, CSS handles timing |
| `src/styles/globals.css` | Added `4s` transition-delay to `.fade-on-idle` |
