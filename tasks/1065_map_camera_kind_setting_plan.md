# Map Cell: Camera Mode Setting Plan (#1065)

## Issue Reference
- [#1065](https://github.com/madesroches/micromegas/issues/1065) — Map viewer:
  support orthographic camera mode.

## Overview

The map viewer hard-codes a perspective camera. For flat heatmap-style data
(gameplay density binned into cells, scale-Z=0 box overlays) perspective
foreshortening hurts readability. This plan adds a per-Map-cell **Camera**
setting with two values to start — `perspective` (current behavior) and
`orthographic` — and restructures the camera/controller around a
"Mode = (camera, controller)" pairing.

Picking a mode in the dropdown selects a self-contained component that
renders the right drei camera element and a controller typed against that
exact camera class. The controller has direct access to its camera's API
(`fov`, `zoom`, ...) — no casts, no `useThree().camera` race, no shared
abstract `CameraMode` interface to evolve as new modes are added.

**Controls policy.** All existing inputs (left-drag pan, right-drag orbit,
Ctrl-wheel zoom, WASDQE fly, `Z` reset) stay active in every mode. We ship
one orthographic variant with the same controls as perspective and watch
for real usability issues before introducing per-mode control gating.

## Current State

### Where the camera is wired

`analytics-web-app/src/components/map/MapViewer.tsx`:

- Hardcoded `<PerspectiveCamera makeDefault fov={60} near={1} far={100000} />`
  at `MapViewer.tsx:194`, outside the `{ready && ...}` gate so r3f always
  has a default camera registered.
- `<MapCameraController>` rendered inside `{ready && ...}`
  (`MapViewer.tsx:200-207`), reading `useThree().camera` and mutating it
  every frame.
- Threads `glbCamera: THREE.PerspectiveCamera | null` from `MapModel`.

`analytics-web-app/src/components/map/MapCamera.tsx` (~400 lines) is the
controller. The orbit math is camera-agnostic, but three places carry
perspective-specific assumptions:

- **GLB seed effect** (`:113-153`): copies `fov / near / far` off the GLB
  camera; casts `camera as THREE.PerspectiveCamera`.
- **Wheel handler** (`:251-294`): drives `sphericalRef.current.radius`. In
  ortho the projection is distance-invariant — scaling radius doesn't
  change apparent size; the user-perceived zoom lives on `camera.zoom`.
- **Reset-view effect** (`:84-109`): restores orbit refs but captures
  nothing ortho would need (e.g. `camera.zoom`).

Pure helpers in `map-camera-math.ts` (`sphericalToZUpOffset`,
`zUpOffsetToSphericalInput`, `cameraBasisFromSpherical`, `panTarget`,
`zoomAnchorTarget`) operate on plain THREE primitives. One needs a
signature change: `panTarget(target, theta, radius, dx, dy)` bakes in
`panSpeed = radius * 0.001`. The `radius` argument becomes `panSpeed`,
computed by each mode from its own camera state.

### Where the cell config lives

`analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`:

- Reads typed options inline: `shape`, `mapUrl`, `detailTemplate`,
  `showHoverTooltip` (`:260, :292, :298-301`).
- Default config in `createDefaultConfig` (`:989-1001`).
- Editor "Map Options" block renders the map dropdown (`:826-844`);
  "Primitive" renders the shape select (`:847-921`) — the pattern this
  plan follows for the new Camera select.

### Tests / docs

- `analytics-web-app/src/components/map/__tests__/map-camera-math.test.ts`
  — pure helpers; no controller mount.
- `analytics-web-app/src/components/map/__tests__/MapViewer.test.tsx`
  — exercises `cameraBasisFromSpherical`; no Canvas mounting.
- `mkdocs/docs/web-app/notebooks/cell-types.md:216-367` covers the Map
  cell, with a **Camera controls** table at `:336-347`.

## Design

### `MapMode` interface

A `MapMode` is a kind tag plus a React component that mounts the camera
and its controller together. The interface exists only so `MapViewer` can
switch on `cameraKind` and `MapCell`'s editor dropdown can enumerate
choices.

```ts
// modes/types.ts
import type { ComponentType } from 'react'
import type * as THREE from 'three'

export type MapModeKind = 'perspective' | 'orthographic'

export interface MapModeRenderProps {
  // All three are non-nullable: `MapViewer` only mounts a mode when
  // the GLB has arrived *and* includes an embedded camera. `mapScene`,
  // `mapBounds`, and `glbCamera` are set together in `handleMapLoaded`,
  // so the gate checks all three inline (rather than reusing the
  // aliased `ready = mapScene !== null` boolean) — TS narrows each at
  // the call site only when its non-null check is part of the JSX
  // expression. A missing camera is a contract violation that surfaces
  // as a red error banner with no map content rendered.
  glbCamera: THREE.PerspectiveCamera
  mapScene: THREE.Object3D
  mapBounds: THREE.Box3
  resetViewTrigger: number
}

export interface MapMode {
  kind: MapModeKind
  /** Mounts the camera element and its controller as a wired pair. */
  Render: ComponentType<MapModeRenderProps>
}

export const MAP_MODE_LABELS: Record<MapModeKind, string> = {
  perspective: 'Perspective',
  orthographic: 'Orthographic',
}
```

```ts
// modes/index.ts
import { perspectiveMode } from './PerspectiveMode'
import { orthographicMode } from './OrthographicMode'
import type { MapMode, MapModeKind } from './types'

export const MAP_MODES: Record<MapModeKind, MapMode> = {
  perspective: perspectiveMode,
  orthographic: orthographicMode,
}

export const MAP_MODE_KINDS = Object.keys(MAP_MODES) as MapModeKind[]

export { MAP_MODE_LABELS } from './types'
export type { MapMode, MapModeKind, MapModeRenderProps } from './types'
```

`MapCell` resolves `cameraKind` to a valid `MapModeKind` at the boundary
(handling `undefined` for notebooks saved before this option existed) and
passes it into `MapViewer` as a typed prop. `MapViewer` then indexes
`MAP_MODES[cameraKind]` directly — no helper, no defensive
`?? perspectiveMode`. If a malformed string ever sneaks past the
boundary, `MAP_MODES[bad]` is `undefined` and the component fails to
render at that cell: a loud, visible failure beats a silent fallback that
hides notebook corruption.

A future locked-top-down (or isometric) mode adds one file and one entry
in `MAP_MODES` / `MAP_MODE_LABELS`. No edits to `MapViewer`, `MapCell`, or
the existing modes.

### `PerspectiveMode` (current behavior, extracted)

```tsx
// modes/PerspectiveMode.tsx
import { useRef } from 'react'
import { PerspectiveCamera } from '@react-three/drei'
import * as THREE from 'three'
import type { MapMode, MapModeRenderProps } from './types'
import { PerspectiveCameraController } from './PerspectiveCameraController'

function PerspectiveModeRender({
  glbCamera, mapScene, mapBounds, resetViewTrigger,
}: MapModeRenderProps) {
  const cameraRef = useRef<THREE.PerspectiveCamera>(null!)
  return (
    <>
      <PerspectiveCamera ref={cameraRef} makeDefault fov={60} near={1} far={100000} />
      <PerspectiveCameraController
        cameraRef={cameraRef}
        glbCamera={glbCamera}
        mapScene={mapScene}
        mapBounds={mapBounds}
        resetViewTrigger={resetViewTrigger}
      />
    </>
  )
}

export const perspectiveMode: MapMode = {
  kind: 'perspective',
  Render: PerspectiveModeRender,
}
```

`PerspectiveCameraController` takes
`cameraRef: RefObject<THREE.PerspectiveCamera>` — no `useThree().camera`,
no cast. Its body is the current `MapCamera.tsx` with four changes:

- GLB seed reads `cameraRef.current` and writes `fov/near/far` without a
  cast.
- The wheel handler is unchanged (radius-driven zoom) — passed to the hook
  as `onWheel`.
- Pan speed: the actual `panTarget(target, theta, getPanSpeed(), dx, dy)`
  call lives in the hook's `onMouseMove` (see Shared hook below). The
  controller only supplies `getPanSpeed: () => radius * 0.001`, matching
  the new explicit-`panSpeed` signature.
- Owns the perspective-specific `fitRadiusRef` and `zoomFactorRef` (the
  invariant `radius = fitRadius * zoomFactor`). Passes
  `onRightDragReAnchor: () => { zoomFactorRef.current = sphericalRef.current.radius / fitRadiusRef.current }`
  into `useMapOrbitController` so the zoom invariant is preserved after
  the hook's right-drag re-anchor updates `sphericalRef.radius`. The
  callback closes over `sphericalRef` (returned by the hook) — fine
  because it fires at mousedown time, after the destructure has run.

### `OrthographicMode` (new)

```tsx
// modes/OrthographicMode.tsx
import { useRef } from 'react'
import { OrthographicCamera } from '@react-three/drei'
import * as THREE from 'three'
import type { MapMode, MapModeRenderProps } from './types'
import { OrthographicCameraController } from './OrthographicCameraController'

function OrthographicModeRender({
  glbCamera, mapScene, mapBounds, resetViewTrigger,
}: MapModeRenderProps) {
  const cameraRef = useRef<THREE.OrthographicCamera>(null!)
  return (
    <>
      <OrthographicCamera ref={cameraRef} makeDefault near={1} far={100000} />
      <OrthographicCameraController
        cameraRef={cameraRef}
        glbCamera={glbCamera}
        mapScene={mapScene}
        mapBounds={mapBounds}
        resetViewTrigger={resetViewTrigger}
      />
    </>
  )
}

export const orthographicMode: MapMode = {
  kind: 'orthographic',
  Render: OrthographicModeRender,
}
```

Drei's `<OrthographicCamera>` auto-fits `left/right/top/bottom` to the
canvas viewport and re-fits on resize — the controller doesn't manage the
frustum.

`OrthographicCameraController` takes
`cameraRef: RefObject<THREE.OrthographicCamera>` and reads/writes `.zoom`
directly. Diffs from perspective:

- **GLB seed**: copies `near/far` off the GLB camera; computes
  `camera.zoom` so the initial framing visually matches perspective mode.
  With `vFov = glbCamera.fov` (degrees, per THREE) and
  `R = sphericalRef.radius`, the world height visible at distance R in
  perspective is `worldHeight = 2 * R * tan(degToRad(vFov) / 2)`. For the
  ortho camera, `camera.zoom = H_px / worldHeight` matches the height-fit,
  where `H_px = camera.top - camera.bottom` (drei passes these as JSX
  props on the underlying `<orthographicCamera>` primitive, applied
  during commit before any `useLayoutEffect`, so they're populated by
  the time the controller's seed effect runs). Note: pass `degToRad(vFov) / 2` to
  `Math.tan`, **not** `vFov / 2` directly — `Math.tan` expects radians.
  After writing `near/far/zoom`, the seed calls
  `camera.updateProjectionMatrix()` — drei's own `useLayoutEffect` for
  this fires *before* the controller's seed in sibling effect order, and
  drei's `useFrame` is a no-op without functional `children`, so without
  the explicit call the first paint would use the stale default
  projection until something else triggered a re-render. (Mirrors the
  perspective seed, which already calls `updateProjectionMatrix` after
  writing `fov/near/far`.)
  The seed assumes `glbCamera` is present — `MapViewer` only mounts the
  ortho mode when the GLB has an embedded camera (see
  `MapViewer.tsx` changes below); no null-camera branch is needed.
- **Wheel handler**: cursor-anchored on `camera.zoom`. Algorithm:
  1. `m = e.deltaY > 0 ? 1 / (1 + zoomSpeed) : (1 + zoomSpeed)` — matches
     perspective UX where `deltaY > 0` zooms out.
  2. `newZoom = clamp(camera.zoom * m, ZOOM_MIN, ZOOM_MAX)`.
  3. Raycast cursor against `mapScene` → world anchor `a`. Translate
     `target` to keep `a` under the cursor:
     `target = a + (target - a) / m` — same `zoomAnchorTarget` helper
     with `s = 1 / m`.
  4. `camera.zoom = newZoom; camera.updateProjectionMatrix()`.

  Orbit `radius` is **not** changed — camera-to-target distance is
  irrelevant to ortho projection.
- **Pan / fly speed**: computed live from camera intrinsics, not from
  `radius` and not from any captured seed state.
  - Pan: `panSpeed = 1 / camera.zoom` — exact world-per-pixel for ortho,
    since drei's auto-fit makes `top - bottom = H_px`.
  - Fly per frame: `moveSpeed = (camera.top - camera.bottom) / camera.zoom * FLY_SPEED * delta`
    — half the visible world-height per second at `FLY_SPEED = 0.5`,
    same dimensionless ratio perspective uses (`radius * 0.5 * delta`,
    which is half the radius per second).

  Both auto-scale as the user zooms (rising `camera.zoom` shrinks visible
  world, shrinks pan/fly speed proportionally). No `seedZoom`/`radiusAtSeed`
  captured state needed — current camera state is sufficient.
- **Reset (`Z`)**: in addition to restoring orbit refs, restore
  `camera.zoom` from the saved view. Captured at seed time alongside the
  orbit snapshot.
- **Per-frame pose**: identical to perspective —
  `camera.position = target + sphericalToZUpOffset(spherical)`,
  `camera.up` from theta, `camera.lookAt(target)`. Projection-agnostic.

### Shared hook

The two controllers share structure: orbit refs, DOM event wiring, the
per-frame pose write. Extract into a single hook
`useMapOrbitController` under
`analytics-web-app/src/components/map/hooks/useMapOrbitController.ts`.

```ts
function useMapOrbitController<C extends THREE.PerspectiveCamera | THREE.OrthographicCamera>({
  cameraRef,
  mapSceneRef,
  domElement,
  onWheel,
  getPanSpeed,
  getFlyMoveSpeedPerFrame,
  onRightDragReAnchor,
}: {
  cameraRef: RefObject<C>
  mapSceneRef: RefObject<THREE.Object3D>
  domElement: HTMLElement | null
  onWheel: (e: WheelEvent) => void
  getPanSpeed: () => number
  getFlyMoveSpeedPerFrame: (delta: number) => number
  onRightDragReAnchor?: () => void
}): {
  targetRef: RefObject<THREE.Vector3>
  sphericalRef: RefObject<THREE.Spherical>
}
```

Responsibilities, in one place:

- **Orbit refs.** Creates and returns `targetRef`, `sphericalRef` — the
  mode-agnostic state the per-mode GLB-seed and reset effects read/write.
  Perspective-specific zoom-invariant state (`fitRadiusRef`,
  `zoomFactorRef`) lives in `PerspectiveCameraController`, not the hook;
  the hook stays mode-agnostic. Saved-view state also stays with each
  mode controller (perspective stores the orbit snapshot; ortho stores
  the snapshot plus `camera.zoom`).
- **DOM event wiring.** mousedown/mousemove/mouseup, right-drag
  re-anchor raycast, contextmenu suppression, hover gating, keyboard,
  window-blur cleanup. After the right-drag re-anchor updates
  `targetRef` and `sphericalRef`, the hook invokes the optional
  `onRightDragReAnchor` callback so perspective can write its zoom
  invariant (`zoomFactorRef = sphericalRef.radius / fitRadiusRef`);
  ortho omits the callback. `isHoveredRef` and `keysRef` are local refs —
  not returned, since the per-frame loop is also inside this hook.
- **Per-frame pose.** A `useFrame` body that steps the WASD position
  with `getFlyMoveSpeedPerFrame(delta)`, then writes
  `camera.position / up / lookAt` from the orbit refs. Projection-
  agnostic.
- **Latest-callback refs.** Stores `onWheel`, `getPanSpeed`,
  `getFlyMoveSpeedPerFrame`, `onRightDragReAnchor` in internal refs so
  the DOM-binding `useEffect` doesn't rebind listeners every render as
  callback identities change. The `useFrame` callback reads the latest
  from the same refs. The effect is keyed on
  `[cameraRef, domElement, mapSceneRef]` (stable ref objects) and reads
  `cameraRef.current` *inside* the effect body — **not** on
  `[domElement, cameraRef.current]`. On the controller's first render
  `cameraRef.current` is still `null` (the sibling drei camera's ref
  attaches at commit, and nothing forces a follow-up render), so a
  dep-array read would capture `null` and the listeners would never
  bind. Reading inside the effect is correct because React attaches all
  refs in the committed tree before any effect fires, and the camera
  identity is fixed for the controller's mount lifetime (`MapViewer`
  remounts the camera/controller pair on `mapUrl` or `cameraKind`
  change), so binding once is sufficient.

Each per-mode controller is ~80 lines: declare `mapSceneRef` and its own
`savedViewRef`, call `useMapOrbitController` with mode-specific
callbacks, declare the GLB-seed `useLayoutEffect` (writing the orbit
refs returned by the hook and the snapshot into `savedViewRef`), declare
the reset-view `useEffect` (reading `savedViewRef` and restoring both
orbit refs and any mode-specific fields like `camera.zoom`). The
`useLayoutEffect` ordering — hook's effects + mode's seed in declaration
order, all before the first `useFrame` tick — keeps initial framing
correct.

### `MapViewer.tsx` changes

- Add prop `cameraKind: MapModeKind`.
- Resolve `const mode = MAP_MODES[cameraKind]` — a stable module-level
  reference, so no `useMemo` is needed. No helper, no fallback: the prop
  type guarantees the lookup; if a malformed value ever bypasses the
  `MapCell` boundary, the cell fails loudly rather than silently
  rendering perspective.
- Keep the unconditional `<PerspectiveCamera makeDefault fov={60} near={1} far={100000} />`
  at line 194 as r3f's always-registered default camera. It exists only
  so r3f never reports "no default camera" during the not-ready window
  or while a non-conforming GLB is on screen; it doesn't drive the
  user-facing render once a mode is mounted. Update its comment
  (`MapViewer.tsx:187-194`): the current text — "FOV/near/far are the
  seed for GLB-cameraless contracts; the GLB-camera effect copies
  intrinsics onto this camera" — is stale once intrinsics are seeded
  onto the mode's own camera and a non-conforming GLB renders nothing.
- Gate the camera/controller/markers on a conforming GLB: replace the
  current `{ready && ...}` block with an inline triple-check
  `{mapScene !== null && mapBounds !== null && glbCamera !== null && ...}`,
  containing `<mode.Render key={mapUrl} glbCamera={glbCamera} mapScene={mapScene} mapBounds={mapBounds} .../>`,
  the optional `<ambientLight>`, and `<MapInstancedMarkers>`. The
  inline checks (rather than reusing the aliased `ready` boolean) are
  what let TS narrow `mapScene`, `mapBounds`, and `glbCamera` to
  non-null at the `<mode.Render>` call site — TS 5.4 aliased-condition
  narrowing covers `mapScene` through `ready`, but `mapBounds` is
  independent state and only narrows through an inline check. With
  the triple-check, the `ready` local in `MapViewer` is no longer
  used and can be removed. A non-conforming GLB (no embedded camera)
  is a hard contract failure: nothing mounts, the existing red
  `contractErrors` banner is the entire failure UI. No silent
  fallback, no degraded "default camera" experience. The
  `key={mapUrl}` ensures a map switch fully remounts the camera/
  controller pair and the new GLB gets a fresh seed.
- Because `<mode.Render>` only mounts when all three of `mapScene`,
  `mapBounds`, `glbCamera` are non-null, the corresponding
  `MapModeRenderProps` fields are non-nullable (see `MapMode`
  interface above). Per-mode controllers do not branch on any of them
  being null.
- Tighten the missing-GLB-camera `contractErrors` push at
  `MapViewer.tsx:128-129`. The current message — "No perspective
  camera in GLB — initial framing is the default seed, and Reset View
  will not work." — is no longer accurate (we don't render the map at
  all in this case). Replace with: "No camera in GLB — map cannot be
  rendered." `handleMapLoaded` and `contractErrors` stay
  mode-agnostic.

