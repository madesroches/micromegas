# Issue #1036 — Map: native UE coordinates end-to-end (drop runtime coord math)

## Status (as of 2026-05-11)

All phases (1–5) are landed on the `map` branch and the branch has been pushed for review. The implementation grew beyond the original plan as interaction issues surfaced during testing — those late additions are tracked in **Phase 6 — Polish & branch review** below.

Verified against the current working tree:

- `MapViewer.tsx`: `SceneSetup` flips `scene.up` to `(0, 0, 1)` (camera.up is recomputed every frame from theta — see Phase 6); `sphericalToZUpOffset` / `zUpOffsetToSphericalInput` helpers applied at all four call sites (`useFrame`, reset-view, GLB-camera seed, right-mouse-down re-anchor); pan basis is theta-derived (not `camera.getWorldDirection`-based); WASD Q/E moves along Z; marker auto-scale and height offset use `Math.max(size.x, size.y)`; ground-snap raycasts from `+Z` down; marker positions are written through unmodified; heatmap plane sits at `[centerX, centerY, 10]` with no rotation and `tex.flipY = false`; `transformEvents` / `MapType` / `WorldBounds` / `MapBounds` / `onMapBoundsChange` / `fitToBounds` / `fitToMapTrigger` / `fitToDataTrigger` / `hasAutoFitRef` are all gone; the five hard-coded JSX lights are replaced by a single `<ambientLight>` driven by `MM_ambient_light`; Canvas uses `NeutralToneMapping` with `toneMappingExposure: 1.3`; `MapModel` forwards `{ scene, bounds, glbCamera, ambientLight }` through a single `onLoaded` callback; `UnrealCameraController` seeds from `glbCamera` and copies `fov`/`near`/`far`; missing-camera and missing-ambient errors log once per resolve from `handleMapLoaded` AND surface as an in-cell red banner via `contractErrors` state; `mapUrl`-clear is render-phase state derivation (not an effect) and resets all five state slots.
- `MapCell.tsx`: catalog entry collapsed to `{ name, file }`; `MapType` / `WorldBounds` imports gone; `mapType` / `worldBounds` props no longer threaded; "Fit to data" toolbar button removed (only "Reset" and "Heatmap" remain); `MapCell` and `MapCellEditor` are now top-level named exports.
- `notebook-utils.ts`: `DEFAULT_SQL.map` collapsed to `SELECT NOW() as time, 0.0 as x, 0.0 as y, 0.0 as z` (single point at origin) — no more `spans`-based placeholder.
- `EventDetailPanel.tsx`: properties iterator no longer filters `ue_x/y/z`; coordinates row shows raw `event.x/y/z`.
- `.gitignore`: `public/maps/*.glb` added alongside `public/maps/maps.json`.
- `mkdocs/docs/web-app/notebooks/cell-types.md`: Map section rewritten — catalog table is `{ name, file }`, "Coordinate frame" note replaces the old transform-modes section, "GLB authoring contract" subsection added, out-of-spec glTF disclaimer included, "Fit to data" reference removed from feature list.

Phase 4 manual verification happened iteratively through the bug-fix commits in Phase 6 (stuck-zoom, marker overlay visibility, heatmap orientation, state-clear race, etc.). No automated coverage was added; none was planned.

## Overview

The web-app map cell currently runs per-frame coordinate math at runtime to compensate for GLBs that don't carry UE world coordinates: a topdown branch fits events into a model bounding box via a per-map `worldBounds` rectangle, a 3D branch applies a fixed `*0.01` scale and Y/Z axis swap, and `InstancedMarkers` does a separate `(x, y, z) → (x, z, y)` swap when placing markers. The reason any of this exists is that the producer (UE ViewportTools) used to emit Y-up RH meters glTFs centered at the origin.

