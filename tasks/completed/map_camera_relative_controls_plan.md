# Map Cell: Camera-Relative Keyboard Controls Plan

## Overview

The Map cell's keyboard controls collapse onto the same world direction when
the camera is tilted to look near-horizontally. Specifically, W/S
(theta-based XY pan along the "forward" projection) and Q/E (radial zoom
along the camera-to-target ray) both translate the camera along the
camera-forward XY direction at high `phi`. The user-visible symptom is "W-S
and Q-E do the same thing after I rotate 90 degrees".

Replace the current mixed-basis scheme (XY-pan for WASD + radial-zoom for
QE) with a single orthonormal camera-relative basis derived from the orbit's
spherical state. Each key pair maps to one of the camera's three
mutually-perpendicular world-space axes (the right-hand rule):

- **A/D** → camera-right (screen X)
- **W/S** → camera-up (screen Y)
- **Q/E** → camera-forward (screen −Z, into the scene)

Because the three pairs are orthogonal at every camera tilt, they can never
collapse onto the same world direction. Radial zoom remains available via
`Ctrl/Cmd + wheel` (already a separate input path); the keyboard no longer
controls radius.

## Current State

### Where the controls live

`analytics-web-app/src/components/map/MapViewer.tsx`, inside the
`MapCameraController` component:

- **Key state**: `keysRef` initialized at lines 526–533 with the six keys
  (`w`, `a`, `s`, `d`, `q`, `e`).
- **Key listeners**: `onKeyDown` / `onKeyUp` at lines 794–809 set/clear the
  ref; hover-leave at lines 816–824 force-clears.
- **WASD apply** (`useFrame`): lines 871–894.
  ```ts
  const theta = sphericalRef.current.theta
  const right = new THREE.Vector3(Math.cos(theta), Math.sin(theta), 0)
  const forward = new THREE.Vector3(-Math.sin(theta), Math.cos(theta), 0)
  if (keysRef.current.w) targetRef.current.addScaledVector(forward, moveSpeed)
  ...
  ```
  Both basis vectors are in the XY plane — independent of `phi`. The
  comment at lines 875–877 explains the choice: "keeps WASD as a top-down
  pan (W/S no longer changes elevation), and avoids the cross-product
  collapse at phi=0".
- **Q/E apply** (`useFrame`): lines 896–910. Scales `sphericalRef.radius`
  exponentially (Q in, E out), with a `[0.001, 10.0]` clamp on
  `zoomFactorRef`.

### Why W-S and Q-E collapse at high phi

The orbit places the camera at `target + sphericalOffset` where
`sphericalOffset = (sin(phi)sin(theta), -sin(phi)cos(theta), cos(phi)) * radius`
(see `sphericalToZUpOffset` at lines 471–477).

- **WASD forward** = `(-sin(theta), cos(theta), 0)` — XY-forward, the projection
  of the camera-to-target ray onto the ground plane.
- **Q/E radial axis** = `camera_forward = (-sin(phi)sin(theta), sin(phi)cos(theta), -cos(phi))`
  (normalized). At `phi ≈ π/2 - 0.05` (the clamp limit, set at line 731), this
  collapses to approximately `(-sin(theta), cos(theta), 0)` — the same XY
  direction as WASD forward.

So pressing W advances the camera along the same screen direction as
pressing Q. The mechanism differs (W moves target+camera together at fixed
radius; Q moves camera toward fixed target, shrinking radius), but
visually it is the same motion.

### Mouse and wheel paths are unrelated