This is a deliberate behavior change for non-conforming GLBs. Today the
controller mounts against the default `PerspectiveCamera`, markers
render against an arbitrary world-origin view, and Reset View silently
no-ops. After this change the canvas shows only the dark background
plus the red contract-error banner — a clear failure state that can't
be mistaken for "working but framed weirdly."

**Defenses the gate makes dead.** Because the controllers only mount
after `{ready && glbCamera}`, several null-guards in the current
`MapCamera.tsx` become unreachable and are dropped in the per-mode
controllers and the shared hook — no per-controller restatement:

- GLB seed drops the `mapBounds` null fallback (current
  `MapCamera.tsx:122-127`'s `else` branch using
  `Math.max(cameraPos.length(), 1000)`). The gate guarantees
  `mapBounds` is set when the controller mounts (`mapScene`,
  `mapBounds`, and `glbCamera` are set together in
  `handleMapLoaded`).
- GLB seed drops the `seededGlbCameraRef` re-seed guard (current
  `MapCamera.tsx:112-115`). With `key={mapUrl}` the controller
  remounts per URL, so `glbCamera` identity is fixed for the entire
  mount lifetime — the effect runs once and no guard is needed.
- Reset effect drops the `initialViewRef.current` null check (current
  `MapCamera.tsx:86`). The seed runs synchronously at mount via
  `useLayoutEffect`, so the snapshot is populated before any
  `resetViewTrigger` change can fire.
