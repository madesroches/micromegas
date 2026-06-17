/**
 * Perspective-camera orbit/fly controller. Typed against
 * THREE.PerspectiveCamera — reads/writes `fov` directly with no cast and no
 * `useThree().camera`. Shares the orbit refs, DOM wiring, and per-frame pose
 * with the orthographic controller via `useMapOrbitController`; owns the
 * perspective-specific GLB seed, the radius-driven cursor-anchored wheel
 * zoom, and the `radius = fitRadius * zoomFactor` invariant.
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

// WASD speed derives from the current orbit radius so flying feels the same
// at every zoom level — one camera-to-target distance per ~2 seconds.
const SPEED_PER_RADIUS = 0.5

interface PerspectiveCameraControllerProps extends MapModeRenderProps {
  cameraRef: RefObject<THREE.PerspectiveCamera>
}

export function PerspectiveCameraController({
  cameraRef,
  glbCamera,
  mapScene,
  mapBounds,
  resetViewTrigger,
}: PerspectiveCameraControllerProps) {
  const { gl } = useThree()
  const domElement = gl.domElement

  const fitRadiusRef = useRef(5000)
  const zoomFactorRef = useRef(1.0)

  // mapScene is fixed for the controller's mount lifetime (MapViewer keys the
  // mode on mapUrl), but the hook and handlers read it through a ref.
  const mapSceneRef = useRef<THREE.Object3D>(mapScene)
  mapSceneRef.current = mapScene

  const { targetRef, sphericalRef } = useMapOrbitController<THREE.PerspectiveCamera>({
    cameraRef,
    mapSceneRef,
    domElement,
    getPanSpeed: (): number => sphericalRef.current.radius * 0.001,
    getFlyMoveSpeedPerFrame: (delta: number): number => sphericalRef.current.radius * SPEED_PER_RADIUS * delta,
    onRightDragReAnchor: () => {
      // Recompute zoomFactor against the existing scene-sized fitRadius,
      // preserving the invariant `radius = fitRadius * zoomFactor`. We
      // deliberately do not shrink fitRadius here: if we did, the new
      // zoomFactor would land at the upper cap and wheel-out would be
      // blocked when the re-anchor target is close to the camera.
      zoomFactorRef.current = sphericalRef.current.radius / fitRadiusRef.current
    },
    onWheel: (e) => {
      const camera = cameraRef.current
      if (!camera) return
      // Gate zoom on Ctrl/Cmd so plain wheel scrolls the surrounding page
      // (notebooks embed Map cells in a scrollable container).
      if (!e.ctrlKey && !e.metaKey) return
      e.preventDefault()

      const zoomSpeed = 0.1
      const zoomMultiplier = e.deltaY > 0 ? (1 + zoomSpeed) : (1 - zoomSpeed)
      // Wide clamp range so re-anchored zoomFactors (which can land far
      // outside [0.01, 1.0] when the new orbit pivot is close to the camera)
      // don't cause a single-step snap on the next wheel event, and so the
      // user can zoom out past the initial scene fit if they want.
      const newZoomFactor = Math.max(
        0.001,
        Math.min(10.0, zoomFactorRef.current * zoomMultiplier)
      )
      const oldRadius = sphericalRef.current.radius
      const newRadius = fitRadiusRef.current * newZoomFactor
      zoomFactorRef.current = newZoomFactor
      sphericalRef.current.radius = newRadius

      // Cursor-anchored zoom. Scale the orbit target around the world point
      // under the cursor; the camera follows in useFrame as
      // `target + sphericalOffset`. Because we only scaled the spherical
      // radius by s = newRadius/oldRadius, the cursor's world hit point
      // stays at the same screen location across the zoom step.
      if (oldRadius === 0) return
      const rect = domElement.getBoundingClientRect()
      if (rect.width === 0 || rect.height === 0) return

      const ndc = new THREE.Vector2(
        ((e.clientX - rect.left) / rect.width) * 2 - 1,
        -((e.clientY - rect.top) / rect.height) * 2 + 1
      )
      const rc = new THREE.Raycaster()
      rc.setFromCamera(ndc, camera)
      const hits = rc.intersectObject(mapSceneRef.current, true)
      if (hits.length === 0) return

      const anchor = hits[0].point
      const s = newRadius / oldRadius
      zoomAnchorTarget(targetRef.current, anchor, s)
    },
  })

  const initialViewRef = useRef<{
    target: THREE.Vector3
    spherical: { radius: number; phi: number; theta: number }
  } | null>(null)

  // Seed orbit state from the GLB's embedded camera. The MapViewer gate
  // remounts this controller per mapUrl (key={mapUrl}), so glbCamera identity
  // is fixed for the mount lifetime — the effect runs once, no re-seed guard.
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

    fitRadiusRef.current = sphericalRef.current.radius
    zoomFactorRef.current = 1.0

    // Copy intrinsics onto the scene camera.
    if (glbCamera instanceof THREE.PerspectiveCamera) {
      camera.fov = glbCamera.fov
    }
    camera.near = glbCamera.near
    camera.far = glbCamera.far
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
    }
  }, [glbCamera, mapBounds, cameraRef, targetRef, sphericalRef])

  // Reset view on Z. The seed runs synchronously at mount via useLayoutEffect,
  // so initialViewRef is populated before any resetViewTrigger change fires.
  const prevResetViewTriggerRef = useRef(resetViewTrigger)
  useEffect(() => {
    const camera = cameraRef.current
    if (resetViewTrigger !== prevResetViewTriggerRef.current && initialViewRef.current && camera) {
      prevResetViewTriggerRef.current = resetViewTrigger
      targetRef.current.copy(initialViewRef.current.target)
      sphericalRef.current.radius = initialViewRef.current.spherical.radius
      sphericalRef.current.phi = initialViewRef.current.spherical.phi
      sphericalRef.current.theta = initialViewRef.current.spherical.theta
      zoomFactorRef.current = 1.0
      // Restore fitRadius alongside the spherical state. The GLB seed set
      // fitRadius equal to the initial spherical.radius; without restoring
      // it, a stale fitRadius left over from a prior re-anchor would
      // cause the next wheel event to snap radius to fitRadius * zoomFactor.
      fitRadiusRef.current = initialViewRef.current.spherical.radius

      const offset = new THREE.Vector3()
      sphericalToZUpOffset(sphericalRef.current, offset)
      camera.position.copy(targetRef.current).add(offset)
      camera.up.set(-Math.sin(sphericalRef.current.theta), Math.cos(sphericalRef.current.theta), 0)
      camera.lookAt(targetRef.current)
    }
  }, [resetViewTrigger, cameraRef, targetRef, sphericalRef])

  return null
}
