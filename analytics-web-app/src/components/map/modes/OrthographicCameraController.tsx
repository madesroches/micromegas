/**
 * Orthographic-camera orbit/fly controller. Typed against
 * THREE.OrthographicCamera — reads/writes `.zoom` directly. Drei's
 * <OrthographicCamera> auto-fits left/right/top/bottom to the canvas
 * viewport, so the controller never manages the frustum; the user-perceived
 * zoom lives entirely on `camera.zoom`.
 *
 * Shares the orbit refs, DOM wiring, and per-frame pose with the perspective
 * controller via `useMapOrbitController`. Diffs from perspective: the GLB
 * seed computes a `camera.zoom` matching perspective's height-fit; the wheel
 * is cursor-anchored on `camera.zoom` (orbit radius is irrelevant to ortho
 * projection); pan/fly speeds are computed live from camera intrinsics.
 */
import { useEffect, useLayoutEffect, useRef } from 'react'
import { useThree } from '@react-three/fiber'
import * as THREE from 'three'
import type { RefObject } from 'react'
import {
  sphericalToZUpOffset,
  zoomAnchorTarget,
  zUpOffsetToSphericalInput,
} from '../map-camera-math'
import { useMapOrbitController } from '../hooks/useMapOrbitController'
import type { MapModeRenderProps } from './types'

// Half the visible world-height per second of fly travel at full hold — the
// same dimensionless ratio perspective uses (half the orbit radius per second).
const FLY_SPEED = 0.5
const ZOOM_SPEED = 0.1
// Wide absolute clamp so the user can zoom far in/out without the step
// snapping; in practice never reached at sane scene scales.
const ZOOM_MIN = 1e-6
const ZOOM_MAX = 1e6

/**
 * Seed `camera.zoom` so the initial ortho framing visually matches what the
 * perspective camera shows at orbit distance `radius`. In perspective the
 * world height visible at distance R is `2 * R * tan(vFov/2)`; an ortho
 * camera spanning `hPx` pixels over that same world height has
 * `zoom = hPx / worldHeight`. `vFov` is in degrees (THREE convention).
 */
// eslint-disable-next-line react-refresh/only-export-components
export function computeOrthoSeedZoom(
  glbCamera: { fov: number },
  radius: number,
  hPx: number,
): number {
  const worldHeight = 2 * radius * Math.tan(THREE.MathUtils.degToRad(glbCamera.fov) / 2)
  return hPx / worldHeight
}

/**
 * Wheel zoom multiplier. `deltaY > 0` zooms out (multiplier < 1), matching
 * the perspective UX. The orbit target is then translated by
 * `zoomAnchorTarget(target, anchor, 1 / m)` to keep the cursor's world point
 * fixed on screen.
 */
// eslint-disable-next-line react-refresh/only-export-components
export function computeOrthoZoomMultiplier(deltaY: number): number {
  return deltaY > 0 ? 1 / (1 + ZOOM_SPEED) : 1 + ZOOM_SPEED
}

interface OrthographicCameraControllerProps extends MapModeRenderProps {
  cameraRef: RefObject<THREE.OrthographicCamera>
}