- Shared hook types `mapSceneRef` as `RefObject<THREE.Object3D>` (see
  hook signature below); wheel and right-drag re-anchor handlers drop
  their `if (scene)` guards.

A `cameraKind` change makes React swap component types in `<mode.Render>`
— the prior camera/controller unmount cleanly, listeners are removed by
the existing cleanup, the new mode mounts with fresh refs and re-seeds
from the GLB. No `key={cameraKind}` workaround needed.

### `MapCell.tsx` changes

- In the renderer scope, alongside the other option reads
  (`MapCell.tsx:298-301`):
  ```ts
  const cameraKind = (options?.cameraKind as MapModeKind | undefined) ?? 'perspective'
  ```
  Pass `cameraKind={cameraKind}` to `<MapViewer>`.
- In `MapCellEditor`, alongside the other editor-scope option reads
  (`:784-787`):
  ```ts
  const cameraKind = (mapConfig.options?.cameraKind as MapModeKind | undefined) ?? 'perspective'
  ```
  Add a **Camera** row in the "Map Options" section (`:826-844`), modeled
  after the Shape select:
  ```tsx
  <div className="flex items-center gap-2">
    <label className="text-xs text-theme-text-secondary w-24 shrink-0">Camera</label>
    <select
      value={cameraKind}
      onChange={(e) => updateOption('cameraKind', e.target.value)}
      className="bg-app-card border border-theme-border rounded px-2 py-1 text-sm text-theme-text-primary focus:outline-none focus:border-accent-link"
    >
      {MAP_MODE_KINDS.map((kind) => (
        <option key={kind} value={kind}>{MAP_MODE_LABELS[kind]}</option>
      ))}
    </select>
  </div>
  ```
