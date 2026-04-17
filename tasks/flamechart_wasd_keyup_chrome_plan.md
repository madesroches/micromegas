# Flamechart WASD Keyup Stuck in Chrome — Plan

Issue: [#1012](https://github.com/madesroches/micromegas/issues/1012) — WASD zoom on a flamechart does not stop when the key is released in Chrome (works correctly in Firefox).

## Overview

The flame graph cell drives continuous WASD zoom/pan via a `requestAnimationFrame` loop that ticks while at least one WASD key is in a `Set`. The set is filled on `keydown` and drained on `keyup`. In Chrome, the keyup *is* delivered, but `e.key` sometimes comes back as `"Unidentified"` even when `e.code` is correct (`"KeyW"`, etc.). Because the original code keyed off `e.key.toLowerCase()`, the delete missed and the held key stayed in the set forever. The primary fix is to identify the key by `e.code`. Focus / blur / visibility safety nets are kept as defense in depth for the rarer focus-loss paths.

**Diagnosis evidence** (from instrumented Chrome run):

```
[WASD] keydown raw= "w" code= KeyW lower= w repeat= false target= DIV
[WASD] tick keys= ['w']    ← held
[WASD] keyup raw= "Unidentified" code= KeyW lower= unidentified target= DIV hadKey= false
[WASD] tick keys= ['w']    ← still held after release because delete("unidentified") was a no-op
```

For comparison, the S keyup in the same session correctly returned `e.key === "s"`, which is why combined-key release sometimes worked and the bug was intermittent.

## Current State

The behavior lives in `analytics-web-app/src/lib/screen-renderers/cells/FlameGraphCell.tsx`.

Relevant locations:

- `keysRef` and `keyAnimRef` declared at `FlameGraphCell.tsx:818-819`.
- `keyTick` rAF loop at `FlameGraphCell.tsx:821-846` — exits only when `keysRef.current.size === 0`.
- `useEffect` that wires `keydown` / `keyup` / `wheel` listeners on the `container` div at `FlameGraphCell.tsx:1001-1028`.
- The container is a focusable `<div tabIndex={0} … outline-none>` at `FlameGraphCell.tsx:1031-1041`.
- Mouse-down focuses the container at `FlameGraphCell.tsx:873`.

```tsx
const onKeyDown = (e: KeyboardEvent) => {
  const key = e.key.toLowerCase()
  if ('wasd'.includes(key)) {
    e.preventDefault()
    keysRef.current.add(key)
    if (!keyAnimRef.current) keyAnimRef.current = requestAnimationFrame(keyTick)
  }
}
const onKeyUp = (e: KeyboardEvent) => {
  keysRef.current.delete(e.key.toLowerCase())
}

container.addEventListener('keydown', onKeyDown)
container.addEventListener('keyup', onKeyUp)
```

Why this wedges in Chrome (but typically not Firefox):

- **`e.key === "Unidentified"` on keyup.** In some Chrome / OS / keyboard combinations the keyup fires with `e.key` set to the literal string `"Unidentified"` while `e.code` is still correct (`"KeyW"`). The original `keys.delete(e.key.toLowerCase())` then deletes `"unidentified"`, leaving the real `"w"` in the set. Firefox returns the actual key string here, so it's not visibly broken there.
- **Secondary risk: no `blur` / `visibilitychange` cleanup.** Even with `e.code` keying, if the user Alt-Tabs / switches tabs / clicks away while a key is held, the OS-level keyup may be delivered to a window we are no longer listening from. Without a focus/visibility safety net, the key stays stuck.
- **No safety net.** There is no maximum-duration guard, so once the set is wedged the animation loop runs until the cell unmounts.

There are no existing tests for the WASD interaction (only `FlameGraphLayout.test.ts` covers layout math).

## Design

Identify keys by `e.code` instead of `e.key` so the keyup match is invariant under the Chrome `"Unidentified"` quirk and across keyboard layouts. Make the "key is no longer held" signal further robust by listening at `window` and by clearing all keys on any focus/visibility loss event. The keydown listener stays on the container so that WASD only triggers when the user has the flame graph focused — that part works correctly today.

### Behavior changes

1. **Identify keys by `e.code`** via a small `codeToKey(code)` helper that maps `"KeyW"/"KeyA"/"KeyS"/"KeyD"` → `"w"/"a"/"s"/"d"` and returns `null` otherwise. Both `keydown` and `keyup` route through this helper, so the lookup that adds and the lookup that removes are guaranteed to match.
2. `keyup` listener moves from `container` to `window`. Keyup is global by nature — once a key was registered, we want to clear it regardless of where focus has wandered.
3. Add a `blur` listener on `window` that clears the entire `keysRef` set and cancels the animation. Triggered when the tab/window loses focus (Alt-Tab, switching apps, switching browser tabs, devtools focus, etc.).
4. Add a `blur` listener on `container` that clears the set. Triggered when focus moves to another element in the same document (clicking another cell, tabbing away).
5. Add a `visibilitychange` listener on `document` that clears the set when `document.hidden` is true. Defense-in-depth for tab switches that don't fire `blur`.
6. Extract a small helper inside the effect — `clearAllKeys()` — that drains `keysRef`, cancels `keyAnimRef`, and resets it to 0. All "release" paths (keyup, container blur, window blur, visibilitychange) ultimately call this helper or `keys.delete(key)` for a known code.

### Why these particular events

| Event                                 | Catches                                                  |
| ------------------------------------- | -------------------------------------------------------- |
| `window` `keyup`                      | Any keyup, regardless of which element currently has focus |
| `container` `blur`                    | Focus moved to another element in the same document      |
| `window` `blur`                       | User switched windows / apps / tabs                      |
| `document` `visibilitychange` (hidden)| Tab became hidden (some platforms skip `blur`)           |

The keydown listener stays on `container` so that pressing WASD outside the flame graph doesn't hijack the keys.

### Code shape (illustrative — full code in implementation step)

```tsx
useEffect(() => {
  const container = containerRef.current
  if (!container) return

  // Capture the ref deref once; the existing code does the same to keep
  // react-hooks/exhaustive-deps quiet about reading refs from cleanup.
  const keys = keysRef.current

  const clearAllKeys = () => {
    keys.clear()
    if (keyAnimRef.current) {
      cancelAnimationFrame(keyAnimRef.current)
      keyAnimRef.current = 0
    }
  }

  const codeToKey = (code: string): string | null => {
    switch (code) {
      case 'KeyW': return 'w'
      case 'KeyA': return 'a'
      case 'KeyS': return 's'
      case 'KeyD': return 'd'
      default: return null
    }
  }

  const onKeyDown = (e: KeyboardEvent) => {
    const key = codeToKey(e.code)
    if (key) {
      e.preventDefault()
      keys.add(key)
      if (!keyAnimRef.current) keyAnimRef.current = requestAnimationFrame(keyTick)
    }
  }
  const onKeyUp = (e: KeyboardEvent) => {
    const key = codeToKey(e.code)
    if (key) keys.delete(key)
  }
  const onVisibilityChange = () => {
    if (document.hidden) clearAllKeys()
  }

  container.addEventListener('keydown', onKeyDown)
  container.addEventListener('blur', clearAllKeys)
  window.addEventListener('keyup', onKeyUp)
  window.addEventListener('blur', clearAllKeys)
  document.addEventListener('visibilitychange', onVisibilityChange)
  container.addEventListener('wheel', handleWheel, { passive: true })

  return () => {
    container.removeEventListener('keydown', onKeyDown)
    container.removeEventListener('blur', clearAllKeys)
    window.removeEventListener('keyup', onKeyUp)
    window.removeEventListener('blur', clearAllKeys)
    document.removeEventListener('visibilitychange', onVisibilityChange)
    container.removeEventListener('wheel', handleWheel)
    clearAllKeys()
  }
}, [handleWheel, keyTick])
```

Notes on the shape:

- `keysRef.current` is captured into a local `keys` at the top of the effect so all handlers (including `clearAllKeys`, which is called from cleanup) close over a stable local instead of reading the ref from inside the cleanup closure. This matches what the existing code already does and avoids tripping `react-hooks/exhaustive-deps`.
- The cleanup function calls `clearAllKeys()` instead of the previous `keys.clear()` + `cancelAnimationFrame` so the cancel and the clear stay in lockstep.
- `onKeyUp` runs on every keyup in the document, but `Set.delete` on a key that is not in the set is a harmless no-op, so there's no need to filter to WASD only.
- We do not need `pointermove`/`mouseleave` heuristics — the four signals above cover the realistic stuck-key paths.

## Implementation Steps

1. **Edit `analytics-web-app/src/lib/screen-renderers/cells/FlameGraphCell.tsx`** (the `useEffect` at lines 1001-1028):
   - Keep the existing `const keys = keysRef.current` capture at the top of the effect; have the new handlers and helper close over `keys` rather than dereferencing the ref.
   - Introduce a local `clearAllKeys` helper that clears `keys` and cancels `keyAnimRef.current`.
   - Move the `keyup` listener from `container` to `window`.
   - Add a `blur` listener on `container` that calls `clearAllKeys`.
   - Add a `blur` listener on `window` that calls `clearAllKeys`.
   - Add a `visibilitychange` listener on `document` that calls `clearAllKeys` when `document.hidden`.
   - Update the cleanup to remove all of the above and call `clearAllKeys` once at unmount.

2. **Manual verification in Chrome** (matrix below, see Testing Strategy).

3. **Lint + type-check**: `yarn lint && yarn type-check` from `analytics-web-app/`.

## Files to Modify

- `analytics-web-app/src/lib/screen-renderers/cells/FlameGraphCell.tsx` — the `useEffect` at lines 1001-1028 only.

## Trade-offs

- **Listen on `window` for keyup vs. `document`.** `window` is conventional for keyboard input and matches the `blur` listener target. Either works in practice; `window` is chosen for symmetry.
- **Keep keydown on `container`.** Moving keydown to `window` would let WASD zoom fire when the user is editing SQL in another cell, which is exactly the kind of focus-stealing we're trying to prevent. Container-only keydown preserves the "focus this cell to drive it" model.
- **Use focus/visibility events instead of an idle timeout.** A timeout (e.g., "if no keydown for 200ms, drop the key") would also work and would survive missing `keyup` even within the same focused element. But it adds latency to the existing fast-release path, conflicts with auto-repeat semantics, and adds a tunable that has to be right. The focus/visibility approach is event-driven and free of timing tunables. If real-world reports show keys getting stuck without focus loss, an idle-timeout fallback can be added later.
- **`blur` on `container` may fire frequently.** Calling `clearAllKeys` on container blur is cheap (`Set.clear` + at most one `cancelAnimationFrame`) and is the correct behavior — when the cell loses focus the user no longer expects WASD to drive it.

## Testing Strategy

No existing automated coverage for WASD; jsdom does not run rAF or model focus realistically enough to cover this without a heavy harness. Manual verification:

In Chrome, on a notebook with a flame graph cell:

1. **Baseline**: focus the flame graph, hold W, release — zoom stops. Repeat with A, S, D.
2. **Alt-Tab while held**: focus the flame graph, hold W, Alt-Tab to another window, release the key. Return to the tab — no stale zoom.
3. **Switch tab while held**: hold W, switch browser tabs, release, switch back — no stale zoom.
4. **Click another cell while held**: hold W, click into a SQL editor in another cell, release W — no stale zoom.
5. **Devtools focus while held**: hold W, click into devtools, release W — no stale zoom.
6. **Combined keys**: hold W and D together (zoom in + pan right); release one, then the other; the corresponding action should stop on each release.

Repeat #1 and #6 in Firefox to confirm no regression.

Optional unit test (worth doing if it can stay simple): a Vitest test that mounts `FlameGraphView` against a mocked Three.js stack and asserts that `window.dispatchEvent(new Event('blur'))` empties `keysRef` after a `keydown('w')`. Skip if mocking Three.js is more code than the test is worth — manual verification is sufficient for a focus/event-plumbing fix.

## Documentation

No documentation pages describe WASD zoom today (`mkdocs/docs` does not mention flamegraph keyboard controls). No docs update required for this fix. If a controls reference is added later, it should mention WASD zoom/pan, double-click to reset, and Alt-drag to propagate the time range.

## Open Questions

- None blocking. If manual testing turns up a Chrome scenario where keys still get stuck *without* focus loss, fall back to an idle-timeout in `keyTick` (drop a key if no `keydown` has been seen for it within ~200ms).