The producer side is now done — see [`topdown-glb-native-coords-plan.md`](https://github.com/madesroches/micromegas/issues/1036) for the upstream context. ViewportTools writes a renderer-contract-compliant GLB: **Z-up, left-handed, centimeters, anchored at world XY**, with one embedded perspective camera, `KHR_lights_punctual` directional + a vendor `MM_ambient_light`, and `asset.extras` provenance. A reference capture has shipped through that pipeline and validates against 31 contract invariants.

This plan is the renderer-side flip. The web app moves to a Z-up scene convention, drops all per-map fitting / `transformEvents` / `worldBounds` logic, reads its lights and initial camera straight out of the GLB, and the catalog collapses to a thin `{ name, file }` pointer (no per-map type or bounds fields).

## Current State

### MapViewer (`analytics-web-app/src/components/map/MapViewer.tsx`)

- `MapType = 'topdown' | '3d'` and `WorldBounds = { ueMinX, ueMaxX, ueMinY, ueMaxY }` declared at lines 22–29 and threaded through props (line 33–34).
- `transformEvents` (lines 694–751) does the per-map coordinate flatten — topdown branch normalizes against `worldBounds` and remaps onto the model's XZ extent, 3D branch applies `(y, z, -x) * 0.01`. Both branches stash the original UE coordinates back into `properties.ue_x/y/z` so the detail panel can show them.
- `InstancedMarkers` does a second axis swap inside the inner loop: `tempObject.position.set(pos.x, pos.z, pos.y)` (line 187). The ground-snap raycast at lines 122–155 fires from `+Y` (`new THREE.Vector3(0, -1, 0)`) down through `mapBox.max.y + 1000`.
- `UnrealCameraController` (lines 357–677) hard-codes Y-up everywhere: pan basis is built from `new THREE.Vector3(0, 1, 0)` (lines 530, 648), WASD vertical movement is `targetRef.current.y += moveSpeed` (lines 663–667), the `fitToDataTrigger` effect uses `(event.x, event.z + 50, event.y)` to reconstruct points (line 467), and the default camera position is `[0, 5000, 5000]` with `phi = 0.001` (line 418, 839).
- `effectiveMarkerSize` and `heightOffset` derive their scale factor from `Math.max(size.x, size.z)` (lines 807, 816) — assumes XZ is the ground plane.
- Lights are five hard-coded JSX nodes at lines 850–854 (`<ambientLight intensity={0.8} />`, `<hemisphereLight />`, three `<directionalLight />`s).
- `MapModel` (lines 67–85) loads the GLB via drei's `useGLTF(url)`, clones the scene, computes `Box3.setFromObject`, and forwards the cloned scene up via `onSceneReady`. It only destructures `scene` from the loader result — `cameras`, `parser`, and `userData` are dropped.

### MapCell (`analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx`)

- Catalog entry shape (lines 65–70) declares `name`, `file`, optional `type` (= `MapType`), optional `worldBounds`. `mapUrl` is matched against `entry.file` (line 122) and the resolved entry's `type` + `worldBounds` are passed to `MapViewer` (lines 192–193).
- Catalog is fetched once from `/maps/maps.json` and shared across cells via the `catalogPromise` singleton (lines 73–82).

### Documentation (`mkdocs/docs/web-app/notebooks/cell-types.md`, lines 512–605)

The Map cell section documents the `topdown` / `3d` / no-type tri-state and shows a `worldBounds` example. All of that goes away.

### What's already in shape

- The drei + three.js stack (`@react-three/drei ~9.122`, `@react-three/fiber ~8.18`, `three ^0.183`) has `KHR_lights_punctual` parsing built into `GLTFLoader` — directional / point / spot lights show up as plain `THREE.Light` instances inside `gltf.scene`. So once the producer ships lights inside the GLB, walking `clonedScene.traverse` already finds them with no extra extension handler.
- `MM_ambient_light` is a vendor extension — three.js won't decode it — but the loader exposes `gltf.parser.json.extensions["MM_ambient_light"]` verbatim, so we read it directly from the JSON.
- `gltf.cameras` is populated by `GLTFLoader` with each camera attached as a child of its glTF node. The camera's own `.position`/`.quaternion` are local (typically identity) — the world transform lives on the parent node. After `gltf.scene.updateMatrixWorld(true)`, `camera.getWorldPosition(...)` and `camera.getWorldQuaternion(...)` give the authored world transform.

## Design

### Coordinate convention

The scene runs in **Z-up, left-handed, centimeters** end-to-end. Three.js doesn't constrain handedness or up-axis at the GPU level — they're scene properties (`scene.up`, `camera.up`) that drive matrix math in the controllers and helpers. Setting `scene.up = camera.up = (0, 0, 1)` makes orbit math, `Object3D.lookAt`, and lighting all interpret Z as the world-up axis. Backface culling stays correct because the GLB exports both windings and the producer flagged the material `doubleSided`.

Marker positions are written through unmodified: `tempObject.position.set(event.x, event.y, event.z + heightOffset)`. No swap, no scale.

### What goes away

- `transformEvents`, `MapType`, `WorldBounds`, `worldBounds`/`mapType` props on `MapViewer` and `MapCell`, the `RESERVED_COLUMNS`-via-`ue_x/y/z` fallback inside `transformEvents` (the original coords *are* the marker coords now, so the detail panel reads them straight from `event.x/y/z`).
- The `tempObject.position.set(pos.x, pos.z, pos.y)` swap inside `InstancedMarkers`.
- The five hard-coded lights — replaced by reading the GLB's `KHR_lights_punctual` lights (rendered automatically via `<primitive object={scene}>`) and the `MM_ambient_light` vendor extension. No default-lights fallback: a GLB missing either piece logs an error and renders without that lighting class.
- The fixed initial camera (`[0, 5000, 5000]`, `phi = 0.001`) — replaced by GLB-camera-when-present; the default `(radius=5000, phi=π/4, theta=0)` seed stays in place when the GLB has no embedded camera (the user clicks "Fit" on the toolbar to frame the GLB). Auto-fit-to-bounds on first load is removed, since it would race the GLB-camera seeder; per step 20.

### What changes shape

- **Catalog entry**: drops `type` and `worldBounds`. Just `{ name, file }` remains. The orbit controller seeded from the GLB's embedded camera handles both flat top-down and 3D scene framing — a straight-down initial pose with free rotation gives users the "flat earth" feel by default and lets them orbit if they want to inspect from another angle. No need for a separate input-mode hint.
- **`UnrealCameraController`**: pan basis switches from `(0, 1, 0)` to `(0, 0, 1)`. WASD up/down moves along Z (`targetRef.current.z += moveSpeed`). The default spherical seed (`phi = π/4`) stays in place when the GLB has no embedded camera; the GLB-camera path is the only automatic seeder that overrides it. The toolbar "Fit" button still calls `fitToBounds(mapBounds)` on demand (per step 20).
- **GroundSnap**: ray origin is `(event.x, event.y, mapBox.max.z + 1000)`, ray direction is `(0, 0, -1)`, and the hit point's Z is `hit.point.z + heightOffset`.
- **Marker / height-offset auto-scaling**: replaces `Math.max(size.x, size.z)` with `Math.max(size.x, size.y)` since XY is now the ground plane (footprint extent). Z is height; not relevant to marker footprint.
- **`MapModel`**: forwards the full GLTF result (scene + cameras + parser) up to `MapViewer`, so the camera + ambient-light extension can be read alongside the scene.
- **`fitToDataTrigger` events box**: rebuild the bounding box with the events' actual coordinates (`event.x, event.y, event.z + 50`) instead of the swapped `(x, z, y)`.

### Lights from GLB (contract-required)

The renderer consumes lights straight from the GLB and authors none of its own.

- **Punctual lights** — already in the scene tree (drei/three.js attached them when parsing `KHR_lights_punctual`). The renderer adds nothing on top of `<primitive object={scene}>`.
- **Ambient** — three.js doesn't decode `MM_ambient_light`. Read `gltf.parser.json.extensions?.MM_ambient_light` (shape: `{ color: [r, g, b], intensity: number }`) and render `<ambientLight color={...} intensity={...} />`. If the extension is missing, log a console.error naming the offending file and skip ambient — the GLB is non-conforming.
- **No fallback path.** A GLB without `KHR_lights_punctual` lights renders without directional illumination; a GLB without `MM_ambient_light` renders without ambient. Both are visible failure modes that signal "this GLB doesn't satisfy the contract." Catalog discipline is the user's responsibility.

Hemisphere is not authored in v1. The producer contract has the option to add a sibling `MM_hemisphere_light` later for 3D scene-mode; out of scope here.

### Camera from GLB (contract-required)

After load, take the first member of `gltf.cameras`. The producer guarantees exactly one perspective camera referenced from `scenes[0]`; we don't defensively iterate.

- The camera is a `THREE.PerspectiveCamera` with `fov`, `near`, `far` populated from the camera definition. Position and orientation live on the parent node, so before reading them the consumer must call `gltf.scene.updateMatrixWorld(true)` and then use `camera.getWorldPosition(new Vector3())` / `camera.getWorldQuaternion(new Quaternion())`.
- Derive the orbit controller's initial state from it:
  - `cameraPos = camera.getWorldPosition(new Vector3())`
  - `worldQuat = camera.getWorldQuaternion(new Quaternion())`
  - `forward = new Vector3(0, 0, -1).applyQuaternion(worldQuat)`
  - `radius` = `mapBounds.getBoundingSphere(new THREE.Sphere()).radius * 2` (stable across overhead and oblique cameras). `Box3.getBoundingSphere` requires a `Sphere` target argument in three.js r131+ — the no-arg form throws `TypeError`.
  - `target` = `cameraPos + forward * radius`.
  - `phi` / `theta` = derived from the offset via `Spherical.setFromVector3(cameraPos - target)`.
  - `fov`, `near`, `far` are copied directly from the GLB camera onto the scene's perspective camera, then `sceneCamera.updateProjectionMatrix()` is called so the new intrinsics take effect.
- If `gltf.cameras` is empty, log a console.error and leave the default seed camera in place; the user gets a misframed scene as the visible failure mode.

The orbit controller assumes camera-up aligned with scene-up, so an authored camera with non-zero roll silently loses its roll on attach. Document this in the producer-facing authoring guide rather than try to support tilted cameras in the orbit controller.

### Type changes

```typescript
// MapCatalogEntry — collapse to:
interface MapCatalogEntry {
  name: string
  file: string
}

// MapViewer props — drop mapType, worldBounds
interface MapViewerProps {
  mapUrl?: string
  events: MapEvent[]
  // ...everything else stays
}

// MapModel — forward full GLTF, not just scene
interface MapModelProps {
  url: string
  onLoaded: (gltf: {
    scene: THREE.Object3D
    bounds: THREE.Box3
    glbCamera: THREE.PerspectiveCamera | null  // null = contract violation, log + degrade
    ambientLight: { color: [number, number, number]; intensity: number } | null  // null = contract violation, log + skip
  }) => void
}

// MapEvent — unchanged
```

`MapType` and `WorldBounds` exports are deleted.

### Architecture flow

```
useGLTF(url)
    │
    ├─ scene  ──────────────┐
    │                       │
    ├─ cameras[0]  ─────────┤
    │                       ├─→ MapViewer state
    └─ parser.json          │
         .extensions        │
         .MM_ambient_light ─┘
                            │
                            ├─→ <ambientLight> from MM_ambient_light extension
                            ├─→ UnrealCameraController seeded from glbCamera
                            └─→ <primitive object={scene} />  (KHR_lights_punctual lights render natively)
```

## Implementation Steps

### Phase 1 — Coordinate convention flip [DONE]

1. **[DONE]** **`MapViewer.tsx`** — set scene up-axis. Add a small `<SceneSetup />` component as the first child of `<Canvas>` so it mounts before the controller's `useFrame` runs:

    ```tsx
    function SceneSetup() {
      const { scene, camera } = useThree()
      useEffect(() => {
        scene.up.set(0, 0, 1)
        camera.up.set(0, 0, 1)
      }, [scene, camera])
      return null
    }
    ```

    `useThree` from `@react-three/fiber` is a *selector* hook (returns state); the side effect must live in `useEffect`, not in a callback to `useThree`. This must run before any controller computes a camera matrix, so render it as the first child of `<Canvas>`.
2. **[DONE]** **No JSX camera-seed change needed.** The JSX `<PerspectiveCamera position={[0, 5000, 5000]}>` at line 839 is overwritten on frame 1 by `useFrame` (line 638), which recomputes `camera.position = targetRef.current + offset.setFromSpherical(sphericalRef.current)` every frame. The only seed the user observes is encoded in `targetRef` (line 368, `Vector3(0, 0, 0)`) and `sphericalRef` (line 369, `Spherical(5000, π/4, 0)`). After the Spherical permutation fix in step 3 below, the existing `(radius=5000, phi=π/4, theta=0)` reads as "tilted overhead at 45° from world-up (+Z), no azimuth offset" — a reasonable default for the brief window before the GLB-camera effect overrides it (or the durable state if the GLB has no embedded camera and the user hasn't clicked the toolbar's "Fit" button yet). Leave the JSX `position` prop alone (or set it to anything — it has no observable effect).
3. **[DONE]** **`MapViewer.tsx` orbit math — explicit Y-up→Z-up offset permutation.** `THREE.Spherical` is hard-coded Y-up: `setFromCartesianCoords` uses `phi = acos(y/r)` and `setFromSphericalCoords` uses `y = cos(phi) * r`. Setting `scene.up = camera.up = (0, 0, 1)` doesn't change that. Without an explicit fix, `phi = 0` would put the camera offset along scene +Y (a horizontal direction in a Z-up scene), not along scene +Z (world up) — orbit's "tilt-from-vertical" angle would tilt around the wrong axis.

    Fix: keep the `Spherical` storage as-is, but permute its decoded offset to remap Y-up→Z-up at every consumption point. After every `offset.setFromSpherical(spherical)` call:
    ```ts
    offset.setFromSpherical(spherical)
    // Reinterpret Y-up offset as Z-up: (x, y, z) → (x, -z, y) so that phi=0 → +Z (world up).
    const tmp = offset.y
    offset.y = -offset.z
    offset.z = tmp
    ```
    Wrap in a `sphericalToZUpOffset(spherical, out)` helper. Apply at three call sites: `fitToBounds` (line 425), `useFrame`'s per-frame update (line 671), and the reset-view effect (line 484). The inverse — converting a Z-up world offset back to a Y-up `Spherical` — is needed in step 20 when seeding from a GLB camera; convert via `(x, y, z) → (x, z, -y)` before `Spherical.setFromVector3`.

    With this in place, the existing seeds keep their original geometric meaning: `phi = π/4` (line 369) is "45° tilt from world-up", `phi = 0.001` (line 418, fitToBounds) is "near-overhead with tiny tilt to dodge the lookAt degeneracy" — no further adjustment to the seed values needed.

    Also replace `new THREE.Vector3(0, 1, 0)` with `new THREE.Vector3(0, 0, 1)` at the two world-up call sites (line 530's `panCamera` right-vector cross, line 648's WASD right-vector cross) so pan and strafe align with screen-up under Z-up.

    Finally, fix the ground-plane projection in `panCamera` (line 537): the existing `forward.y = 0` flattens the pan-forward vector onto the XZ plane (the ground in Y-up). Under Z-up the ground plane is XY, so this must become `forward.z = 0` — otherwise dragging "forward" pans the target along world -Z (downward through the floor) instead of along the ground plane. The WASD code at lines 643–661 uses unprojected `forward` for full-3D fly, so it needs no analogous change.
4. **[DONE]** **`MapViewer.tsx:663–667`** — change Q/E vertical fly to move along Z: `targetRef.current.z += moveSpeed` for E, `-= moveSpeed` for Q. Drop `targetRef.current.y +=`.
5. **[DONE]** **`MapViewer.tsx:467`** — fitToData event box: `box.expandByPoint(new THREE.Vector3(event.x, event.y, event.z + 50))`. Drop the swap.
6. **[DONE]** **`MapViewer.tsx:805–823`** — marker auto-scale and height-offset auto-scale: replace `Math.max(size.x, size.z)` with `Math.max(size.x, size.y)`. The trigger condition (`mapType && mapBounds`) becomes just `mapBounds` since `mapType` is gone; same for the `else` fallback that returns the un-scaled `markerSize`/`heightOffset`. Also drop `mapType` from the two useMemo dependency arrays (line 811 `[markerSize, mapType, mapBounds]` and line 823 `[mapType, mapBounds, heightOffsetProp]`) — leaving them in place would `ReferenceError` once step 16 deletes the prop.
7. **[DONE]** **`MapViewer.tsx:786–790`** — `onMapBoundsChange` currently flips Y/Z (`{ y: bounds.min.z, z: bounds.min.y }`) to convert back to UE coords. After the flip, GLB bounds are already UE coords, so emit `{ x: bounds.min.x, y: bounds.min.y, z: bounds.min.z }` directly. Verify no consumer of `onMapBoundsChange` relies on the old swap (grep for it).
8. **[DONE]** **`MapViewer.tsx:122` / `:132–135`** — `InstancedMarkers` ground-snap: `rayDirection = new THREE.Vector3(0, 0, -1)`; `rayStartHeight = mapBox.max.z + 1000`; `rayOrigin.set(event.x, event.y, rayStartHeight)`; the hit position becomes `{ x: event.x, y: event.y, z: hit.point.z + heightOffset }`.
9. **[DONE]** **`MapViewer.tsx:187`** — drop the swap: `tempObject.position.set(pos.x, pos.y, pos.z)`.
10. **[DONE]** **`MapViewer.tsx:341`** — `HeatmapLayer`'s plane: it's currently positioned at `[centerX, 10, centerY]` with `rotation={[-Math.PI/2, 0, 0]}` (rotating an XZ plane to face up). Under Z-up, a `planeGeometry` with no rotation is already in the XY plane facing +Z — drop the rotation, position at `[centerX, centerY, 10]` (10 cm above the ground).
11. **[DONE]** **`MapViewer.tsx:694–751`** — delete `transformEvents` entirely. Delete the `useMemo` at lines 799–802 and replace each of the three `transformedEvents` JSX references with `events`: line 845 (`UnrealCameraController` `events` prop), line 869 (`HeatmapLayer` `events` prop), line 874 (`InstancedMarkers` `events` prop). Also update `EventDetailPanel.tsx` (lines 43, 58–60): drop the `ue_x/y/z` filter from the properties iterator and collapse the coordinates display to just the `event.x/y/z` formatter — `transformEvents` no longer injects `ue_x/y/z`, so the synthetic source is gone. Caveat: a user query that *happens* to return columns named `ue_x` / `ue_y` / `ue_z` will now flow into `properties` (they aren't in `RESERVED_COLUMNS` at `MapCell.tsx:24`) and render as generic property rows. This is a niche collision and acceptable; mention it in the Phase 5 docs rewrite as a reserved-name caveat rather than re-adding the filter.
12. **[DONE]** **`MapViewer.tsx:408–417` (`fitToBounds`)** — the framing math is hard-coded to Y-up: `distForZ = (size.z / 2) / tan(fov/2)` treats Z as the screen-vertical extent (correct when the camera looks down -Y). Under Z-up with the camera looking down -Z, screen-vertical maps to scene Y and the height axis (now Z) is irrelevant for top-down framing. Replace `size.z` with `size.y` in the vertical-extent calc and rename the local `distForZ` → `distForY` to keep the name aligned with the math; `size.x` / `distForX` for the horizontal-extent calc stay the same. Without this fix, both fit-to-data and fit-to-map produce visibly wrong distances.

    *Superseded by Phase 6:* `fitToBounds` was deleted entirely when the "Fit to data" toolbar button was removed and the `fitToMapTrigger` machinery was dropped (the GLB-camera seed is now the only automatic seeder).

### Phase 2 — Catalog collapse [DONE]

13. **[DONE]** **`MapCell.tsx:65–70`** — collapse the `MapCatalogEntry` interface to `{ name: string; file: string }`.
14. **[DONE]** **`MapCell.tsx:15`** — drop `MapType` and `WorldBounds` from the import statement (`import { MapViewer, type MapEvent } from '@/components/map/MapViewer'`); they're being deleted in step 16 and the import would otherwise fail typecheck.
15. **[DONE]** **`MapCell.tsx:122–125, 192–193`** — drop the `mapType` and `worldBounds` props on `MapViewer`, and delete the now-unused `catalogEntry` `useMemo` (its only readers were `entry.type` / `entry.worldBounds`). The catalog itself is still consumed by `MapCellEditor` for the dropdown — that stays.
16. **[DONE]** **`MapViewer.tsx:22–34`** — delete the `MapType` and `WorldBounds` exports; drop the corresponding props from `MapViewerProps`.
17. **[DONE]** **`public/maps/` is user-supplied; both the catalog and GLBs stay gitignored.** `analytics-web-app/.gitignore` already excludes `public/maps/maps.json` (per-developer catalog). Add `public/maps/*.glb` next to that line. Reason for ignoring GLBs: binaries can carry project-identifying metadata (`asset.extras.source.{level, bookmark}` from the producer's provenance) and the embedded texture can show in-game signage / debug overlays. Dev workflow: drop your own GLB into `public/maps/`, write a local `maps.json` pointing at it, run `yarn dev`.

    Local `maps.json` shape (template — not committed):

    ```json
    [
      {
        "name": "Example Map",
        "file": "/maps/example.glb"
      }
    ]
    ```

### Phase 3 — Camera + lights from GLB [DONE]

18. **[DONE]** **`MapViewer.tsx:67–85`** — rework `MapModel`. Read the full `useGLTF` result: `const gltf = useGLTF(url)`. Call `gltf.scene.updateMatrixWorld(true)` so cameras' world transforms are current before forwarding (cameras' own `.position`/`.quaternion` are local-to-parent and won't reflect the authored placement otherwise). Clone the scene as today. Pick `glbCamera` with a runtime perspective check: `const cam = gltf.cameras[0]; const glbCamera = cam instanceof THREE.PerspectiveCamera ? cam : null`. `GLTF.cameras` is typed `Camera[]` in `three/examples/jsm/loaders/GLTFLoader.d.ts`, so the runtime check (not just a cast) is required: it satisfies the `PerspectiveCamera | null` shape `MapModelProps` declares *and* makes a non-conforming GLB with an orthographic camera fall into the `null` branch — which triggers the missing-camera log in step 21 — instead of silently casting and then NaN-propagating through the controller's `.fov`/`.near`/`.far` reads. The producer contract guarantees exactly one perspective camera referenced from `scenes[0]`, so no traversal/reachability check is needed beyond the type guard. Read the ambient extension as `const ambientLight = (gltf.parser.json.extensions?.MM_ambient_light ?? null) as { color: [number, number, number]; intensity: number } | null` — the parser-json path is typed `any` so the cast pins down the shape; if you'd rather not trust the producer here, narrow at runtime with an `Array.isArray(color) && color.length === 3 && typeof intensity === 'number'` check before forwarding. Forward `{ scene, bounds, glbCamera, ambientLight }` via a single `onLoaded` callback. Punctual lights (directional, point, spot) ride inside `scene` and render automatically via `<primitive object={scene} />` — no explicit "has lights" flag is needed.
19. **[DONE]** **`MapViewer.tsx:850–854`** — delete the five hard-coded JSX lights. Replace with a single `<ambientLight>` driven by the GLB's `MM_ambient_light` extension when present. If the extension is missing, render no ambient (the missing-extension `console.error` is fired from `handleMapLoaded` per step 21, not from this JSX render). Punctual lights (directional, etc.) come from inside `<primitive object={scene} />` — nothing to do at the JSX level.
20. **[DONE]** **`UnrealCameraController`** — add a `glbCamera: THREE.PerspectiveCamera | null` prop. Add an effect parallel to the existing `fitToMapTrigger` effect: when `glbCamera` becomes non-null, read its world transform (`MapModel` must call `gltf.scene.updateMatrixWorld(true)` before forwarding it). Derive `target` / `radius` / `phi` / `theta`:
    - `cameraPos = glbCamera.getWorldPosition(new Vector3())`
    - `worldQuat = glbCamera.getWorldQuaternion(new Quaternion())`
    - `forward = (0, 0, -1).applyQuaternion(worldQuat)`
    - `radius = mapBounds.getBoundingSphere(new THREE.Sphere()).radius * 2` if available (`Box3.getBoundingSphere` requires a `Sphere` target argument in this three.js version), else `Math.max(cameraPos.length(), 1000)`
    - `target = cameraPos + forward * radius`
    - spherical from `cameraPos - target` via the inverse permutation from step 3 (apply `(x, y, z) → (x, z, -y)` to the offset before calling `Spherical.setFromVector3`, since `Spherical` expects a Y-up vector)
    - `fitRadiusRef.current = radius` and `zoomFactorRef.current = 1.0` — without this, the next wheel-zoom (line 591: `sphericalRef.current.radius = fitRadiusRef.current * zoomFactorRef.current`) snaps the camera back to the constructor-default `5000` cm.
    - Call `saveInitialView()` after seeding `target` / `spherical` so the toolbar's "Reset View" button has a meaningful state to restore (today, `initialViewRef` is only populated inside the `fitToMapTrigger` and now-deleted `hasAutoFitRef` effects via `saveInitialView()`; without this call, "Reset View" silently no-ops on first interaction because `initialViewRef.current` stays null). For the no-GLB-camera fallback, `initialViewRef` stays null and "Reset View" no-ops until the user manually clicks "Fit" once — acceptable, since the visible failure mode already signals a non-conforming GLB.

    Then copy `glbCamera.fov`, `.near`, `.far` onto the scene camera and call `sceneCamera.updateProjectionMatrix()` — without that call the new intrinsics never reach the projection matrix. If `glbCamera` is null, the controller's effect early-returns and leaves the default seed in place; the missing-camera `console.error` is fired from `handleMapLoaded` per step 21 (not from this controller effect, which mounts before the GLB resolves and would false-fire the log on every mount).

    **Delete the `hasAutoFitRef` ref (line 452) and its `useEffect` (lines 453–459) outright.** Leaving them in place would race the new GLB-camera effect: both fire when `mapBounds` becomes non-null on the same render, and the auto-fit's `fitToBounds(mapBounds)` would clobber the GLB-camera-derived `target` / `spherical` state. The GLB-camera path is now the single deterministic automatic seeder; the manual `fitToMapTrigger` effect (lines 443–450) stays for the toolbar's "Fit" button only — handleMapLoaded must NOT increment `fitToMapTrigger` on load (see step 21), or it would re-introduce the same race via a different code path.
21. **[DONE]** **`MapViewer.tsx:771–797, 836–848`** — rewire the `MapModel` call site for the new single-callback shape, and thread the new payload fields through to the JSX. Concretely:
    - Replace the `mapBounds` / `mapScene` `useState` pair (line 771–772) with four pieces of state: `mapBounds`, `mapScene`, `glbCamera`, `ambientLight` — each initially `null` (or `Box3 | null` for bounds, matching today).
    - Delete `handleBoundsCalculated` (lines 780–793) and `handleSceneReady` (lines 795–797). Replace with a single `handleMapLoaded` that destructures `{ scene, bounds, glbCamera, ambientLight }`, calls `setMapBounds(bounds)`, sets the other three from the payload, and calls `onMapBoundsChange` with the un-swapped bounds (per step 7). The `MapModel` JSX (line 858–862) takes `onLoaded={handleMapLoaded}` instead of the two old props. **Do NOT call `setFitToMapTrigger((p) => p + 1)` here** — today's `handleBoundsCalculated` increments that trigger to auto-fit on first load, but under the new design the GLB-camera effect (step 20) is the single deterministic automatic seeder; incrementing `fitToMapTrigger` would fire its useEffect → `fitToBounds(mapBounds)` → clobber the GLB-camera-derived `target` / `spherical` state on the same render the GLB-camera effect runs. The `fitToMapTrigger` state stays in place for the toolbar "Fit" button only.
    - Inside `handleMapLoaded`, also fire the contract-violation logs from this single load-completion path. Both messages must name the offending file (per the Design section's "naming the offending file" wording) by including `mapUrl` so a developer flipping between catalog entries knows which GLB to fix: when `payload.ambientLight === null` log `console.error(`[MapViewer] GLB ${mapUrl} is missing MM_ambient_light extension; ambient lighting will be absent`)`, and when `payload.glbCamera === null` log `console.error(`[MapViewer] GLB ${mapUrl} has no perspective camera; initial framing may be wrong`)`. This runs exactly once per GLB resolve with the *actual* loaded payload — driving these logs from a `useEffect` on the state values (or from the controller effect for the camera log) would false-fire on initial mount, when the state/prop is null because the GLB hasn't resolved yet, regardless of whether the eventual GLB satisfies the contract.
    - Pass `glbCamera={glbCamera}` to `<UnrealCameraController>` (the prop added in step 20). Single controller, no mode switch.
    - Render ambient from state (per step 19): `{ambientLight && <ambientLight color={...} intensity={ambientLight.intensity} />}`.
    - The `useEffect` at lines 825–832 that clears state when `mapUrl` becomes empty must also clear `mapBounds`, `glbCamera`, and `ambientLight` so stale state doesn't bleed across map switches. (`mapBounds` is a pre-existing gap — today's effect only clears `mapScene` and the external `onMapBoundsChange` callback, leaving the local `mapBounds` state stale; the new GLB-camera effect makes that gap visible.)

### Phase 4 — Tests & manual verification [DONE]

22. **[DONE]** **Manual verification** — verified iteratively through the Phase 6 bug-fix commits. The producer's reference GLB renders with markers aligned to map features, GLB-camera seed frames the bookmark, GLB-sourced ambient + directional lights render correctly, pan/orbit/WASD all behave under Z-up, ground-snap places markers on the textured plane, Reset View works, and the heatmap overlay sits on the ground plane in the correct orientation. The "Fit-to-data" path mentioned in the original plan was deleted; see Phase 6.
23. **[DONE]** **Non-conforming GLB verification** — handled via the in-cell red banner (`contractErrors` JSX overlay) plus the per-resolve `console.error` calls in `handleMapLoaded`. Missing `MM_ambient_light` → ambient skipped, banner lists the missing piece; missing GLB camera → default seed framing used, banner lists the missing piece. Cell does not crash; user can pan/orbit to find data.
24. **[DONE]** **Regression sweep** — `yarn lint` and `yarn type-check` pass on the branch (lint warnings cleared in commit `c2000b042`). No new test coverage was added.

### Phase 5 — Documentation [DONE]

25. **[DONE]** **`mkdocs/docs/web-app/notebooks/cell-types.md:512–605`** — rewrite the Map cell catalog and "Coordinate transform modes" sections:
    - Drop the `type` field and the entire `worldBounds` block from the catalog example.
    - Replace the "Coordinate transform modes" subsection with a "Coordinate frame" note: events are placed at their raw `x`, `y`, `z` values; the GLB is expected to be authored in the same frame and units the events are emitted in. Cross-reference the producer-side authoring guide.
    - Add a short "GLB authoring contract" subsection: Z-up, left-handed, no auto-centering, raw centimeters, exactly one embedded perspective camera referenced from `scenes[0]`, `KHR_lights_punctual` directional, and `MM_ambient_light` vendor extension. Call out that GLBs missing any contract bit will log console errors and degrade visibly (no ambient / wrong framing).
    - Update the catalog table: drop `type` and `worldBounds` rows. Catalog is now `{ name, file }` only.
    - Drop the "the event detail panel shows the original UE coordinates" sentence — the panel now shows `event.x/y/z` which *are* the UE coordinates.
    - Add a note that the catalog's referenced GLB files are user-supplied and gitignored under `public/maps/`.

### Phase 6 — Polish & branch review [DONE]

Issues that surfaced during manual testing and branch review, after the Phase 1–5 cutover landed. Each item is a separate commit on the branch.

26. **[DONE]** **Default SQL collapse** (`notebook-utils.ts`) — `DEFAULT_SQL.map` was a placeholder query against `spans` with `properties->>'x'`-style accessors that don't match the new `x`/`y`/`z` column contract. Replaced with `SELECT NOW() as time, 0.0 as x, 0.0 as y, 0.0 as z` — a single point at origin that renders one marker, gives the user a working starting state, and unambiguously demonstrates the expected column shape.
27. **[DONE]** **Neutral tone mapping** (`MapViewer.tsx` Canvas `gl` prop) — switched from R3F's default ACES tone mapping to `THREE.NeutralToneMapping` with `toneMappingExposure: 1.3`. ACES crushes the producer's authored albedos toward orange; Neutral preserves them.
28. **[DONE]** **Right-mouse-down orbit re-anchor** (`UnrealCameraController`) — on right-mouse-down, raycast cursor against `mapSceneRef.current` and re-anchor `targetRef` / recompute `sphericalRef` from `(camera.position - newTarget)`. Recomputes `zoomFactorRef = radius/fitRadius` to preserve `radius = fitRadius * zoomFactor` (does NOT shrink `fitRadius` — that would cap zoom-out). Without this, right-drag rotates around stale targets from prior fits/flys.
29. **[DONE]** **Cursor-anchored wheel zoom + window-scoped drag handlers** (`UnrealCameraController`) —
    - Wheel zoom raycasts cursor against the map scene; scales `targetRef` around the hit point by `s = newRadius/oldRadius`. Since the new camera position is also `s` times the old offset from anchor, the cursor's world hit stays at the same NDC across the zoom step.
    - `mousedown` stays on the canvas (drags only start over the map); `mousemove` / `mouseup` move to `window` so drags that sweep off the canvas keep tracking and a release-outside isn't lost.
    - Added a `window.blur` listener as a safety net: alt-tab / OS dialogs can swallow the eventual `mouseup`, so we clear all drag-state refs on blur to start the next interaction clean.
30. **[DONE]** **Stuck-zoom fixes** (`UnrealCameraController`) —
    - `zoomFactor` clamp widened from `[0.01, 1.0]` to `[0.001, 10.0]` so re-anchored zoomFactors that land far outside the original range don't snap on the next wheel event, and users can zoom out past the initial scene fit.
    - Reset-view effect restores `fitRadiusRef.current = initialView.spherical.radius` alongside the spherical state. Without this, a stale `fitRadius` left over from a prior re-anchor caused the next wheel to snap radius to `staleFitRadius * zoomFactor`.
31. **[DONE]** **Marker overlay visibility & picking** (`InstancedMarkers`) — `MeshBasicMaterial` now sets `depthTest: false, depthWrite: false`; mesh sets `renderOrder={10}`. Markers render in front of the map regardless of their Z relative to map geometry. The marker-update effect calls `mesh.computeBoundingSphere()` after writing matrices — without this, raycast picking and frustum culling skip every instance that doesn't intersect the unit-sphere-at-origin default bound.
32. **[DONE]** **Remove "Fit to data" toolbar button** (`MapCell.tsx`) — `fitToDataTrigger` state and prop are deleted. The GLB-camera seed is the single automatic framer; users who want to recenter on data click "Reset" (which now restores the GLB-camera framing) or just orbit. `fitToBounds`, `fitToMapTrigger`, and `hasAutoFitRef` were all removed from `MapViewer.tsx` in the same cleanup; step 12's `fitToBounds` framing-math fix is moot as a result.
33. **[DONE]** **Lint warnings cleared** — drop unused imports / variables introduced during the Phase 1–3 refactor (e.g. unused `tempColor`, stale `mapType`/`worldBounds` refs).
34. **[DONE]** **Theta-based pan & strafe basis** (`UnrealCameraController`) — `panCamera` and the WASD strafe path no longer compute `right` via `cross(camera-forward, world-up)`. At `phi=0` (looking straight down), camera-forward is parallel to world-up and the cross product silently collapses to zero, dropping all horizontal input. Both paths now derive `right` directly from `sphericalRef.current.theta`, which is well-defined at every `phi`. WASD `forward` still uses unprojected `camera.getWorldDirection` for full-3D fly semantics.
35. **[DONE]** **Theta-driven `camera.up` every frame** (`UnrealCameraController.useFrame`) — at `phi=0`, the spherical offset is parallel to world-up and a static `(0,0,1)` `camera.up` makes `lookAt` degenerate. `camera.up` is now recomputed each frame as `(-sin(theta), cos(theta), 0)`. This keeps `lookAt` well-defined at every `phi` AND makes `theta` rotate the screen orientation when looking straight down (which is otherwise rotation-invariant). The phi clamp was simultaneously expanded to `[0, π/2 - 0.05]` so users can hit straight-down (but can't flip below the horizon).

    *Note:* `SceneSetup` now only sets `scene.up = (0, 0, 1)`. Setting `camera.up` there would be dead code — the per-frame override would clobber it immediately.
36. **[DONE]** **Render-phase state-clear for `mapUrl` changes** (`MapViewer`) — the original `useEffect`-based clear race against `MapModel`'s load effect when the new GLB is already in drei's `useGLTF` cache (no Suspense). `useEffect`s fire child-first then parent, and React 18 batches both into one commit where the parent's `null` wins — leaving `mapScene` null and ground-snap raycasting against a stale scene. Replaced with a render-phase state derivation:

    ```tsx
    const [clearedForUrl, setClearedForUrl] = useState(mapUrl)
    if (clearedForUrl !== mapUrl) {
      setClearedForUrl(mapUrl)
      setMapBounds(null)
      setMapScene(null)
      setGlbCamera(null)
      setAmbientLight(null)
      setContractErrors([])
    }
    ```

    This forces the clear before `MapModel` re-renders, so the child effect runs against already-cleared state. Documented "adjusting state during rendering" React pattern.
37. **[DONE]** **Docs: drop the `ue_x/y/z` reserved-column note** — the original Phase 5 doc warned that user queries returning `ue_x`/`ue_y`/`ue_z` columns would flow into `properties` as plain rows. The note was confusing without context (there's no longer any synthetic `ue_x/y/z` injection happening) and was deleted from the cell-types page.
38. **[DONE]** **Heatmap canvas-texture unflip** (`HeatmapLayer`) — `tex.flipY = false`. Canvas Y grows top-to-bottom; `CanvasTexture`'s default `flipY = true` would map canvas-row-0 to plane local +Y, sending events at world `minY` to world `maxY` on the plane (mirrored relative to the markers). With `flipY = false`, canvas-row-0 maps to plane local -Y and the density aligns with the markers.
39. **[DONE]** **Drop `onMapBoundsChange` / `MapBounds` and partial marker updates** (branch-review cleanup) — the `onMapBoundsChange` callback and `MapBounds` interface were dead wiring (no consumers); deleted. The two-pass marker update (matrix loop + separate color loop with `tempColor.copy(...)`) was collapsed to a single pass that writes both matrix and color in one iteration. The InstancedMarkers `key={...}` no longer includes `markerColor` / `effectiveMarkerSize` / `groundSnap` — those changes flow through React reconciliation, not forced remount.
40. **[DONE]** **Single-effect marker update; in-cell GLB contract errors** —
    - Removed the partial-marker-update path entirely. There's now exactly one `useEffect` in `InstancedMarkers` that writes matrices, colors, and `computeBoundingSphere` together.
    - Surfaced GLB contract violations in the cell UI: `contractErrors` state in `MapViewer` populated by `handleMapLoaded`, rendered as a red banner at the top of the canvas listing the missing pieces and the offending `mapUrl`. The `console.error` calls remain for developer-tools logging.

## Files to Modify

- `analytics-web-app/src/components/map/MapViewer.tsx` — coordinate flip, camera/lights from GLB, drop `transformEvents`/`MapType`/`WorldBounds`/`fitToBounds`/`onMapBoundsChange`; Phase 6 interaction work (theta-based pan/strafe/camera-up, cursor-anchored zoom, window-scoped drag handlers, render-phase state clear, in-cell contract-error banner, NeutralToneMapping, marker overlay visibility, heatmap unflip).
- `analytics-web-app/src/lib/screen-renderers/cells/MapCell.tsx` — catalog shape collapse, prop wiring, "Fit to data" button removal; `MapCell` / `MapCellEditor` now top-level exports.
- `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts` — `DEFAULT_SQL.map` collapsed to a single point at origin.
- `analytics-web-app/.gitignore` — add `public/maps/*.glb` next to the existing `public/maps/maps.json` entry to keep user-supplied GLB binaries out of the repo.
- `analytics-web-app/src/components/map/EventDetailPanel.tsx` — drop the `ue_x/y/z` filter and the dual-coord display fallback (events now carry their UE coords directly in `event.x/y/z`).
- `mkdocs/docs/web-app/notebooks/cell-types.md` — rewrite the catalog + coordinate sections of the Map cell docs; drop the `ue_x/y/z` reserved-column note; drop "Fit to data" from the feature list.

## Trade-offs

- **Hard cutover, no fallbacks.** The producer's reference GLB is the only GLB shipping through the new contract, and `public/maps/` is otherwise empty in the repo. Keeping a `mapType: 'topdown' | '3d'` branch alive in parallel would mean dual-maintaining two coordinate frames in the same component for an unknown duration — not worth it. We also drop the "no embedded camera / no embedded lights" defensive paths: a non-conforming GLB logs a console error and renders without lights / with the default seed framing, which is a visible, fixable failure mode rather than dead code in the renderer.
- **No "locked topdown" controller mode.** A flat-earth feel is achievable with the orbit controller seeded from a straight-down GLB camera; the user can rotate if they want to inspect from another angle. Adding a separate locked-pan-zoom mode would double the controller's state surface for no current product need. If a future use case wants the locked feel, add it then — the catalog's currently-empty schema has room.
- **Out-of-spec glTF**. The producer-authored GLBs are technically non-conformant (glTF 2.0 mandates Y-up RH meters). External viewers — Blender, online glTF validators, Windows 3D Viewer — will render them rotated or flag warnings. The micromegas web-app is the only intended consumer; the producer doc has the same disclaimer. We accept this and document it.
- **Custom controller stays under Z-up via an explicit Spherical permutation.** `THREE.Spherical` is permanently Y-up — setting `scene.up`/`camera.up` doesn't change its math. Rather than swap to drei's `OrbitControls` (which honors `camera.up` natively but doesn't carry over the speed-tunable WASD-fly, drag-threshold pan-vs-select, and right-mouse wheel-speed behaviors), we keep the hand-rolled `UnrealCameraController` and apply a fixed `(x, y, z) → (x, -z, y)` permutation around every `setFromSpherical` / `setFromVector3` boundary (step 3). Three call sites, no behavioral surprises. If the rotate/pan still feels wrong after that, the fallback is still drei's `OrbitControls`.

## Migration

- **Existing maps in `public/maps/maps.json`** — none in the repo today. New GLBs are user-supplied and gitignored. Anyone with a pre-refactor GLB authored Y-up RH meters needs to re-export through the producer (or equivalent DCC pipeline) before pointing the catalog at it.
- **Telemetry events** — already emitted in raw UE coordinates upstream (the SQL columns `x`, `y`, `z` are the raw UE values). No change.

## Documentation

- `mkdocs/docs/web-app/notebooks/cell-types.md` — rewrite the Map section per Phase 5.
- `mkdocs/docs/...` — no other doc page references `worldBounds` or `mapType` (verified by grep). The producer-side authoring guide lives off-repo with the upstream UE plugin.

## Testing Strategy

- Type checking + lint pin the structural changes (`yarn type-check`, `yarn lint`).
- Existing `yarn test` pass — no unit tests on `MapViewer.tsx` itself today; integration tests touching `MapCell` should still pass since the public surface (SQL → events → markers) is unchanged.
- Manual verification of the conforming-GLB path (Phase 4 step 22) and the non-conforming-GLB fallback path (step 23) is load-bearing — there's no automated way to assert "the marker is on the textured plane and the camera frames the bookmark" without a screenshot pipeline we don't have.
- Coordinate the producer's reference GLB drop and the renderer cutover so there's no flash of unlit / mis-fitted content during the deploy. The GLB itself stays gitignored (per Files to Modify / Migration sections); coordination here means lining up the developer-local catalog refresh with the merge of this PR.

## Open Questions

None remaining — catalog is just `{ name, file }`, GLB binaries are gitignored, and the renderer requires a contract-compliant GLB with no fallback path.