- Leave `createDefaultConfig` (`:989-1001`) unchanged — saved notebooks
  and new cells default to `'perspective'` when `cameraKind` is absent.

### `map-camera-math.ts` changes

`panTarget`'s `radius` parameter is replaced by `panSpeed`:

```ts
// Before
export function panTarget(target, theta, radius, deltaX, deltaY) {
  const panSpeed = radius * 0.001
  // ...
}

// After
export function panTarget(target, theta, panSpeed, deltaX, deltaY) {
  // ...
}
```

Callers pass `radius * 0.001` (perspective) or `1 / camera.zoom` (ortho).
The doc comment loses the "drives pan speed" note on the old `radius`
parameter and gains one on `panSpeed` instead.

## File Layout

```
analytics-web-app/src/components/map/
  modes/
    types.ts                            // MapMode, MapModeKind, MapModeRenderProps, MAP_MODE_LABELS
    PerspectiveMode.tsx                 // <PerspectiveCamera> + PerspectiveCameraController
    OrthographicMode.tsx                // <OrthographicCamera> + OrthographicCameraController
    PerspectiveCameraController.tsx     // typed against THREE.PerspectiveCamera
    OrthographicCameraController.tsx    // typed against THREE.OrthographicCamera
    index.ts                            // MAP_MODES, MAP_MODE_KINDS, MAP_MODE_LABELS
  hooks/
    useMapOrbitController.ts
  MapViewer.tsx                         // selects mode, renders <mode.Render>
  map-camera-math.ts                    // panTarget signature update
  MapCamera.tsx                         // DELETED
```