- **Left-drag pan** (`panCamera`, lines 685–699) uses a theta-based XY
  basis. The `forward` vector matches WASD's, but the `right` vector is
  sign-flipped (`(-cosθ, -sinθ, 0)` vs WASD's `(cosθ, sinθ, 0)`) — the
  deliberate drag-the-map-under-the-cursor convention. Not part of this
  change — see Trade-offs.
- **Right-drag rotate** (lines 720–734) adjusts `theta`/`phi`. Unchanged.
- **Ctrl+wheel zoom** (`onWheel`, lines 736–780) is the only radial-zoom
  path that remains after this change. Its logic is unchanged — it has
  cursor-anchored semantics that depend on the spherical radius — but its
  lead comment (lines 738–739) calls QE "an unmodified zoom path" and
  needs a copy fix once QE stop zooming (see Implementation Step 2).
- **`fitRadiusRef` / `zoomFactorRef`** stay live because the wheel handler
  and the reset-view path both read them. Only the Q/E write path is
  removed.

### Reset and seed paths

- **GLB-camera seed** (lines 581–622) writes `targetRef`, `sphericalRef`,
  `fitRadiusRef`, `zoomFactorRef`. Unaffected.
- **Reset-view** (lines 553–578, triggered by `z` key in
  `src/lib/screen-renderers/cells/MapCell.tsx:337–348`)
  restores spherical state from `initialViewRef`. Unaffected — it doesn't
  touch the per-frame key logic.

### Controls help panel

`MapViewer.tsx:1067-1075`:
```tsx
<div className="font-semibold ...">Controls</div>
<div>Left-click + drag: Pan</div>
<div>Right-click + drag: Rotate</div>
<div>Ctrl + Scroll: Zoom</div>
<div>WASD: Pan</div>
<div>QE: Zoom</div>
<div>Z: Reset view</div>
```

The "WASD: Pan" and "QE: Zoom" lines are wrong after the change and need
updating.

### Tests

No tests exist for `MapViewer.tsx`. The only file in
`analytics-web-app/src/components/map/__tests__/` is `EventDetailPanel.test.tsx`
(unrelated). The repo's test runner is Jest (`package.json:"test": "jest"`),
with `@testing-library/react` for component tests — but r3f scenes are
not unit-tested anywhere in the repo. The route to test coverage here is
to extract the camera-basis math into a pure function and test that.

## Design

### The camera-relative basis

The orbit camera's orientation is fully determined by `theta` and `phi` (the
`spherical` state) plus the `camera.up` reference that
`MapCameraController` writes every frame:
`up_ref = (-sin(theta), cos(theta), 0)` (line 916). Combined with the
`sphericalToZUpOffset` permutation, the resulting orthonormal camera frame
in world coordinates is:

```
right   = ( cos(theta),                       sin(theta),                      0      )
up      = (-cos(phi) * sin(theta),            cos(phi) * cos(theta),           sin(phi))
forward = (-sin(phi) * sin(theta),            sin(phi) * cos(theta),          -cos(phi))
```

Verification (sanity-check at the limits):
- **phi = 0 (top-down)**: `right = (cos θ, sin θ, 0)`, `up = (-sin θ, cos θ, 0)`,
  `forward = (0, 0, -1)`. Camera-up lies in XY (matches `up_ref`), camera-forward
  is straight down. Pressing W moves target perpendicular to theta in XY —
  identical to current top-down map-pan behavior.
- **phi → π/2 (horizontal)**: `right = (cos θ, sin θ, 0)`, `up → (0, 0, 1)`,
  `forward → (-sin θ, cos θ, 0)`. Camera-up becomes world-up; camera-forward
  becomes XY-forward (the same vector that WASD currently uses). Pressing
  W now elevates the camera; pressing Q advances forward. The two are
  orthogonal again.
- **phi = π/4 (45° tilt)**: all three axes have well-defined non-zero
  components; W mixes XY drift with elevation in the proportion the user
  sees on screen.

