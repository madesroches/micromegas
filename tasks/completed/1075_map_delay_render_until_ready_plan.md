# Map Cell: Delay Marker/Camera Rendering Until GLB-Ready Plan

## Overview

The Map cell currently renders its `<InstancedMarkers>` and `<UnrealCameraController>`
as *siblings* of the `<Suspense>` boundary that wraps GLB loading, not as children
of it. The camera controller's `useFrame` loop ticks every frame from the moment
the `<Canvas>` mounts, applying a default-seeded orbit (`target = (0,0,0)`,
`spherical.radius = 5000`, `spherical.phi = π/4`). Even after Suspense resolves
and the GLB scene is visible in the world, the controller still uses the default
orbit for another 2–3 commit cycles while the camera-seed payload (`mapBounds`,
`mapScene`, `glbCamera`, `ambientLight`) bounces through child `useEffect` →
parent `setState` → grandchild `useEffect`. The user-visible symptom is markers
painted from the wrong viewpoint, then the camera "jumps" to its authored framing.

This plan delays the render of markers and camera controls until the GLB has
been loaded *and* its payload has propagated, *and* promotes the two effects
that delay the seed (`MapModel`'s payload extraction and
`UnrealCameraController`'s GLB-camera seeding) from `useEffect` to
`useLayoutEffect` so the whole chain — payload extraction → parent state set →
controller re-render → seed → `camera.position` write — completes inside a
single browser paint. The first frame the user sees is already correctly
framed.

## Current State

### The render graph

`analytics-web-app/src/components/map/MapViewer.tsx:980-1025`:

```tsx
<Canvas ...>
  <SceneSetup />
  <color attach="background" args={['#0a0a0f']} />
  <PerspectiveCamera makeDefault fov={60} near={1} far={100000} />

  <UnrealCameraController .../>           {/* outside Suspense */}

  {ambientLight && <ambientLight .../>}   {/* gated on parent state */}

  <Suspense fallback={<LoadingIndicator />}>
    <MapModel url={mapUrl} onLoaded={handleMapLoaded} />
  </Suspense>

  <InstancedMarkers .../>                  {/* outside Suspense */}
</Canvas>
```

### The async hops that delay the camera seed

Five commit-cycle hops between "user picks a map" and "camera correctly framed":

1. **Suspense resolves** (`MapViewer.tsx:1014-1016`). `MapModel` renders;
   `<primitive object={clonedScene} />` puts the scene in the world tree. The
   map is now visible. `glbCamera` parent state is still `null`.
2. **`MapModel` `useEffect`** (`MapViewer.tsx:56-86`) fires after that paint.
   It traverses the scene, reads `gltf.cameras[0]`, reads the `MM_ambient_light`
   extension, computes bounds, and calls `onLoaded(payload)`.
3. **`MapViewer` re-renders** when `handleMapLoaded` (`MapViewer.tsx:935-956`)
   calls four `setState`s. `UnrealCameraController` now receives a non-null
   `glbCamera` prop.
4. **`UnrealCameraController` seed `useEffect`** (`MapViewer.tsx:573-613`) fires
   after that commit. It writes `targetRef`, `sphericalRef`, `fitRadiusRef`,
   `zoomFactorRef`, and copies `fov/near/far` onto the scene camera; then calls
   `saveInitialView()`.
5. **Next `useFrame`** (`MapViewer.tsx:858-905`) finally reads the seeded
   refs and writes `camera.position`/`camera.up`/`camera.lookAt`.

During hops 1–4, `useFrame` is already running with the default
`sphericalRef = (5000, π/4, 0)` and `targetRef = (0,0,0)`. So the user sees
2–3 frames of the GLB plus markers framed from an arbitrary orbit, then a snap.

### The cleared-state window

`MapViewer.tsx:970-978` has a render-phase state derivation that clears
`mapBounds/mapScene/glbCamera/ambientLight` to `null` whenever `mapUrl` changes.
This was added because an `useEffect`-based clear races against drei's cache
hit when the new GLB is already in memory. The clear handles the A→B URL
swap correctly, but the cleared interval — between when `mapUrl` changes and
when `handleMapLoaded` fires for the new URL — is exactly the same default-orbit
window described above.

### What is already inside the Suspense boundary

Only `<MapModel>`. The `<LoadingIndicator>` fallback covers the canvas with
an HTML overlay during the GLB fetch, but it does **not** prevent the camera
controller's `useFrame` loop from running, nor the marker `InstancedMesh` from
mounting and running its three `useLayoutEffect` passes (matrix, color, highlight)
against the overlay.

### Why this matters

Two distinct user-visible bugs share this root cause:

1. **Initial load**: markers + GLB rendered from default orbit before settling
   to the GLB camera framing.
2. **Map swap**: the same flash on every `mapUrl` change, including the
   drei-cache-hit case where the new GLB resolves on the same frame.

Both will be observable on any GLB whose authored camera differs from the
default seed — i.e. every GLB.

### Tests

No unit tests exist for `MapViewer.tsx` (no test file present in
`analytics-web-app/src/components/map/__tests__/`). The current map test surface
is `EventDetailPanel.test.tsx` (template rendering) and `MapCell.test.tsx`
(overlay-build path). Three.js/r3f scenes aren't unit-tested in this repo.

## Design

### Goal

Two changes, composed:

1. **Gate** `<InstancedMarkers>`, `<UnrealCameraController>`, and
   `<ambientLight>` on a single "ready" predicate (`mapScene !== null`) so they
   only mount once the GLB payload has propagated to parent state. The
   `<LoadingIndicator>` continues to cover the canvas during the not-ready
   period.

2. **Collapse the async hops** by promoting two effects from `useEffect` to
   `useLayoutEffect`, so the payload-extract → setState → re-render → seed →
   `camera.position` write chain runs synchronously between commit and paint.
   The first frame the user sees is already correctly framed.

Together: no flash, no camera snap, no orphan markers.

### Approach: gate on `mapScene !== null`

`MapViewer` already holds the canonical "GLB payload arrived" signal in
`mapScene` — it's set by `handleMapLoaded` and cleared by the render-phase
URL-change derivation. Use it as the ready predicate.

```tsx
const ready = mapScene !== null
```

`glbCamera` is *not* part of the predicate: a GLB without an embedded camera is
a known contract violation surfaced via `contractErrors`, not a hard failure.
The seed effect in `UnrealCameraController` already early-returns when
`glbCamera` is null, so the controller can mount with `glbCamera = null` and
continue with default seeds — but only *after* `mapScene` exists, so markers
and camera are in the same lifecycle.

`mapBounds` and `ambientLight` are likewise covariant with `mapScene` (all four
are set in the same `handleMapLoaded` call) so the single predicate captures
all of them.

### Render graph after the change

```tsx
<Canvas ...>
  <SceneSetup />
  <color attach="background" args={['#0a0a0f']} />
  <PerspectiveCamera makeDefault fov={60} near={1} far={100000} />

  <Suspense fallback={<LoadingIndicator />}>
    <MapModel url={mapUrl} onLoaded={handleMapLoaded} />
  </Suspense>

  {ready && (
    <>
      <UnrealCameraController
        mapBounds={mapBounds}
        mapScene={mapScene}
        resetViewTrigger={resetViewTrigger}
        glbCamera={glbCamera}
      />
      {ambientLight && <ambientLight ... />}
      <InstancedMarkers
        overlay={overlay}
        constants={constants}
        shape={shape}
        selectedRowIndex={selectedRowIndex}
        onSelect={onSelect}
      />
    </>
  )}
</Canvas>
```

### Why mount + unmount is fine here

Three subsystems are getting conditionally mounted; each is checked:

1. **`UnrealCameraController`**: holds DOM event listeners on `gl.domElement`
   and a `useFrame` loop. All of its state lives in refs (`targetRef`,
   `sphericalRef`, `zoomFactorRef`, `fitRadiusRef`, `initialViewRef`,
   `seededGlbCameraRef`, `prevResetViewTriggerRef`, `mapSceneRef`,
   `keysRef`). When unmounted, the effect cleanup at `MapViewer.tsx:844-855`
   removes every listener and restores cursor — clean. When remounted, the
   seed effect runs immediately because the new `glbCamera` prop is
   non-null on first render. No regression.

2. **`InstancedMarkers`**: holds three `useLayoutEffect` passes that own the
   `runtimeColorsRef`/`colorAttrRef`/`prevHighlightRef` lifecycle. The overlay
   is independent of the GLB (it's built from the SQL query result), so the
   rebake triggered by remount is wasted work in the rare case where `mapUrl`
   changes but the query result hasn't. Acceptable because URL changes are
   user-initiated and infrequent, and the rebake is a one-time per-mount
   cost, not a per-frame one. `geometry.dispose()`/`material.dispose()`
   cleanup at `MapViewer.tsx:408-413` runs on unmount, so no GPU resource
   leak.

3. **`ambientLight`**: already conditional on `ambientLight !== null`. Wrapping
   it in `ready` is a no-op when `ambientLight` is set (because `ready` is
   true whenever `mapScene` is set, and they're set together), and the
   contractError "no MM_ambient_light extension" case still works because the
   `<ambientLight />` simply doesn't render — same as today.

### What happens during the not-ready window

- `<Canvas>` mounts; background color shows.
- `<PerspectiveCamera>` mounts with default `fov/near/far`. `makeDefault`
  takes effect but no `useFrame` writes to it.
- `<Suspense>` shows `<LoadingIndicator>` (existing behavior).
- No markers paint, no camera ticks. The default `(0,0,0)` camera position is
  irrelevant because nothing is rendered against it except the GLB-empty
  background.

### Collapsing the async hops via `useLayoutEffect`

Two effects bracket the seed pipeline; both run *after* paint today and that's
what produces the camera snap. Promoting them to `useLayoutEffect` makes them
run synchronously after their commit, before the browser paints. React also
processes any state updates queued inside a `useLayoutEffect` synchronously
before yielding to the browser, so the chain collapses into a single paint:

**Effect 1 — `MapModel` payload extraction** (`MapViewer.tsx:56-86`):

```ts
// before
useEffect(() => {
  clonedScene.traverse(...)       // set shadow flags
  gltf.scene.updateMatrixWorld()  // refresh world matrix for camera read
  const ambient = ...
  const bounds = new THREE.Box3().setFromObject(clonedScene)
  onLoaded({ scene: clonedScene, bounds, glbCamera, ambientLight })
}, [clonedScene, gltf, onLoaded])
```

Mutation-during-commit is fine here: `useLayoutEffect` is the documented home
for synchronous DOM/scene mutations and the scene is already in the world tree
via `<primitive>`. Setting shadow flags on mounted meshes and refreshing
matrices are both idempotent. The `onLoaded` callback ends in
`setState`s on the parent — those queue, then React processes them inside the
layout-effect flush, triggering a synchronous parent re-render.

**Effect 2 — `UnrealCameraController` seed from `glbCamera`**
(`MapViewer.tsx:573-613`):

```ts
// before
useEffect(() => {
  if (!glbCamera || seededGlbCameraRef.current === glbCamera) return
  // ...read glbCamera.getWorldPosition(), compute spherical, write directly to
  // camera.position / camera.up / camera.lookAt...
}, [glbCamera, mapBounds, camera, saveInitialView])
```

The seed already writes `camera.position`, `camera.up`, and calls `lookAt`
directly (`MapViewer.tsx:606-610`). When this runs in `useLayoutEffect`, those
writes land before paint. The next `useFrame` (rAF) re-writes the same values
from the seeded `sphericalRef`, so there's no divergence between the seed and
the steady-state loop.

### Why not the full render-phase extraction (`useMemo` payload)?

The deeper refactor — move payload extraction into a `useMemo` in `MapModel`,
publish via shared refs instead of parent state, eliminate the parent
re-render entirely — would skip one commit cycle. But the user-visible
outcome is identical: with both effects as `useLayoutEffect`, React processes
the parent setState synchronously inside the layout-effect flush, so the
parent re-render and the controller's seed effect both run *before paint*.
First paint has correct framing either way.

The `useMemo` rewrite would also bring its own awkwardness: `useMemo` is
documented as pure, but the GLB payload extraction does idempotent scene
mutation (`traverse` to set shadow flags, `updateMatrixWorld`). Doing that in
`useMemo` is a code-smell that this codebase has been careful to avoid — see
the existing render-phase state-derivation block at `MapViewer.tsx:970-978`,
which goes out of its way to use the `if (X !== Y) setState(...)` pattern
*only* for parent-owned URL-clear state, not for mutating an external object.
`useLayoutEffect` is the documented home for synchronous DOM/scene mutations
and gets us the same user-visible result, so that's the right tool here.

### Sequence after both changes

```
user picks map
  ↓
Canvas mounts (PerspectiveCamera mounts; ambient/controller/markers gated off)
  ↓
useGLTF suspends → <LoadingIndicator> shows
  ↓
GLB loads
  ↓
Render: MapModel returns <primitive object={clonedScene}/>
  ↓
Commit 1: scene in world tree (still not painted)
  ↓
useLayoutEffect (MapModel): extract payload, setState on parent
  ↓ (React flushes pending update synchronously)
Render: parent has mapScene/glbCamera/etc; ready=true; controller+markers mount
  ↓
Commit 2: controller and markers mounted
  ↓
useLayoutEffect (controller seed): writes sphericalRef + camera.position directly
useLayoutEffect (markers matrix/color passes): existing — already useLayoutEffect
  ↓
yield to browser
  ↓
rAF: useFrame writes camera.position (same values as seed)
  ↓
gl.render → first painted frame: correct framing, correct markers
```

Two commit cycles, one paint.

### Trade-offs already addressed inline

Picking `mapScene !== null` over `glbCamera !== null`: covered above. The
contract-violation path (no GLB camera) must still proceed to a rendered state.

Picking conditional mount over an in-component `if (!ready) return null` for
the camera controller: identical behavior for the controller (it already
returns `null` as its JSX, and would just early-return). But `InstancedMarkers`
returns a real `<instancedMesh>` and the disposal lifecycle of
geometry/material is tied to its mount/unmount; an in-component guard would
leave geometry alive across the not-ready window, which is harmless but
wasteful. Conditional mount keeps lifecycle behavior consistent.

### What this does NOT do

This plan does not restructure the data flow inside `MapModel` —
`onLoaded` still goes through parent state, and the payload extraction
still lives in an effect (now `useLayoutEffect`). A more invasive
refactor would publish the payload via shared refs and skip the parent
re-render entirely. That's deferred because the `useLayoutEffect`
promotion already gets the same user-visible result (correct framing on
first paint after Suspense resolves). See the corresponding entry in
Trade-offs for the full reasoning.

## Implementation Steps

1. **Compute `ready` in `MapViewer`.** In
   `analytics-web-app/src/components/map/MapViewer.tsx`, after the
   render-phase state derivation block (`MapViewer.tsx:970-978`), derive
   `const ready = mapScene !== null`.

2. **Move `<UnrealCameraController>` and `<InstancedMarkers>` inside a
   `{ready && (...)}` block.** Reuse the existing `<ambientLight>` JSX
   inside that block so all three subsystems share the same gating
   predicate. The `<Suspense>` wrapping `<MapModel>` stays where it is —
   `MapModel` must keep rendering during load so drei can suspend on it.

3. **Verify the `<PerspectiveCamera makeDefault>` stays outside the gate.**
   r3f needs a default camera registered before any component reads
   `useThree().camera`. Even though `UnrealCameraController` is the only
   reader of `useThree().camera` and it's now gated, leaving the camera
   mounted is the safe and conventional choice — it costs nothing and
   avoids r3f "no default camera" warnings during the not-ready window.

4. **Promote `MapModel`'s payload-extract effect to `useLayoutEffect`**
   (`MapViewer.tsx:56`). Change is one token: `useEffect` → `useLayoutEffect`.
   No body changes. Add the import to the existing
   `import { Suspense, useRef, useEffect, useLayoutEffect, ... } from 'react'`
   line if not already there (it is — used by the InstancedMarkers passes).

5. **Promote `UnrealCameraController`'s GLB-camera seed effect to
   `useLayoutEffect`** (`MapViewer.tsx:573`). Same one-token change. The seed
   already writes `camera.position` directly so the next paint reflects the
   correct camera without waiting for `useFrame`.

6. **Smoke-check with a slow network.** Throttle to "Slow 3G" in DevTools
   and load a map. Confirm no marker flash, no camera snap, and that the
   `<LoadingIndicator>` is the only visible UI until the GLB lands and
   `handleMapLoaded` fires.

7. **Smoke-check a map swap.** With one map loaded, change the
   `mapUrl` option in the editor. Confirm the cleared-state branch at
   `MapViewer.tsx:970-978` triggers, the loading indicator reappears, and
   the new GLB renders directly into its authored framing with no flash.

8. **Smoke-check the drei-cache-hit case.** Swap to a previously-loaded map.
   `useGLTF` returns synchronously from cache, so Suspense doesn't suspend
   visibly, but `handleMapLoaded` still runs an extra commit later. With the
   `useLayoutEffect` promotion that extra commit happens before paint, so the
   cache-hit case should show no flash at all — indistinguishable from a
   steady state. This is the case the existing render-phase clear at
   `MapViewer.tsx:970-978` was added for, and the gate + layout-effect
   promotion together close the remaining flash window.

9. **Smoke-check the contract-violation cases.** Load a GLB that lacks
   `MM_ambient_light` (contractError 2) and one that lacks an embedded camera
   (contractError 1). Markers must still appear and the contract banner must
   still render — the gate is on `mapScene`, not on the contract validity.
   The no-camera GLB seeds from the default `sphericalRef = (5000, π/4, 0)`
   because the seed effect early-returns when `glbCamera` is null; the
   contract banner explains the result.

10. **Rename `UnrealCameraController` → `MapCameraController`.** Bundled
   into this change because it touches the same file and same JSX site
   (step 2 already moves the `<UnrealCameraController>` call site, so
   renaming it costs nothing extra). Rationale:

   - The "Unreal" prefix dates to when the controller mirrored Unreal
     Editor's viewport navigation *and* the map data was in Unreal's
     left-handed Y-up frame. After plan #1036 (`1036_map_native_ue_coords_plan.md`)
     the controller is Z-up generic and works against any GLB — the name
     is now misleading and suggests an Unreal-specific coupling that
     doesn't exist.
   - `MapCameraController` matches the file's existing naming
     (`MapModel`, `MapViewer`, `MapLoadPayload`, `MapViewerProps`).

   Sites to rename (from `grep -n UnrealCamera analytics-web-app/src/components/map/MapViewer.tsx`):
   - `interface UnrealCameraControllerProps` (line 480) → `MapCameraControllerProps`
   - `function UnrealCameraController(...)` (line 487) → `MapCameraController`
   - Type annotation on the destructure (line 492)
   - Comment reference in `SceneSetup` (line 912) — also update the comment text
   - Comment in the URL-clear block (line 959)
   - JSX comment above `<PerspectiveCamera>` (line 994)
   - JSX call site (line 1000)

   Also update references in `tasks/completed/` plan documents? **No.** Historical
   plans are immutable record of how things were when written — leave them.
   Only the live source file is renamed.

## Files to Modify

- `analytics-web-app/src/components/map/MapViewer.tsx` — only file touched.
  Render-graph change inside the `<Canvas>` JSX (~10 lines), two
  `useEffect` → `useLayoutEffect` token swaps, and the
  `UnrealCameraController` → `MapCameraController` rename (~7 sites).

## Trade-offs

### Why not move `<InstancedMarkers>` inside the `<Suspense>` boundary?

Considered. Two reasons against it:

1. **Doesn't fix the camera-init gap.** Suspense resolves when the GLB binary
   is decoded — that's hop 1 in the cycle list above. The camera-seed effect
   still runs 2–3 commits later. Markers would render from the default orbit
   for that interval. The whole point of the issue is to also cover the
   *post-Suspense-but-pre-seed* window.

2. **Couples marker lifecycle to GLB lifecycle in a misleading way.**
   `InstancedMarkers` is conceptually a peer of the GLB scene; nesting it
   inside the Suspense boundary that gates the GLB suggests the markers
   *depend on the GLB scene*, which they don't — they depend on the parent
   knowing the bounds/camera are ready. The `ready` predicate captures that
   relationship without lying about the dependency.

### Why `useLayoutEffect` and not full render-phase payload extraction?

The deeper version of the collapse would move payload extraction into a
`useMemo` inside `MapModel`, publish via shared refs instead of parent
state, and skip the parent setState → re-render entirely. That's one
fewer commit cycle. But:

- React processes pending `setState`s synchronously inside the
  layout-effect flush, so the parent re-render in the `useLayoutEffect`
  path *already happens before paint*. The user-visible outcome — first
  paint has correct framing — is identical.
- The current architecture has `mapScene`/`mapBounds`/`ambientLight`/
  `contractErrors` as parent state because the contract banner reads from
  them. Moving to ref-based publish would either need a context or split
  state across two sources of truth.
- `MapModel` mutates the scene (`traverse` to set shadow flags,
  `updateMatrixWorld`). `useLayoutEffect` is the documented home for that
  kind of synchronous post-commit mutation; `useMemo` is not. Keeping
  these mutations in an effect is the right shape regardless of timing.

So `useLayoutEffect` is the right tool: same user-visible result, zero
architectural rework, two one-token changes.

### Why gate `<ambientLight>` too?

It's already conditional on `ambientLight !== null` and would behave
identically without the additional gate. But keeping all three subsystems
inside the same `{ready && (...)}` block reads as one decision instead of
three — and if a future change makes `ambientLight` covariant with something
other than `mapScene`, the gate is the obvious place to relax.

## Testing Strategy

Manual smoke checks in Implementation Steps 6–9 cover the relevant cases:
cold load, map swap, drei-cache-hit, and the two contract-violation paths.

No unit tests exist for `MapViewer.tsx` and adding one for a render-graph
gate would require a full r3f test harness (not present in the repo) to be
meaningful. The smoke checks are sufficient.

If automated coverage is desired later, a thin DOM-level assertion would
work: render `<MapViewer mapUrl="…" overlay={…} … />` in jsdom with a
mocked `useGLTF` that holds a never-resolving promise, and assert that
`<canvas>` is present but no `instanceMatrix` updates have been issued —
i.e. `InstancedMarkers` hasn't mounted. This is a fair amount of mocking
for one assertion; skip unless other r3f-level coverage materializes first.

## Documentation

None. The render-graph change is internal to one file and does not affect
the cell config schema, the GLB contract, or any user-facing UI surface.
The "GLB does not satisfy renderer contract" banner copy is unchanged.