`MapCamera.tsx` goes away — its logic is split between hooks and the two
per-mode controllers. Only `MapViewer.tsx:6` imports it today.

## Implementation Steps

1. **Scaffold the modes module.** Create `modes/types.ts` and
   `modes/index.ts` registering only `perspectiveMode` initially. No
   behavior change yet.
2. **Extract the shared hook.** Move orbit-state setup, DOM event
   wiring, and per-frame pose from `MapCamera.tsx` into
   `useMapOrbitController.ts`. The hook accepts callbacks for the
   mode-specific bits (`onWheel`, `getPanSpeed`,
   `getFlyMoveSpeedPerFrame`, `onRightDragReAnchor`) and a
   `cameraRef: RefObject<THREE.PerspectiveCamera>`. Perspective-specific
   refs (`fitRadiusRef`, `zoomFactorRef`) stay in the wrapper, which
   passes `onRightDragReAnchor` to write the zoom invariant after the
   hook's right-drag re-anchor updates target/spherical. Attach a `ref`
   to the JSX `<PerspectiveCamera>` in `MapViewer.tsx:194` and thread
   it into `MapCamera` as a new prop; `MapCamera.tsx` remains,
   refactored into a thin wrapper that calls the hook with that
   `cameraRef` and perspective-specific callbacks — verifies the
   extraction is behavior-preserving. Tests still pass against
   `MapCamera`.