export function OrthographicCameraController({
  cameraRef,
  glbCamera,
  mapScene,
  mapBounds,
  resetViewTrigger,
}: OrthographicCameraControllerProps) {
  const { gl } = useThree()
  const domElement = gl.domElement

  // mapScene is fixed for the controller's mount lifetime (MapViewer keys the
  // mode on mapUrl), but the hook and handlers read it through a ref.
  const mapSceneRef = useRef<THREE.Object3D>(mapScene)
  mapSceneRef.current = mapScene

  const { targetRef, sphericalRef } = useMapOrbitController<THREE.OrthographicCamera>({
    cameraRef,
    mapSceneRef,
    domElement,
    // Pan: exact world-per-pixel for ortho, since drei's auto-fit makes
    // top - bottom = canvas height in pixels.
    getPanSpeed: (): number => {
      const camera = cameraRef.current
      return camera ? 1 / camera.zoom : 0
    },
    // Fly: half the visible world-height per second at FLY_SPEED.
    getFlyMoveSpeedPerFrame: (delta: number): number => {
      const camera = cameraRef.current
      if (!camera) return 0
      return ((camera.top - camera.bottom) / camera.zoom) * FLY_SPEED * delta
    },
    // No zoom invariant to maintain — ortho zoom lives on camera.zoom, not radius.
    onWheel: (e) => {
      const camera = cameraRef.current
      if (!camera) return
      // Gate zoom on Ctrl/Cmd so plain wheel scrolls the surrounding page
      // (notebooks embed Map cells in a scrollable container).
      if (!e.ctrlKey && !e.metaKey) return
      e.preventDefault()

      const m = computeOrthoZoomMultiplier(e.deltaY)
      const newZoom = Math.max(ZOOM_MIN, Math.min(ZOOM_MAX, camera.zoom * m))

      // Cursor-anchored zoom on camera.zoom. Raycast the cursor against the
      // scene; translate the orbit target to keep that world point fixed on
      // screen: target = a + (target - a) / m, i.e. zoomAnchorTarget(s = 1/m).
      const rect = domElement.getBoundingClientRect()
      if (rect.width > 0 && rect.height > 0) {
        const ndc = new THREE.Vector2(
          ((e.clientX - rect.left) / rect.width) * 2 - 1,
          -((e.clientY - rect.top) / rect.height) * 2 + 1
        )
        const rc = new THREE.Raycaster()
        rc.setFromCamera(ndc, camera)
        const hits = rc.intersectObject(mapSceneRef.current, true)
        if (hits.length > 0) {
          zoomAnchorTarget(targetRef.current, hits[0].point, 1 / m)
        }
      }

      camera.zoom = newZoom
      camera.updateProjectionMatrix()
    },
  })

  const initialViewRef = useRef<{
    target: THREE.Vector3
    spherical: { radius: number; phi: number; theta: number }
    zoom: number
  } | null>(null)

  // Seed orbit state + camera.zoom from the GLB's embedded camera. The
  // MapViewer gate remounts this controller per mapUrl (key={mapUrl}), so
  // glbCamera identity is fixed for the mount lifetime — runs once.
  useLayoutEffect(() => {
    const camera = cameraRef.current
    if (!camera) return

    const cameraPos = glbCamera.getWorldPosition(new THREE.Vector3())
    const worldQuat = glbCamera.getWorldQuaternion(new THREE.Quaternion())
    const forward = new THREE.Vector3(0, 0, -1).applyQuaternion(worldQuat)

    const sphere = mapBounds.getBoundingSphere(new THREE.Sphere())
    const radius = sphere.radius * 2

    const target = cameraPos.clone().addScaledVector(forward, radius)
    targetRef.current.copy(target)

    const worldOffset = cameraPos.clone().sub(target)
    const sphericalInput = zUpOffsetToSphericalInput(worldOffset, new THREE.Vector3())
    sphericalRef.current.setFromVector3(sphericalInput)

    // Drei applies left/right/top/bottom on the underlying primitive during
    // commit (before this useLayoutEffect), so camera.top/bottom are populated.
    const hPx = camera.top - camera.bottom
    camera.near = glbCamera.near
    camera.far = glbCamera.far
    camera.zoom = computeOrthoSeedZoom(glbCamera, sphericalRef.current.radius, hPx)
    // Drei's own useLayoutEffect for the frustum fires before this one and its
    // useFrame is a no-op without functional children, so without this call
    // the first paint would use the stale default projection.
    camera.updateProjectionMatrix()

    const offset = new THREE.Vector3()
    sphericalToZUpOffset(sphericalRef.current, offset)
    camera.position.copy(targetRef.current).add(offset)
    camera.up.set(-Math.sin(sphericalRef.current.theta), Math.cos(sphericalRef.current.theta), 0)
    camera.lookAt(targetRef.current)

    initialViewRef.current = {
      target: targetRef.current.clone(),
      spherical: {
        radius: sphericalRef.current.radius,
        phi: sphericalRef.current.phi,
        theta: sphericalRef.current.theta,
      },
      zoom: camera.zoom,
    }
  }, [glbCamera, mapBounds, cameraRef, targetRef, sphericalRef])

  // Reset view on Z: restore orbit refs *and* camera.zoom. The seed runs
  // synchronously at mount, so initialViewRef is populated before any
  // resetViewTrigger change fires.
  const prevResetViewTriggerRef = useRef(resetViewTrigger)
  useEffect(() => {
    const camera = cameraRef.current
    if (resetViewTrigger !== prevResetViewTriggerRef.current && initialViewRef.current && camera) {
      prevResetViewTriggerRef.current = resetViewTrigger
      targetRef.current.copy(initialViewRef.current.target)
      sphericalRef.current.radius = initialViewRef.current.spherical.radius
      sphericalRef.current.phi = initialViewRef.current.spherical.phi
      sphericalRef.current.theta = initialViewRef.current.spherical.theta
      camera.zoom = initialViewRef.current.zoom
      camera.updateProjectionMatrix()

      const offset = new THREE.Vector3()
      sphericalToZUpOffset(sphericalRef.current, offset)
      camera.position.copy(targetRef.current).add(offset)
      camera.up.set(-Math.sin(sphericalRef.current.theta), Math.cos(sphericalRef.current.theta), 0)
      camera.lookAt(targetRef.current)
    }
  }, [resetViewTrigger, cameraRef, targetRef, sphericalRef])

  return null
}
