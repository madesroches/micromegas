# Map Cell: Camera Kind Setting Plan (#1065)

## Issue Reference
- [#1065](https://github.com/madesroches/micromegas/issues/1065) — Map viewer:
  support orthographic camera mode.

## Overview

The map viewer hard-codes a perspective camera and an orbit-style interaction.
For flat heatmap-style data (gameplay density binned into cells) the
foreshortening hurts readability. This plan adds a Map-cell **Camera** setting
with two values to start — `perspective` (current behavior) and `orthographic`
(same orbit/pan/fly controls, just a different projection) — and restructures
the controller so each new camera variant is a self-contained adapter behind
a small interface. Adding a future locked-top-down or isometric mode becomes a
new file in `camera-modes/` plus one registry entry; no edits to
`MapCamera.tsx`, `MapViewer.tsx`, or `MapCell.tsx` (OCP).

**Controls policy.** All existing inputs (left-drag pan, right-drag orbit,
Ctrl-wheel zoom, WASDQE, `Z` reset) stay active in every mode for now. We
ship one orthographic variant with the same controls as perspective and
watch for real usability issues before introducing per-mode control gating
(e.g. a locked top-down sub-variant). Keeping the controls uniform also
keeps the adapter interface smaller.

## Current State

### Where the camera is wired

`analytics-web-app/src/components/map/MapViewer.tsx`:

- Hardcoded `<PerspectiveCamera makeDefault fov={60} near={1} far={100000} />`
  inside `<Canvas>` (`MapViewer.tsx:194`).
- `<MapCameraController>` reads `useThree().camera`, mutates it every frame.
- Threads `glbCamera: THREE.PerspectiveCamera | null` from `MapModel` into the
  controller (`:200-207`).

`analytics-web-app/src/components/map/MapCamera.tsx` is the single ~400-line
controller. The orbit math is camera-agnostic (target + Z-up spherical), but
three places reach in with perspective-specific assumptions:

- **GLB seed effect** (`MapCamera.tsx:113-153`): copies `fov / near / far`
  off the GLB perspective camera onto the scene camera. Casts
  `camera as THREE.PerspectiveCamera`.
- **Wheel handler** (`:251-294`): drives `sphericalRef.current.radius` via a
  `zoomFactor`. For an ortho camera, the projection is distance-invariant —
  scaling radius changes only the orbit point distance, not the apparent size.
  The user-perceived zoom for ortho lives on `camera.zoom`.
- **Reset-view effect** (`:84-109`) and **`useFrame` placement** (`:385-411`):
  set `camera.position`, `camera.up`, then `camera.lookAt(target)` —
  projection-agnostic, but the saved snapshot omits anything the ortho variant
  would need to restore (e.g., `camera.zoom`).

Pure helpers in `map-camera-math.ts` (`sphericalToZUpOffset`,
`zUpOffsetToSphericalInput`, `cameraBasisFromSpherical`, `panTarget`,
`zoomAnchorTarget`) take only THREE vector primitives — already adapter-ready.

### Where the cell config lives

`analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`:

- Reads typed options inline: `shape`, `mapUrl`, `detailTemplate`,
  `showHoverTooltip` (`:260, :292, :298-301`).
- Default config in `createDefaultConfig` (`:989-1001`).
- Editor block "Map Options" renders the map dropdown (`:832-844`); "Primitive"
  renders the shape select + bindings (`:847-921`). The shape select is the
  pattern this plan follows for the new camera select.

### Tests / docs

- `analytics-web-app/src/components/map/__tests__/map-camera-math.test.ts` —
  pure helpers; no controller mount.
- `analytics-web-app/src/components/map/__tests__/MapViewer.test.tsx` —
  smoke-level.
- Docs: `mkdocs/docs/web-app/notebooks/cell-types.md:216-348` — the Map cell
  section, including the **Camera controls** table at `:336-348`.

## Design

### Camera-mode adapter (the OCP seam)

A `CameraMode` describes everything that varies between camera variants. The
controller and viewer talk only to this interface; concrete adapters
encapsulate the perspective vs. orthographic differences.

```ts
// camera-modes/types.ts
import type { ComponentType } from 'react'
import type * as THREE from 'three'

export type CameraKind = 'perspective' | 'orthographic'

export interface CameraOrbitState {
  target: THREE.Vector3
  spherical: THREE.Spherical
  fitRadius: number
  zoomFactor: number
}

export interface SeedParams {
  camera: THREE.Camera
  glbCamera: THREE.PerspectiveCamera | null
  mapBounds: THREE.Box3 | null
  orbit: CameraOrbitState  // mutated in place
}

export interface ApplyParams {
  camera: THREE.Camera
  orbit: CameraOrbitState
}

export interface ZoomParams {
  camera: THREE.Camera
  orbit: CameraOrbitState  // mutated
  domElement: HTMLCanvasElement
  scene: THREE.Object3D | null
  event: WheelEvent
}

export interface CameraMode {
  kind: CameraKind
  /** Drei camera element rendered inside <Canvas> with makeDefault. */
  CameraElement: ComponentType
  /** Seed orbit + intrinsics from the GLB perspective camera. */
  seed: (params: SeedParams) => void
  /** Per-frame: write camera.position / .up / projection state. */
  apply: (params: ApplyParams) => void
  /** Ctrl-wheel zoom step (mutates orbit and/or camera.zoom). */
  zoom: (params: ZoomParams) => void
  /** Saved-view extras serialized alongside orbit on Z reset. */
  snapshot: (camera: THREE.Camera) => unknown
  restore: (camera: THREE.Camera, snap: unknown) => void
}
```

`CameraOrbitState` is what `MapCameraController` already maintains in refs —
the adapter mutates these refs through the typed surface so the controller has
no `if (kind === ...)` branches.

### Registry

```ts
// camera-modes/index.ts
import { perspectiveOrbit } from './perspectiveOrbit'
import { orthographic } from './orthographic'
import type { CameraKind, CameraMode } from './types'

export const CAMERA_MODES: Record<CameraKind, CameraMode> = {
  'perspective': perspectiveOrbit,
  'orthographic': orthographic,
}

export function getCameraMode(kind: CameraKind | undefined): CameraMode {
  return CAMERA_MODES[kind ?? 'perspective'] ?? perspectiveOrbit
}

export type { CameraKind, CameraMode } from './types'
```

This is the only file that names every concrete adapter. Adding a third mode
is one import + one map entry.

### Adapter: `perspectiveOrbit` (current behavior, reorganized)

`camera-modes/perspectiveOrbit.ts`:

- `CameraElement` is `<PerspectiveCamera makeDefault fov={60} near={1} far={100000} />`
  (the exact element currently in `MapViewer.tsx:194`).
- `seed`: copies `fov / near / far` from `glbCamera`, calls
  `updateProjectionMatrix()`. Same body as `MapCamera.tsx:140-144` today.
- `apply`: writes `camera.position = target + sphericalToZUpOffset(spherical)`;
  sets `camera.up` from theta; calls `lookAt(target)`. Same as the controller's
  current `useFrame` body.
- `zoom`: the existing radius-driven cursor-anchored zoom (`MapCamera.tsx:257-293`),
  moved verbatim — multiplies `zoomFactor`, scales `target` toward the
  raycast-hit anchor by `s = newRadius / oldRadius`.
- `snapshot` / `restore`: empty `{}` — orbit state is enough to restore
  perspective view.

### Adapter: `orthographic` (new)

`camera-modes/orthographic.ts`. Same orbit/pan/fly controls as perspective —
only the projection differs.

- `CameraElement`: `<OrthographicCamera makeDefault near={1} far={100000} />`.
  Drei's `<OrthographicCamera>` auto-fits `left/right/top/bottom` to the
  canvas viewport and re-fits on resize — so we don't manage the frustum
  manually. Initial `zoom={1}` (the seed will overwrite it).
- `seed`: derives an initial `camera.zoom` from the GLB perspective frustum at
  the seeded radius so the initial framing visually matches the perspective
  mode. With `vFov = glbCamera.fov` and `R = sphericalRef.radius`, the world
  height visible at distance R in perspective is `2 * R * tan(vFov/2)`. For
  the ortho camera with viewport height `H_px`, `camera.zoom = H_px /
  worldHeight` gives the same height-fit. `near` / `far` are copied off the
  GLB camera (same approach as the perspective adapter) so depth clipping
  behaves identically. The orbit state — `target`, `spherical (radius, phi,
  theta)`, `fitRadius`, `zoomFactor` — is seeded by the same code path as
  perspective, so right-drag orbit and `Z` reset behave the same.
- `apply`: `camera.position = target + sphericalToZUpOffset(spherical)`;
  `camera.up` from theta; `lookAt(target)`. Identical to perspective —
  projection-agnostic.
- `zoom`: cursor-anchored on `camera.zoom`. Algorithm:
  1. `m = e.deltaY > 0 ? 1 / (1 + zoomSpeed) : (1 + zoomSpeed)` (matches the
     perspective UX where `deltaY > 0` zooms out).
  2. `newZoom = clamp(camera.zoom * m, ZOOM_MIN, ZOOM_MAX)`.
  3. Raycast the cursor against `mapScene` → world anchor `a`. Translate
     `target` so `a` stays under the cursor: `target = a + (target - a) / m`
     — same `zoomAnchorTarget(target, anchor, 1 / m)` helper, with `s = 1/m`.
  4. `camera.zoom = newZoom; camera.updateProjectionMatrix()`.
  Note: the orbit `radius` is **not** changed by ortho zoom — the camera-to-
  target distance is irrelevant to ortho projection. WASD speed scales off
  `radius * SPEED_PER_RADIUS` (`MapCamera.tsx:46-47`), so fly-speed stays
  bound to the seeded radius, which is fine for a first cut. If fly-speed
  ends up feeling wrong in ortho, switching it to scale off the visible
  world-height (`H_px / camera.zoom`) is a one-line follow-up.
- `snapshot: (camera) => ({ zoom: (camera as THREE.OrthographicCamera).zoom })`.
- `restore: (camera, snap)` → write `camera.zoom` and
  `updateProjectionMatrix()`.

### Controller changes (`MapCamera.tsx`)

Take a `cameraMode: CameraMode` prop. Replace the three perspective-specific
sites with adapter calls:

| Today | After |
|---|---|
| `MapCamera.tsx:140-144` perspective intrinsic copy | `cameraMode.seed({ camera, glbCamera, mapBounds, orbit })` |
| `MapCamera.tsx:251-294` radius-driven wheel zoom body | `cameraMode.zoom({ camera, orbit, domElement, scene, event })` |
| `MapCamera.tsx:385-411` useFrame position/lookAt body | `cameraMode.apply({ camera, orbit })` |

`saveInitialView` / reset-view effect:
- Extend `initialViewRef` to `{ orbit, modeSnapshot: unknown }` where
  `modeSnapshot = cameraMode.snapshot(camera)`.
- On reset, after restoring orbit refs, call
  `cameraMode.restore(camera, initialViewRef.current.modeSnapshot)`.

All input handlers — left-drag pan, right-drag orbit/re-anchor, Ctrl-wheel
zoom, WASDQE fly, `Z` reset — stay as-is. The wheel handler is the only one
whose body changes (it delegates to `cameraMode.zoom`); the rest are
projection-agnostic and identical across modes.

### `MapViewer.tsx` changes

- Add prop `cameraKind: CameraKind`.
- Resolve `const mode = getCameraMode(cameraKind)` once.
- Render `<mode.CameraElement />` instead of the hardcoded `<PerspectiveCamera>`
  at `MapViewer.tsx:194`.
- Pass `cameraMode={mode}` to `<MapCameraController>`.
- **Mode-swap remount.** Add `key={cameraKind}` on the inner subtree that owns
  the default camera and controller (the `<mode.CameraElement>` and the
  ready-gated block). Swapping `makeDefault` cameras mid-life leaves stale
  refs in the controller (it captured `useThree().camera` at mount); a key
  forces a clean teardown + reseed when the user changes the dropdown. The
  outer `<Canvas>` and `MapModel` are *not* keyed — the GLB stays loaded
  across mode changes.

### `MapCell.tsx` changes

- Read `cameraKind`:
  ```ts
  const cameraKind = (options?.cameraKind as CameraKind | undefined) ?? 'perspective'
  ```
- Pass `cameraKind={cameraKind}` to `<MapViewer>`.
- Editor: new "Camera" row in the existing **Map Options** section
  (`MapCell.tsx:826-844`), modeled after the **Shape** select (`:852-862`).
  ```tsx
  <div className="flex items-center gap-2">
    <label className="text-xs text-theme-text-secondary w-24 shrink-0">Camera</label>
    <select
      value={cameraKind}
      onChange={(e) => updateOption('cameraKind', e.target.value)}
      className="bg-app-card border border-theme-border rounded px-2 py-1 text-sm text-theme-text-primary focus:outline-none focus:border-accent-link"
    >
      <option value="perspective">Perspective</option>
      <option value="orthographic">Orthographic</option>
    </select>
  </div>
  ```
  The options are populated from `Object.keys(CAMERA_MODES)` with a
  human-label lookup so a new adapter appears automatically in the dropdown
  (OCP at the UI level too).
- `createDefaultConfig` (`MapCell.tsx:989-1001`) — leave `cameraKind` out so
  existing notebooks default to `'perspective'`. New notebooks also get
  perspective (matches current behavior).

### File layout

```
analytics-web-app/src/components/map/
  camera-modes/
    types.ts                 // CameraKind, CameraMode, params
    perspectiveOrbit.ts      // current behavior, extracted
    orthographic.ts   // new
    labels.ts                // CameraKind → human label (single source for editor select)
    index.ts                 // CAMERA_MODES registry + getCameraMode
  MapCamera.tsx              // consumes CameraMode prop
  MapViewer.tsx              // selects CameraElement, passes mode to controller
  map-camera-math.ts         // unchanged (already adapter-agnostic)
```

### How a future mode plugs in (the OCP demonstration)

Adding a hypothetical `orthographic-top-down-locked` mode (phi=0, no orbit —
the "pure 2D heatmap" feel) once we know we want it:

1. New file `camera-modes/orthographicTopDownLocked.ts` exporting a
   `CameraMode`. `apply` writes `spherical.phi = 0` before computing the
   offset (and optionally `spherical.theta = 0`); `zoom` is the ortho
   variant; `seed` is the ortho variant with a phi-lock. If a control needs
   to be disabled for this mode, that's the moment to add a flag to
   `CameraMode` (e.g. `allowsOrbit`) and gate the right-drag handler — the
   interface evolves only when an actual mode forces it.
2. Add `'orthographic-top-down-locked': orthographicTopDownLocked` to
   `CAMERA_MODES`.
3. Add a label entry in `labels.ts`.

`MapViewer.tsx` and `MapCell.tsx` don't change at all. `MapCamera.tsx` only
changes if a new control-gating flag is added.

## Implementation Steps

1. **Scaffold the adapter module.** New `camera-modes/types.ts`,
   `camera-modes/labels.ts`, `camera-modes/index.ts` (with only
   `perspectiveOrbit` registered initially).
2. **Extract `perspectiveOrbit`.** Move the perspective intrinsic-copy
   (`MapCamera.tsx:140-144`), the wheel handler body (`:251-294`), and the
   useFrame body (`:385-411`) into `camera-modes/perspectiveOrbit.ts`,
   preserving every line of behavior. Replace those sites in `MapCamera.tsx`
   with adapter calls. Wire `key={cameraKind}` in `MapViewer.tsx`. Tests
   should still pass — this step is a pure refactor.
3. **Add `orthographic.ts`.** Implement `CameraElement` (drei
   `<OrthographicCamera>`), `seed` (derive `zoom` from GLB frustum, copy
   `near`/`far`, share orbit-state seed with perspective), `apply` (identical
   to perspective), `zoom` (cursor-anchored on `camera.zoom`), `snapshot` /
   `restore`. Register in `index.ts` and `labels.ts`.
4. **MapCell wiring.** Read `options.cameraKind`, thread to `<MapViewer>`,
   add the **Camera** select in the editor.
5. **Tests.** See Testing Strategy below.
6. **Docs.** Update `mkdocs/docs/web-app/notebooks/cell-types.md` Map section.

## Files to Modify

- `analytics-web-app/src/components/map/camera-modes/types.ts` (new)
- `analytics-web-app/src/components/map/camera-modes/perspectiveOrbit.ts` (new)
- `analytics-web-app/src/components/map/camera-modes/orthographic.ts` (new)
- `analytics-web-app/src/components/map/camera-modes/labels.ts` (new)
- `analytics-web-app/src/components/map/camera-modes/index.ts` (new)
- `analytics-web-app/src/components/map/MapCamera.tsx`
- `analytics-web-app/src/components/map/MapViewer.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`
- `analytics-web-app/src/components/map/__tests__/camera-modes.test.ts` (new)
- `mkdocs/docs/web-app/notebooks/cell-types.md`

## Trade-offs

- **Adapter object vs. discriminated union with `switch`.** A `switch
  (kind)` inside `MapCamera.tsx` would work for two cases but each new mode
  reopens the controller — the very OCP violation we're trying to avoid. The
  adapter object closes the controller; the registry is the single place
  modes are enumerated.
- **Adapter mutates `orbit` in place vs. returning a new state.** Mutation
  matches the existing controller idiom (everything is a ref). A pure-return
  shape would force the controller to write the return back into refs, which
  is just ceremony given there's exactly one consumer.
- **Mode swap remount via `key={cameraKind}` vs. preserving controller
  state across switches.** Remount is the simpler correctness guarantee:
  `makeDefault` flips, the controller re-runs its seed effect, and saved-view
  refs are fresh. Preserving state would require manually tearing down DOM
  listeners and re-seeding from the new camera type — extra complexity for a
  user action that's already a deliberate UI choice. The GLB stays cached
  (drei caches by URL); the visible "reframe" is the intended feedback.
- **One ortho variant with full controls vs. a locked top-down mode.** The
  issue floats a locked top-down (no theta/phi, pan + zoom only) as the
  "right default for heatmaps." We're shipping one ortho variant with the
  same controls as perspective first and watching for usability issues —
  cheaper to add a locked sub-variant later (a new adapter file) than to
  guess control gating upfront. The OCP seam means that follow-up costs one
  file and one registry line, not a refactor.
- **`OrthographicCamera` from drei vs. raw THREE.** Drei's variant auto-fits
  the frustum to the canvas viewport and reacts to resize — saving us a
  resize observer. The cost is a drei dependency we already use for
  `<PerspectiveCamera>` and `useGLTF`.
- **Single `zoomAnchorTarget` helper across modes.** Perspective passes
  `s = newRadius / oldRadius` (scaling the orbit radius); ortho passes
  `s = 1 / m` (scaling the inverse zoom multiplier). The math is identical:
  in both cases, `s` is the factor that the world-units-per-pixel changes
  by, and translating `target` toward `anchor` by `(1 - s)` keeps the cursor
  point fixed on screen. Kept the helper as-is.

## Documentation

`mkdocs/docs/web-app/notebooks/cell-types.md`:

- **Options table** (`:239-244`): add a `cameraKind` row —
  ```
  | `cameraKind` | `'perspective'` \| `'orthographic'` | `'perspective'` | Camera projection. Orthographic removes perspective foreshortening — better for flat heatmap-style data. Controls are identical in both modes. |
  ```
- **Camera controls table** (`:336-348`): no row changes — all controls work
  identically in both modes. Add a one-line note above or below the table
  stating that the same controls apply in both perspective and orthographic
  modes.

## Testing Strategy

- **`camera-modes.test.ts` (new):**
  - Construct each adapter and assert `kind` and the shape of `CameraElement`
    (it's a function). No mounting required.
  - **Ortho seed derivation.** Build a fake `glbCamera` (just `{ fov, near,
    far }` shape) + a fake orthographic camera with a known viewport height,
    call `seed`, assert `camera.zoom` matches the `vFov`/`radius` formula
    within tolerance.
  - **Ortho zoom anchor.** With a hand-built `target`/`anchor`/`camera.zoom`,
    call `zoom` with a synthetic `WheelEvent` and assert the anchor's world
    point projects to the same screen position before and after (within a
    small epsilon). Uses `camera.updateProjectionMatrix` + `project` from
    THREE — no DOM needed; pass a fake `domElement` rect.
- **`map-camera-math.test.ts`:** unchanged — math helpers are untouched.
- **`MapViewer.test.tsx`:** add a smoke test that mounts with
  `cameraKind="orthographic"` and a stubbed GLB, asserts no errors
  and that `useThree().camera` is an `OrthographicCamera`.
- **Full:** `yarn lint`, `yarn type-check`, `yarn test` from
  `analytics-web-app/`.
- **Manual:**
  - Switch between modes in the editor — view re-seeds, no console errors,
    no stuck cursor.
  - In ortho: left-drag pans, right-drag orbits, Ctrl-wheel cursor-anchored
    zoom feels right, WASDQE flies, `Z` resets to the seeded view.
  - In perspective: everything works exactly as it did before this change
    (regression check).
  - Two Map cells on the same page with different `cameraKind` values render
    independently.
  - **Usability watch (no acceptance gate — just observations to log for
    future tuning):** does right-drag orbit in ortho feel disorienting?
    Does fly-speed feel wrong after zooming way in or out? These are the
    signals that would justify either a locked top-down variant or a
    fly-speed tweak.

## Open Questions

None blocking — direction decisions resolved:

- **Controls** are identical in both modes; we'll watch for usability issues
  rather than gate inputs preemptively.
- **Default** stays `'perspective'` for new Map cells (saved notebooks
  unchanged; new cells match prior behavior).