3. **Build `PerspectiveCameraController` + `PerspectiveMode`.** Move the
   thin wrapper into `modes/PerspectiveCameraController.tsx`, retyping
   `cameraRef: RefObject<THREE.PerspectiveCamera>`. Build
   `modes/PerspectiveMode.tsx`. Delete `MapCamera.tsx`. Update
   `MapViewer.tsx`:
   - Add `cameraKind` prop (still defaulted to `'perspective'` until
     Step 6 wires it through `MapCell`).
   - Resolve `mode` via `MAP_MODES[cameraKind]`.
   - Keep the line-194 `<PerspectiveCamera>` as r3f's always-registered
     default. Replace the existing `{ready && ...}` block with an
     inline triple-check
     `{mapScene !== null && mapBounds !== null && glbCamera !== null && ...}`;
     replace `<MapCameraController .../>` inside it with
     `<mode.Render key={mapUrl} glbCamera={glbCamera} mapScene={mapScene} mapBounds={mapBounds} .../>`.
     `<ambientLight>` and `<MapInstancedMarkers>` move under the same
     stricter gate. Remove the now-unused `ready` local.
   - Tighten the contractError message at `MapViewer.tsx:128-129` to
     "No camera in GLB — map cannot be rendered."
   For conforming GLBs (the common case), behavior is preserved
   end-to-end. For non-conforming GLBs, the contract violation is now
   a hard bail: markers and controller no longer mount, only the red
   error banner shows over a dark canvas (see `MapViewer.tsx`
   changes).
