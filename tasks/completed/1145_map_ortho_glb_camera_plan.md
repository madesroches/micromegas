# Map Renderer: Accept Orthographic GLB Camera Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1145

## Overview

Widen the `glbCamera` type throughout the map renderer pipeline to accept `THREE.OrthographicCamera` in addition to `THREE.PerspectiveCamera`. Today, a GLB that embeds an orthographic camera (produced by the new bookmark capture path) is silently dropped and triggers the red "GLB does not satisfy renderer contract" banner.

## Current State

Three files hold perspective-camera-only type assumptions:

**`analytics-web-app/src/components/map/modes/types.ts` (line 19)**
```ts
glbCamera: THREE.PerspectiveCamera
```
`MapModeRenderProps.glbCamera` is typed as perspective-only, which propagates through both camera controllers.

**`analytics-web-app/src/components/map/MapViewer.tsx`**
- Line 47 — `MapLoadPayload.glbCamera: THREE.PerspectiveCamera | null` — extraction result
- Line 72 — `cam instanceof THREE.PerspectiveCamera ? cam : null` — silently drops ortho camera
- Line 124 — `useState<THREE.PerspectiveCamera | null>(null)` — held state

**`analytics-web-app/src/components/map/modes/PerspectiveCameraController.tsx` (lines 138–140)**
```ts
camera.fov = glbCamera.fov    // fov is perspective-only
camera.near = glbCamera.near
camera.far = glbCamera.far
```
`fov` is only defined on `THREE.PerspectiveCamera`; accessing it on `OrthographicCamera` would be a TS error once the type widens.

## Design

### Type change

In `types.ts`, widen the `glbCamera` field on `MapModeRenderProps`:
```ts
glbCamera: THREE.PerspectiveCamera | THREE.OrthographicCamera
```

In `MapViewer.tsx`, widen the same field on `MapLoadPayload` and the `useState` type:
```ts
glbCamera: THREE.PerspectiveCamera | THREE.OrthographicCamera | null
```

### Camera extraction in `MapModel`

Accept either concrete type; discard anything else:
```ts
const glbCamera =
  cam instanceof THREE.PerspectiveCamera || cam instanceof THREE.OrthographicCamera
    ? cam
    : null
```

### `PerspectiveCameraController` — guard the `fov` copy

`fov` only exists on `THREE.PerspectiveCamera`. `near`/`far` are on `THREE.Camera` (the common base), so they are unconditional:
```ts
if (glbCamera instanceof THREE.PerspectiveCamera) {
  camera.fov = glbCamera.fov
}
camera.near = glbCamera.near
camera.far = glbCamera.far
```

### `OrthographicCameraController` — no changes needed

Already reads only `glbCamera.near`, `glbCamera.far`, and world position/orientation. All three are on `THREE.Camera`. The controller will work correctly when `glbCamera` is an `OrthographicCamera`.

## Implementation Steps

1. **`modes/types.ts`** — widen `MapModeRenderProps.glbCamera` to `THREE.PerspectiveCamera | THREE.OrthographicCamera`.

2. **`MapViewer.tsx`** — widen `MapLoadPayload.glbCamera` to `THREE.PerspectiveCamera | THREE.OrthographicCamera | null`.

3. **`MapViewer.tsx`** — widen `useState<THREE.PerspectiveCamera | null>(null)` to `useState<THREE.PerspectiveCamera | THREE.OrthographicCamera | null>(null)`.

4. **`MapViewer.tsx`** — update camera extraction in `MapModel` to accept `THREE.OrthographicCamera` (see Design above).

5. **`modes/PerspectiveCameraController.tsx`** — guard the `fov` copy inside `instanceof THREE.PerspectiveCamera` (see Design above).

## Files to Modify

- `analytics-web-app/src/components/map/modes/types.ts`
- `analytics-web-app/src/components/map/MapViewer.tsx`
- `analytics-web-app/src/components/map/modes/PerspectiveCameraController.tsx`

## Trade-offs

A union type `PerspectiveCamera | OrthographicCamera` is more explicit than the common base `THREE.Camera`, at the cost of a type guard wherever a perspective-only property is accessed. The alternative — typing everything as `THREE.Camera` — would suppress the compile-time error on `fov` but silently allow any camera subclass through, weakening the contract. The union is preferred: it documents intent and catches any new perspective-only accesses at compile time.

## Testing Strategy

1. `yarn type-check` — no TS errors.
2. `yarn lint` — clean.
3. Manual smoke test with an orthographic GLB: upload via Admin → Maps, open a Map cell, switch camera to Orthographic — renders without the contract-error banner and scene is correctly framed.
4. Regression: a perspective GLB should continue to work normally.
