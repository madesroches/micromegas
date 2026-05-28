import * as THREE from 'three'
import {
  computeOrthoSeedZoom,
  computeOrthoZoomMultiplier,
} from '../modes/OrthographicCameraController'
import { zoomAnchorTarget } from '../map-camera-math'

describe('computeOrthoSeedZoom', () => {
  it('matches the perspective height-fit at distance R', () => {
    const glbCamera = { fov: 60, near: 1, far: 100000 }
    const radius = 4000
    const hPx = 800
    const zoom = computeOrthoSeedZoom(glbCamera, radius, hPx)
    const expected = hPx / (2 * radius * Math.tan(THREE.MathUtils.degToRad(60) / 2))
    expect(zoom).toBeCloseTo(expected, 9)
  })

  it('halving the visible world height doubles the zoom', () => {
    const glbCamera = { fov: 45 }
    const hPx = 1000
    const near = computeOrthoSeedZoom(glbCamera, 2000, hPx)
    const far = computeOrthoSeedZoom(glbCamera, 1000, hPx)
    expect(far).toBeCloseTo(near * 2, 9)
  })
})

describe('computeOrthoZoomMultiplier', () => {
  it('zooms out for deltaY > 0 and in for deltaY < 0', () => {
    expect(computeOrthoZoomMultiplier(1)).toBeLessThan(1)
    expect(computeOrthoZoomMultiplier(-1)).toBeGreaterThan(1)
  })

  it('zoom-in and zoom-out steps are reciprocal', () => {
    expect(computeOrthoZoomMultiplier(1) * computeOrthoZoomMultiplier(-1)).toBeCloseTo(1, 9)
  })
})

describe('ortho cursor-anchored wheel zoom', () => {
  // Build a real ortho camera so we can project the anchor and assert it
  // stays under the cursor across a zoom step. The frustum mirrors drei's
  // auto-fit: left/right/top/bottom span the canvas in pixels.
  function makeCamera(zoom: number): THREE.OrthographicCamera {
    const W = 800
    const H = 600
    const cam = new THREE.OrthographicCamera(-W / 2, W / 2, H / 2, -H / 2, 1, 100000)
    cam.zoom = zoom
    cam.updateProjectionMatrix()
    return cam
  }

  // Place the camera at target + a fixed offset (ortho projection is
  // distance-invariant, so the offset only sets orientation), look at target.
  function placeCamera(cam: THREE.OrthographicCamera, target: THREE.Vector3) {
    const offset = new THREE.Vector3(1500, -2000, 3000)
    cam.position.copy(target).add(offset)
    cam.up.set(0, 0, 1)
    cam.lookAt(target)
    cam.updateMatrixWorld(true)
  }

  it('keeps the anchor world point at the same NDC across the zoom step', () => {
    const target = new THREE.Vector3(120, -50, 0)
    const anchor = new THREE.Vector3(300, 80, 40) // off-center cursor hit

    const cam = makeCamera(0.25)
    placeCamera(cam, target)
    const before = anchor.clone().project(cam)

    // Apply the ortho wheel step: zoom in (deltaY < 0).
    const m = computeOrthoZoomMultiplier(-1)
    zoomAnchorTarget(target, anchor, 1 / m)
    cam.zoom = cam.zoom * m
    cam.updateProjectionMatrix()
    placeCamera(cam, target)
    const after = anchor.clone().project(cam)

    expect(after.x).toBeCloseTo(before.x, 6)
    expect(after.y).toBeCloseTo(before.y, 6)
  })
})