4. **Update `panTarget`.** Change the signature in `map-camera-math.ts`
   and the test. The `panTarget` call itself lives in the hook's
   `onMouseMove` as `panTarget(target, theta, getPanSpeed(), dx, dy)`;
   update the perspective controller's `getPanSpeed` to return
   `radius * 0.001` explicitly (it was returning the raw `radius` while
   the old `panTarget` applied the `* 0.001` internally).
5. **Add `OrthographicMode` + controller.** Widen
   `useMapOrbitController`'s `cameraRef` from
   `RefObject<THREE.PerspectiveCamera>` (introduced in Step 2) to
   `RefObject<C>` where `C extends THREE.PerspectiveCamera | THREE.OrthographicCamera`,
   matching the generic signature in the Design section. Build
   `modes/OrthographicCameraController.tsx` (GLB seed, cursor-anchored
   wheel, live-from-camera speed math, savedZoom restore on `Z`) and
   `modes/OrthographicMode.tsx`. Register in `MAP_MODES` and label it
   in `MAP_MODE_LABELS`.
6. **Wire `MapCell`.** Read `options.cameraKind`, thread it to
   `<MapViewer>`. Add the **Camera** select in the editor.
7. **Tests.** See Testing Strategy.
8. **Docs.** Update `mkdocs/docs/web-app/notebooks/cell-types.md`.

## Files to Modify

- `analytics-web-app/src/components/map/modes/types.ts` (new)
- `analytics-web-app/src/components/map/modes/PerspectiveMode.tsx` (new)
- `analytics-web-app/src/components/map/modes/OrthographicMode.tsx` (new)
- `analytics-web-app/src/components/map/modes/PerspectiveCameraController.tsx` (new)
- `analytics-web-app/src/components/map/modes/OrthographicCameraController.tsx` (new)
- `analytics-web-app/src/components/map/modes/index.ts` (new)
- `analytics-web-app/src/components/map/hooks/useMapOrbitController.ts` (new)
- `analytics-web-app/src/components/map/MapCamera.tsx` (deleted)
- `analytics-web-app/src/components/map/MapViewer.tsx`
- `analytics-web-app/src/components/map/map-camera-math.ts`
- `analytics-web-app/src/components/map/__tests__/map-camera-math.test.ts`
- `analytics-web-app/src/components/map/__tests__/orthographic-mode.test.ts` (new)
- `analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`
- `mkdocs/docs/web-app/notebooks/cell-types.md`

## Trade-offs

- **Mode owns both vs. one controller + adapter.** Considered an adapter
  pattern where one shared controller takes a `CameraMode` object with
  `seed/zoom/snapshot/effectiveRadius/...` methods. The divergences
  between modes (camera API, seed math, zoom mutation target, snapshot
  fields, speed quantity) are deep enough that the adapter ended up with
  leaky abstractions — notably an `effectiveRadius` that returned
  perspective radius for one mode and a synthetic
  `radiusAtSeed * seedZoom / camera.zoom` for the other, with the
  controller forced to receive `THREE.Camera` and cast at every usage
  site. Mode-owns-both produces self-contained typed controllers, no
  casts, no shared interface to maintain. The interface that does exist
  (`MapMode`) is just `{ kind, Render }` — minimal and stable.
- **Per-mode controllers vs. one branched controller.** Considered a
  single controller with `if (cameraKind === ...)` at each divergent
  site. Smaller in lines but reopens the controller for every new mode
  and forces both modes' state to coexist. Per-mode controllers are
  larger but each reads end-to-end for one mode.
- **Ortho speed math from live camera state vs. captured seed state.**
  Considered storing `seedZoom` and `radiusAtSeed` at seed time and
  scaling pan/fly speeds by their ratio against the perspective
  baseline. Simpler: compute `panSpeed = 1 / camera.zoom` and
  `flySpeed = (top - bottom) / zoom * 0.5` directly from current camera
  intrinsics — both auto-scale correctly as the user zooms, no captured
  state needed. The numbers come within ~15% of perspective at the
  seeded framing (close enough to feel consistent across modes).
