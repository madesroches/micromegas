/**
 * Pure camera math for the map viewer's orbit/fly controller.
 *
 * Extracted from MapViewer.tsx (#1089). These functions operate only on the
 * THREE vector/spherical objects passed in (no React, no module state), so they
 * are unit-testable. The scene is Z-up; THREE.Spherical is hard-coded Y-up, so
 * the conversion helpers permute axes between the two conventions.
 */
import * as THREE from 'three'

/**
 * Reinterpret a Y-up offset (decoded from THREE.Spherical) as a Z-up world offset.
 * THREE.Spherical is hard-coded Y-up: phi=0 places the offset along +Y. In a Z-up
 * scene we want phi=0 to mean "world up" (+Z), so permute (x, y, z) → (x, -z, y).
 */
export function sphericalToZUpOffset(spherical: THREE.Spherical, out: THREE.Vector3): THREE.Vector3 {
  out.setFromSpherical(spherical)
  const tmp = out.y
  out.y = -out.z
  out.z = tmp
  return out
}

/**
 * Inverse of sphericalToZUpOffset: convert a Z-up world offset back into a vector
 * suitable for THREE.Spherical.setFromVector3 (which expects Y-up). Permute
 * (x, y, z) → (x, z, -y).
 */
export function zUpOffsetToSphericalInput(offset: THREE.Vector3, out: THREE.Vector3): THREE.Vector3 {
  out.set(offset.x, offset.z, -offset.y)
  return out
}

/**
 * Camera-relative orthonormal basis in world coordinates, derived from the
 * orbit's spherical state and the theta-driven camera.up convention used
 * by MapCameraController. Returns the right/up/forward vectors that
 * correspond to screen-X / screen-Y / screen-(-Z) respectively, so a
 * single right-hand-rule binding can drive A/D, W/S, and Q/E onto three
 * mutually-orthogonal world axes at every camera tilt.
 */
export function cameraBasisFromSpherical(
  theta: number,
  phi: number,
): { right: THREE.Vector3; up: THREE.Vector3; forward: THREE.Vector3 } {
  const sinTheta = Math.sin(theta)
  const cosTheta = Math.cos(theta)
  const sinPhi = Math.sin(phi)
  const cosPhi = Math.cos(phi)
  return {
    right: new THREE.Vector3(cosTheta, sinTheta, 0),
    up: new THREE.Vector3(-cosPhi * sinTheta, cosPhi * cosTheta, sinPhi),
    forward: new THREE.Vector3(-sinPhi * sinTheta, sinPhi * cosTheta, -cosPhi),
  }
}

/**
 * Left-drag pan: translate the orbit `target` in the ground plane.
 *
 * Left-drag intentionally keeps a theta-based XY basis even though the keyboard
 * uses the full camera-relative basis (cameraBasisFromSpherical). Left-drag is
 * "drag the map under the cursor" — a ground-plane map-pan idiom — so horizontal
 * drag must never elevate the camera, unlike WASD's fly-cam semantics. Deriving
 * the basis from theta directly (not camera.getWorldDirection) also avoids the
 * cross-product collapse at phi=0, where cross(worldUp, cameraForward) is zero
 * and would silently drop the input. Theta is well-defined at every phi.
 *
 * Mutates and returns `target`. `panSpeed` is the world-units-per-pixel
 * translation rate; each mode computes it from its own camera state
 * (perspective: `radius * 0.001`; orthographic: `1 / camera.zoom`).
 */
export function panTarget(
  target: THREE.Vector3,
  theta: number,
  panSpeed: number,
  deltaX: number,
  deltaY: number,
): THREE.Vector3 {
  const sinTheta = Math.sin(theta)
  const cosTheta = Math.cos(theta)
  const right = new THREE.Vector3(-cosTheta, -sinTheta, 0)
  const forward = new THREE.Vector3(-sinTheta, cosTheta, 0)
  target.addScaledVector(right, deltaX * panSpeed)
  target.addScaledVector(forward, deltaY * panSpeed)
  return target
}

/**
 * Cursor-anchored zoom: scale the orbit `target` around the world point under
 * the cursor (`anchor`) by `s = newRadius / oldRadius`. Because only the orbit
 * radius scaled by the same `s`, the cursor's world hit point stays at the same
 * screen location across the zoom step. Mutates and returns `target`.
 */
export function zoomAnchorTarget(
  target: THREE.Vector3,
  anchor: THREE.Vector3,
  s: number,
): THREE.Vector3 {
  return target.sub(anchor).multiplyScalar(s).add(anchor)
}