The three vectors are orthonormal by construction (they are the rows of
the camera's rotation matrix), so no normalization or cross-product is
needed at the call site.

### Pure function extraction

Add a new pure function alongside the existing `sphericalToZUpOffset` /
`zUpOffsetToSphericalInput` helpers (lines 471–487):

```ts
/**
 * Camera-relative orthonormal basis in world coordinates, derived from the
 * orbit's spherical state and the theta-driven camera.up convention used
 * by MapCameraController. Returns the right/up/forward vectors that
 * correspond to screen-X / screen-Y / screen-(-Z) respectively, so a
 * single right-hand-rule binding can drive A/D, W/S, and Q/E onto three
 * mutually-orthogonal world axes at every camera tilt.
 */
// eslint-disable-next-line react-refresh/only-export-components
export function cameraBasisFromSpherical(
  theta: number,
  phi: number,
): { right: THREE.Vector3; up: THREE.Vector3; forward: THREE.Vector3 }
```

The `export` keyword is required because the unit tests in
`__tests__/MapViewer.test.tsx` import this function. The
`eslint-disable` comment suppresses `react-refresh/only-export-components`,
which fires whenever a file with React component exports also exports a
non-component value — the same situation as `ChannelBindingControl` in
`src/lib/screen-renderers/cells/MapCell.tsx:459–460` (an exported
function alongside component exports, with the same disable for the
same reason).

The function is a pure stateless transform — no `useThree`, no refs, no
side effects — so it is unit-testable with a plain Jest test that
constructs `THREE.Vector3` values and asserts component-wise.

### `useFrame` rewrite

Replace the current WASD-XY-pan + QE-radius block (lines 871–910) with a
single basis-driven translation:

```ts
useFrame((_, delta) => {
  if (isHoveredRef.current) {
    const moveSpeed = sphericalRef.current.radius * SPEED_PER_RADIUS * delta
    const { right, up, forward } = cameraBasisFromSpherical(
      sphericalRef.current.theta,
      sphericalRef.current.phi,
    )

    if (keysRef.current.d) targetRef.current.addScaledVector(right, moveSpeed)
    if (keysRef.current.a) targetRef.current.addScaledVector(right, -moveSpeed)
    if (keysRef.current.w) targetRef.current.addScaledVector(up, moveSpeed)
    if (keysRef.current.s) targetRef.current.addScaledVector(up, -moveSpeed)
    if (keysRef.current.q) targetRef.current.addScaledVector(forward, moveSpeed)
    if (keysRef.current.e) targetRef.current.addScaledVector(forward, -moveSpeed)
  }

  const offset = new THREE.Vector3()
  sphericalToZUpOffset(sphericalRef.current, offset)
  camera.position.copy(targetRef.current).add(offset)
  camera.up.set(-Math.sin(sphericalRef.current.theta), Math.cos(sphericalRef.current.theta), 0)
  camera.lookAt(targetRef.current)
})
```

Sign convention (matches the right-hand mnemonic the user described):
- **A** translates target by `-right`; **D** by `+right`.
- **W** by `+up`; **S** by `-up`.
- **Q** by `+forward` (into the scene); **E** by `-forward` (out of the scene).

The `SPEED_PER_RADIUS = 0.5` constant carries over unchanged — the
"one camera-to-target distance per ~2 seconds" feel is preserved.

The `KEY_ZOOM_RATE_PER_SEC` constant and the keyboard branch that wrote
to `zoomFactorRef`/`sphericalRef.radius` are removed. Wheel-driven zoom
keeps both refs current as before.

### What this changes for the user

| Input        | Before                          | After                                       |
|--------------|---------------------------------|---------------------------------------------|
| W / S        | XY pan forward / backward       | Move camera up / down (relative to view)    |
| A / D        | XY strafe left / right          | Strafe left / right (relative to view)      |
| Q / E        | Radial zoom in / out            | Move camera forward / backward (into scene) |
| Ctrl + wheel | Cursor-anchored radial zoom     | Unchanged                                   |
| Left-drag    | XY pan                          | Unchanged                                   |
| Right-drag   | Theta/phi orbit rotate          | Unchanged                                   |
| Z            | Reset view                      | Unchanged                                   |

At `phi = 0` (top-down map view) W still pans the map "north" because
camera-up is in XY at phi=0 — the change is invisible at that angle, which
is the most common usage. At tilted angles, W/S now elevates instead of
sliding along the ground, which is the user's stated mental model.

### Controls help panel update

```tsx
<div>Left-click + drag: Pan</div>
<div>Right-click + drag: Rotate</div>
<div>Ctrl + Scroll: Zoom</div>
<div>W/S: Up / Down</div>
<div>A/D: Strafe</div>
<div>Q/E: Forward / Back</div>
<div>Z: Reset view</div>
```

Right-hand-rule explanation is left out of the panel — too much UI noise
for a feature most users will discover by trial. The pair labels speak
for themselves.

## Implementation Steps

1. **Add `cameraBasisFromSpherical` near the existing helpers**
   (`MapViewer.tsx`, just after `zUpOffsetToSphericalInput` at line 487).
   Doc-comment + body as in the Design section. Pure function. Add
   `export` to the declaration (it must be importable by the test file
   in step 5) along with the
   `// eslint-disable-next-line react-refresh/only-export-components`
   comment shown in the Design section.

2. **Rewrite the `useFrame` body** (lines 871–910). Replace the
   theta-based XY pan and the Q/E radius block with the six basis-driven
   `addScaledVector` calls. Keep the post-WASD `camera.position` /
   `camera.up` / `camera.lookAt` writes (lines 913–917) unchanged — they
   are the steady-state output of the orbit and the new basis goes through
   target translation, not camera writes. The `KEY_ZOOM_RATE_PER_SEC`
   local constant (line 902) disappears as part of this rewrite. Leave
   `fitRadiusRef` and `zoomFactorRef` intact — they remain in use by the
   wheel handler (lines 749–756) and by the reset-view path (lines
   561–566). Also update the stale comment at the top of `onWheel`
   (lines 738–739): it currently reads "QE keys remain an unmodified
   zoom path for users already on the keyboard", which becomes false
   once QE translate along camera-forward instead of zooming. Drop that
   clause; the page-scroll rationale for the Ctrl/Cmd gate stands on its
   own.

3. **Update the controls panel JSX** (lines 1067–1075) to the six lines
   in the Design section.

4. **Update the comment block in `panCamera`** (lines 686–696) to clarify
   that left-drag pan intentionally keeps the theta-XY basis even though
   the keyboard now uses the full camera-relative basis — left-drag
   "feels like dragging the map under the cursor", which is map-pan
   semantics, not fly-cam semantics. See Trade-offs.

5. **Add a test file**
   `analytics-web-app/src/components/map/__tests__/MapViewer.test.tsx`
   that imports the new pure function and exercises the cases listed in
   Testing Strategy. The function is the only export from `MapViewer.tsx`
   that needs testing for this change; the test file is named to leave
   room for future component-level tests if r3f mocking is ever added.

6. **Run `yarn lint` and `yarn test`** from
   `analytics-web-app/`. Both must pass cleanly.

7. **Smoke-check in the browser**:
   - Top-down view (just after load on a typical GLB): W/S/A/D pan the
     map; Q/E push the camera down into / up out of the floor. Same
     visible behavior as before for WASD; Q/E now obviously different
     (vertical, not radial).
   - Right-drag to a near-horizontal view: W elevates, S descends, Q
     advances forward, E reverses. W/S and Q/E are obviously
     orthogonal — the original bug.
   - Right-drag back to a 45° tilt: W mixes XY drift and elevation in
     screen-up direction. Visually intuitive.
   - Ctrl+wheel still zooms radially with cursor-anchored behavior.
   - Z still resets to the GLB-authored view.

## Files to Modify

- `analytics-web-app/src/components/map/MapViewer.tsx` — add
  `cameraBasisFromSpherical`, rewrite `useFrame` keyboard branch, update
  the controls help panel, light comment touch-up in `panCamera`. One
  file.
- `analytics-web-app/src/components/map/__tests__/MapViewer.test.tsx`
  (new) — unit tests for `cameraBasisFromSpherical`.

## Trade-offs

### Why not also rebind `panCamera` (left-drag) to the new basis?

Considered and rejected. Left-drag is "drag the map under the cursor" —
a map-pan idiom where dragging the cursor right slides the visible world
right. Rebinding it to the camera-relative basis would make horizontal
left-drag at high `phi` elevate the camera, which is fly-cam semantics
and would diverge from the intuition every web-map user has. The
user's complaint is specifically about the *keyboard* controls; mouse
drag stays as a separate idiom with intentionally different semantics.
The comment block in `panCamera` is updated to note the deliberate
split.

### Why not keep Q/E as radial zoom and just rebind W/S?

The Q/E collapse happens because the *radial direction itself* aligns
with the WASD XY direction at high `phi`. Rebinding only W/S would force
W/S onto either pure world-Z (orthogonal to radial — works) or onto
camera-up (the proposed scheme — also orthogonal). The right-hand-rule
binding the user described needs all three pairs on three orthogonal
axes, not just two. Going halfway (W/S vertical + Q/E radial) leaves Q/E
on the radial ray, which at low `phi` is approximately world-Z — the same
direction as W/S in this half-version. The pairs collapse again at the
opposite limit. The full-basis rewrite is the only solution that holds
at every tilt.

### Why not introduce a config option to switch between "map pan" and "fly cam"?

Two reasons:

1. **No user signal asks for the old behavior.** The original WASD-XY
   design was an internal call ("keeps WASD as a top-down pan", per the
   removed comment), not a requested feature. The user is the requester
   and has stated the new mental model explicitly.
2. **The new binding is a strict superset at phi=0.** At top-down view
   (the most common starting state), W/S/A/D produce the same screen
   motion as before — camera-up at phi=0 is in XY. Users who only use
   the map in top-down won't notice the change. The behavior only
   diverges when the user has already tilted the camera, which is also
   the state where the old binding was broken.

A config toggle would carry the cost of a new schema field, persistence,
editor UI, and documentation; that cost has no offsetting demand.

### Why a pure function instead of inlining the basis in `useFrame`?

The math is straightforward (six trig evaluations and nine arithmetic
ops), and inlining would compile to identical runtime cost. The pure
function buys testability: r3f scenes aren't unit-tested in this repo,
so the only practical way to add coverage for this change is to extract
the math from the frame loop. The function is also a natural site for
the doc-comment explaining the convention link between the basis and
the right-hand-rule binding.

### `q` for forward vs. `q` for back

The right-hand mnemonic the user gave has the index finger pointing
*forward*, mapping `Q → +forward` (into the scene) and `E → -forward`
(out of the scene). The alternative — `Q → out, E → in` — would match
"E for enter / dive in" but breaks the mnemonic. Going with
`Q → forward` per the user's stated convention.

## Documentation

No project documentation update needed. The map cell has no public
user guide that enumerates the keyboard bindings beyond the in-app help
panel (which is updated in step 4). The notebook documentation in
`mkdocs/docs/` references the Map cell as a visualization type but does
not document camera controls.

If a future tasks/notebook_presentation_plan.md follow-up adds a
screenshot of the controls panel, that asset will need re-capture.
Noted in the plan but out of scope here.

## Testing Strategy

Pure-function unit test in
`analytics-web-app/src/components/map/__tests__/MapViewer.test.tsx`, using
Jest (the repo's runner). Tests import the new
`cameraBasisFromSpherical` and assert on `THREE.Vector3` components.

A small helper at the top of the file keeps the assertions concise:

```ts
function expectVec(v: THREE.Vector3, x: number, y: number, z: number) {
  expect(v.x).toBeCloseTo(x, 10)
  expect(v.y).toBeCloseTo(y, 10)
  expect(v.z).toBeCloseTo(z, 10)
}
```

Coverage:

**Orthonormality (the load-bearing property):**
- For each of `(theta, phi) ∈ {(0, 0), (π/4, π/4), (π/2, π/3), (-π/3, 0.1)}`,
  assert each of `right`, `up`, `forward` has unit length and all three
  pairwise dot products are zero. This is the property that guarantees
  W/S and Q/E never collapse.

**phi = 0 (top-down) — boundary case:**
- `right = (cos θ, sin θ, 0)`, `up = (-sin θ, cos θ, 0)`,
  `forward = (0, 0, -1)`. Spot-check at `theta = 0`, `theta = π/2`.
- Confirms that top-down map-pan behavior is preserved at the most
  common camera angle (camera-up lies in XY, so W moves the target in XY).

**phi = π/2 - 0.05 (near-horizontal — the bug case):**
- At `theta = 0`: `right ≈ (1, 0, 0)`, `up.z ≈ sin(π/2 - 0.05) ≈ 0.9988`,
  `forward.xy ≈ (0, sin(π/2-0.05)) ≈ (0, 0.9988)`, `forward.z ≈ -cos(π/2-0.05)`.
- Confirms that at the clamped horizontal limit, `up` is near world-up
  and `forward` is near XY-forward — these are orthogonal, so W/S and
  Q/E translate the camera along distinct directions. This is the
  case the original bug report names.

**`right` is always horizontal:**
- For any `(theta, phi)`, `right.z === 0`. (Follows from the camera.up
  convention; baked into the formula.) Confirms A/D can never elevate
  the camera, only strafe in the ground plane.

**Right-hand orientation:**
- `up.cross(right)` equals `forward` — equivalently `right.cross(up)`
  equals `-forward`. This is the correct identity for a right-handed
  camera frame where `forward` is the camera's local −Z axis (the view
  direction into the scene), which is the convention used everywhere
  else in the plan. Spot-check at a non-degenerate
  `(theta, phi) = (π/3, π/4)`:
  ```ts
  const crossed = new THREE.Vector3().crossVectors(up, right)
  expectVec(crossed, forward.x, forward.y, forward.z)
  ```
  Use the `expectVec` helper (not `Vector3.equals`) because `equals`
  does strict `===` per component, and the cross-product expansion
  computes the z component as `-cos(phi) * (sin²θ + cos²θ)` while the
  direct `forward.z` is `-cos(phi)` — these differ by ~1 ulp in IEEE
  floats for arbitrary θ, so strict equality would flake. Using
  `crossVectors` (not the mutating `up.cross(right)` form) keeps the
  basis vectors intact for later assertions in the same test. If this
  test fails, a sign got flipped in the formula and the binding is
  reversed (e.g. Q would move backward instead of forward), so it's a
  useful regression guard.

**No NaN at the clamp limits:**
- `cameraBasisFromSpherical(0, 0)` — `sin(0) = 0` everywhere; no
  divisions, no NaN.
- `cameraBasisFromSpherical(0, Math.PI / 2 - 0.05)` — at the upper phi
  clamp; values are finite. (We do not test `phi = π/2` because the orbit
  control clamps below it; if the clamp ever changes, this test will need
  to be revisited along with the controller.)

That is the unit-testable surface. The frame-loop wiring (key events →
`keysRef` → `targetRef.addScaledVector`) is exercised by the manual
smoke checks in Implementation Step 7; mocking r3f's `useFrame` and the
DOM key event handlers is not practical without a much larger test
harness, which is out of scope for this change. If a future plan adds
that harness, the smoke checks listed here are the cases it should
cover first.

## Open Questions

None. The binding convention is locked by the user's right-hand-rule
description; the math follows from the camera.up convention already in
use; the help-panel copy is the only UI-text choice and it's a straight
description of the new behavior.