- **Mode swap loses orbit framing.** A `perspective` ↔ `orthographic`
  swap re-seeds from the GLB; target/spherical are not preserved.
  Considered lifting orbit state to `MapViewer` so it survives the
  swap, but: mode swaps are rare (users pick a projection for the
  visualization and stay there), the swap is a deliberate UI action
  with a reframe expectation, and preserving framing across a
  projection change can look broken anyway (the same `radius` produces
  very different on-screen size in the two modes). Re-seeding is the
  simpler contract and not worth optimizing.
- **Drei `<OrthographicCamera>` vs. raw THREE.** Drei auto-fits
  `left/right/top/bottom` to the canvas viewport and reacts to resize,
  saving a manual resize observer. Already a dependency for
  `<PerspectiveCamera>` and `useGLTF`.
- **`panTarget` signature change.** Considered keeping `radius` and
  adding an opt-in override. Two ways to call the same function is the
  worst of both worlds. Replacing the parameter is one line per caller.
- **One ortho variant with full controls vs. a locked top-down mode.**
  Issue floats a locked top-down (no phi/theta, pan + zoom only) as the
  "right default for heatmaps." Shipping one ortho variant with full
  controls — nothing forbids a user who only wants pan+zoom from
  right-dragging to `phi=0` once and using only left-drag and
  Ctrl-wheel afterwards (the controller already allows `phi=0`). A
  locked variant would only prevent accidental re-tilts — a small UX
  nicety, not a feature gap. If real usability data ever justifies it,
  the mode-as-component shape makes it a third file + one registry
  line.

## Documentation

`mkdocs/docs/web-app/notebooks/cell-types.md`:

- **Options table** (around `:239-244`): add a `cameraKind` row.
  ```
  | `cameraKind` | `'perspective'` \| `'orthographic'` | `'perspective'` | Camera projection. Orthographic removes perspective foreshortening — better for flat heatmap-style data. Controls are identical in both modes. |
  ```
- **Camera controls** table (`:336-347`): no row changes — all controls
  work identically. Add one sentence above the table noting that the
  same controls apply in both perspective and orthographic modes.

## Testing Strategy

- **`orthographic-mode.test.ts`** (new) — covers ortho-specific math
  without mounting r3f. The seed and zoom-anchor functions live as pure
  helpers inside `OrthographicCameraController.tsx` (exported for
  testing) so tests can call them with fake camera objects.
  - **Seed zoom from GLB.** Fake `glbCamera = { fov, near, far }`, fake
    ortho camera with known `top - bottom = H_px`, call
    `computeOrthoSeedZoom(glbCamera, radius, H_px)`. Assert
    `zoom ≈ H_px / (2 * radius * tan(degToRad(fov)/2))`.
  - **Wheel zoom anchor stability.** With hand-built
    `target / anchor / camera.zoom`, apply the ortho zoom step. Assert
    the anchor's world point projects to the same NDC coordinate before
    and after, within epsilon. Uses `updateProjectionMatrix` + `project`
    on the fake ortho camera; no DOM needed (synthetic rect).
- **`map-camera-math.test.ts`** — update existing `panTarget` calls to
  the new `panSpeed` signature. Assertions are unchanged in spirit; the
  test inputs swap `radius` for `radius * 0.001`. Rename
  `it('scales pan speed with the orbit radius', …)` to reflect the new
  parameter (e.g. `'scales translation linearly with panSpeed'`).
- **`MapViewer.test.tsx`** — unchanged. Existing assertions exercise
  pure helpers, not Canvas mounting.
- **Full local suite**: `yarn lint`, `yarn type-check`, `yarn test` from
  `analytics-web-app/`.
- **Manual checks**:
  - Switch between modes in the editor; view re-seeds cleanly, no
    console errors, cursor isn't stuck.
  - In ortho: left-drag pans, right-drag orbits, Ctrl-wheel
    cursor-anchored zoom feels right, WASDQE flies, `Z` resets to
    seeded view including the seeded `camera.zoom`.
  - In perspective: behavior matches pre-change (regression).
  - Two Map cells on the same page with different `cameraKind` values
    render independently.
  - GLB without an embedded camera: red `contractErrors` banner shows
    over an empty dark canvas (no markers, no orbit) in both
    `perspective` and `orthographic` modes — the contract violation
    bails the render.
  - **Usability watch (no acceptance gate):** does right-drag orbit in
    ortho feel disorienting? Does fly-speed feel wrong after extreme
    zoom-in or zoom-out? Notes feed into the decision on a locked
    top-down sub-variant or fly-speed tweak.

## Open Questions

None.
