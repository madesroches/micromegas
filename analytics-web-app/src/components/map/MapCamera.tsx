/**
 * Orbit/fly camera controller for the map viewer.
 *
 * Extracted from MapViewer.tsx (#1089). Owns the orbit state (target +
 * spherical), GLB-camera seeding, the reset-view restore, the DOM event
 * bindings (mouse drag/rotate, ctrl-wheel zoom, WASDQE), and the per-frame
 * camera placement. The coordinate/pan/zoom math lives in `map-camera-math.ts`.
 */
import { useRef, useEffect, useLayoutEffect, useCallback } from 'react'
import { useThree, useFrame } from '@react-three/fiber'
import * as THREE from 'three'
import {
  cameraBasisFromSpherical,
  panTarget,
  sphericalToZUpOffset,
  zoomAnchorTarget,
  zUpOffsetToSphericalInput,
} from './map-camera-math'

interface MapCameraControllerProps {
  mapBounds: THREE.Box3 | null
  mapScene: THREE.Object3D | null
  resetViewTrigger: number
  glbCamera: THREE.PerspectiveCamera | null
}

export function MapCameraController({
  mapBounds,
  mapScene,
  resetViewTrigger,
  glbCamera,
}: MapCameraControllerProps) {
  const { camera, gl } = useThree()
  const domElement = gl.domElement

  const targetRef = useRef(new THREE.Vector3(0, 0, 0))
  const sphericalRef = useRef(new THREE.Spherical(5000, Math.PI / 4, 0))

  const initialViewRef = useRef<{
    target: THREE.Vector3
    spherical: { radius: number; phi: number; theta: number }
  } | null>(null)

  // WASD speed derives from the current orbit radius so flying feels the same
  // at every zoom level — one camera-to-target distance per ~2 seconds.
  const SPEED_PER_RADIUS = 0.5

  const fitRadiusRef = useRef(5000)
  const zoomFactorRef = useRef(1.0)

  const isLeftMouseDownRef = useRef(false)
  const isLeftDraggingRef = useRef(false)
  const leftMouseStartRef = useRef({ x: 0, y: 0 })
  const isRightMouseDownRef = useRef(false)
  const isHoveredRef = useRef(false)
  const lastMouseRef = useRef({ x: 0, y: 0 })
  const keysRef = useRef({
    w: false,
    a: false,
    s: false,
    d: false,
    q: false,
    e: false,
  })

  // Track latest mapScene through a ref so mousedown can raycast against it
  // without rebinding the DOM event handlers when the scene changes.
  const mapSceneRef = useRef<THREE.Object3D | null>(mapScene)
  useEffect(() => {
    mapSceneRef.current = mapScene
  }, [mapScene])

  const saveInitialView = useCallback(() => {
    initialViewRef.current = {
      target: targetRef.current.clone(),
      spherical: {
        radius: sphericalRef.current.radius,
        phi: sphericalRef.current.phi,
        theta: sphericalRef.current.theta,
      },
    }
  }, [])

  const prevResetViewTriggerRef = useRef(resetViewTrigger)
  useEffect(() => {
    if (resetViewTrigger !== prevResetViewTriggerRef.current && initialViewRef.current) {
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
      // Theta-driven camera.up: at phi=0 the spherical offset is parallel to
      // world-up, so a static (0,0,1) up makes lookAt degenerate. Deriving up
      // from theta keeps lookAt well-defined at every phi and makes theta
      // rotate the screen orientation when looking straight down.
      camera.up.set(-Math.sin(sphericalRef.current.theta), Math.cos(sphericalRef.current.theta), 0)
      camera.lookAt(targetRef.current)
    }
  }, [resetViewTrigger, camera])

  // Seed orbit state from the GLB's embedded camera once it resolves.
  const seededGlbCameraRef = useRef<THREE.PerspectiveCamera | null>(null)
  useLayoutEffect(() => {
    if (!glbCamera || seededGlbCameraRef.current === glbCamera) return
    seededGlbCameraRef.current = glbCamera

    const cameraPos = glbCamera.getWorldPosition(new THREE.Vector3())
    const worldQuat = glbCamera.getWorldQuaternion(new THREE.Quaternion())
    const forward = new THREE.Vector3(0, 0, -1).applyQuaternion(worldQuat)

    let radius: number
    if (mapBounds) {
      const sphere = mapBounds.getBoundingSphere(new THREE.Sphere())
      radius = sphere.radius * 2
    } else {
      radius = Math.max(cameraPos.length(), 1000)
    }

    const target = cameraPos.clone().addScaledVector(forward, radius)
    targetRef.current.copy(target)

    const worldOffset = cameraPos.clone().sub(target)
    const sphericalInput = zUpOffsetToSphericalInput(worldOffset, new THREE.Vector3())
    sphericalRef.current.setFromVector3(sphericalInput)

    fitRadiusRef.current = sphericalRef.current.radius
    zoomFactorRef.current = 1.0

    // Copy intrinsics onto the scene camera.
    const perspCamera = camera as THREE.PerspectiveCamera
    perspCamera.fov = glbCamera.fov
    perspCamera.near = glbCamera.near
    perspCamera.far = glbCamera.far
    perspCamera.updateProjectionMatrix()

    const offset = new THREE.Vector3()
    sphericalToZUpOffset(sphericalRef.current, offset)
    camera.position.copy(targetRef.current).add(offset)
    camera.up.set(-Math.sin(sphericalRef.current.theta), Math.cos(sphericalRef.current.theta), 0)
    camera.lookAt(targetRef.current)

    saveInitialView()
  }, [glbCamera, mapBounds, camera, saveInitialView])

  useEffect(() => {
    const DRAG_THRESHOLD = 4

    // Arm-and-fire flag for the contextmenu that follows a right-mousedown on
    // the canvas. Set on right-mousedown, consumed by the window-level
    // contextmenu handler. A window listener is required because if the
    // mouseup happens off-canvas, the contextmenu fires on whatever element
    // is under the cursor — not the canvas — so a canvas-bound listener
    // would miss it.
    let suppressNextContextMenu = false

    const onMouseDown = (e: MouseEvent) => {
      if (e.button === 0) {
        isLeftMouseDownRef.current = true
        isLeftDraggingRef.current = false
        leftMouseStartRef.current = { x: e.clientX, y: e.clientY }
        lastMouseRef.current = { x: e.clientX, y: e.clientY }
      } else if (e.button === 2) {
        isRightMouseDownRef.current = true
        suppressNextContextMenu = true
        lastMouseRef.current = { x: e.clientX, y: e.clientY }
        domElement.style.cursor = 'grabbing'

        // Re-anchor the orbit pivot to whatever the camera is currently
        // looking at, so right-drag rotates around the visible POI rather
        // than a stale target left over from a previous fit/fly.
        const scene = mapSceneRef.current
        if (scene) {
          const rc = new THREE.Raycaster()
          rc.setFromCamera(new THREE.Vector2(0, 0), camera)
          const hits = rc.intersectObject(scene, true)
          if (hits.length > 0) {
            const newTarget = hits[0].point
            const offset = new THREE.Vector3().copy(camera.position).sub(newTarget)
            const sphericalInput = zUpOffsetToSphericalInput(offset, new THREE.Vector3())
            sphericalRef.current.setFromVector3(sphericalInput)
            // Recompute zoomFactor against the existing scene-sized fitRadius,
            // preserving the invariant `radius = fitRadius * zoomFactor`. We
            // deliberately do not shrink fitRadius here: if we did, the new
            // zoomFactor would land at the upper cap and wheel-out would be
            // blocked when the re-anchor target is close to the camera.
            zoomFactorRef.current = sphericalRef.current.radius / fitRadiusRef.current
            targetRef.current.copy(newTarget)
          }
        }
      }
    }

    const onMouseUp = (e: MouseEvent) => {
      if (e.button === 0) {
        isLeftMouseDownRef.current = false
        if (isLeftDraggingRef.current) {
          domElement.style.cursor = 'auto'
        }
        isLeftDraggingRef.current = false
      } else if (e.button === 2) {
        isRightMouseDownRef.current = false
        domElement.style.cursor = 'auto'
      }
    }

    const onMouseMove = (e: MouseEvent) => {
      const deltaX = e.clientX - lastMouseRef.current.x
      const deltaY = e.clientY - lastMouseRef.current.y
      lastMouseRef.current = { x: e.clientX, y: e.clientY }

      if (isLeftMouseDownRef.current) {
        if (!isLeftDraggingRef.current) {
          const dx = e.clientX - leftMouseStartRef.current.x
          const dy = e.clientY - leftMouseStartRef.current.y
          if (Math.sqrt(dx * dx + dy * dy) > DRAG_THRESHOLD) {
            isLeftDraggingRef.current = true
            domElement.style.cursor = 'grab'
          }
        }
        if (isLeftDraggingRef.current) {
          panTarget(targetRef.current, sphericalRef.current.theta, sphericalRef.current.radius, deltaX, deltaY)
        }
      }

      if (isRightMouseDownRef.current) {
        const rotateSpeed = 0.005
        sphericalRef.current.theta += deltaX * rotateSpeed
        sphericalRef.current.phi += deltaY * rotateSpeed

        // Keep the camera above the horizon — flipping below the map is
        // disorienting and rarely useful for a top-down scene. phi=0
        // (straight down) is allowed since the theta-driven camera.up keeps
        // lookAt well-defined there.
        sphericalRef.current.phi = Math.max(
          0,
          Math.min(Math.PI / 2 - 0.05, sphericalRef.current.phi)
        )
      }
    }

    const onWheel = (e: WheelEvent) => {
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
      const scene = mapSceneRef.current
      if (!scene || oldRadius === 0) return
      const rect = domElement.getBoundingClientRect()
      if (rect.width === 0 || rect.height === 0) return

      const ndc = new THREE.Vector2(
        ((e.clientX - rect.left) / rect.width) * 2 - 1,
        -((e.clientY - rect.top) / rect.height) * 2 + 1
      )
      const rc = new THREE.Raycaster()
      rc.setFromCamera(ndc, camera)
      const hits = rc.intersectObject(scene, true)
      if (hits.length === 0) return

      const anchor = hits[0].point
      const s = newRadius / oldRadius
      zoomAnchorTarget(targetRef.current, anchor, s)
    }

    const onContextMenu = (e: MouseEvent) => {
      if (suppressNextContextMenu) {
        e.preventDefault()
        suppressNextContextMenu = false
      }
    }

    const isFormTarget = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null
      return !!t?.matches('input, textarea, select, [contenteditable="true"]')
    }

    const onKeyDown = (e: KeyboardEvent) => {
      if (isFormTarget(e)) return
      const key = e.key.toLowerCase()
      if (key in keysRef.current) {
        keysRef.current[key as keyof typeof keysRef.current] = true
      }
    }

    const onKeyUp = (e: KeyboardEvent) => {
      // Don't filter form targets on keyup — we need to release any key that
      // got pressed (e.g., focus moved into an input mid-hold).
      const key = e.key.toLowerCase()
      if (key in keysRef.current) {
        keysRef.current[key as keyof typeof keysRef.current] = false
      }
    }

    // Hover gates WASD so multiple Map cells on a page don't all fly together
    // and so flying stops when the pointer leaves the canvas.
    const onMouseEnter = () => {
      isHoveredRef.current = true
    }
    const onMouseLeave = () => {
      isHoveredRef.current = false
      keysRef.current.w = false
      keysRef.current.a = false
      keysRef.current.s = false
      keysRef.current.d = false
      keysRef.current.q = false
      keysRef.current.e = false
    }

    // Safety net: if the browser/tab loses focus mid-drag (alt-tab, OS dialog),
    // the eventual mouseup may never reach us — clear all drag state so the
    // next interaction starts clean.
    const onWindowBlur = () => {
      if (isLeftDraggingRef.current || isRightMouseDownRef.current) {
        domElement.style.cursor = 'auto'
      }
      isLeftMouseDownRef.current = false
      isLeftDraggingRef.current = false
      isRightMouseDownRef.current = false
      suppressNextContextMenu = false
    }

    // mousedown stays on the canvas (drags only start over the map), but
    // mousemove/mouseup live on the window so a drag that sweeps off the
    // canvas keeps tracking and a release-outside isn't lost.
    domElement.addEventListener('mousedown', onMouseDown)
    window.addEventListener('mouseup', onMouseUp)
    window.addEventListener('mousemove', onMouseMove)
    domElement.addEventListener('wheel', onWheel, { passive: false })
    window.addEventListener('contextmenu', onContextMenu)
    domElement.addEventListener('mouseenter', onMouseEnter)
    domElement.addEventListener('mouseleave', onMouseLeave)
    window.addEventListener('keydown', onKeyDown)
    window.addEventListener('keyup', onKeyUp)
    window.addEventListener('blur', onWindowBlur)

    return () => {
      domElement.removeEventListener('mousedown', onMouseDown)
      window.removeEventListener('mouseup', onMouseUp)
      window.removeEventListener('mousemove', onMouseMove)
      domElement.removeEventListener('wheel', onWheel)
      window.removeEventListener('contextmenu', onContextMenu)
      domElement.removeEventListener('mouseenter', onMouseEnter)
      domElement.removeEventListener('mouseleave', onMouseLeave)
      window.removeEventListener('keydown', onKeyDown)
      window.removeEventListener('keyup', onKeyUp)
      window.removeEventListener('blur', onWindowBlur)
      // Restore cursor on unmount in case we tear down mid-drag — the
      // {ready} gate in MapViewer unmounts this controller on mapUrl
      // changes, and a mouseup we'd otherwise rely on never fires.
      domElement.style.cursor = 'auto'
    }
  }, [camera, domElement])

  useFrame((_, delta) => {
    if (isHoveredRef.current) {
      const moveSpeed = sphericalRef.current.radius * SPEED_PER_RADIUS * delta

      // Single camera-relative orthonormal basis: A/D strafe along camera-right,
      // W/S along camera-up, Q/E along camera-forward. The three axes are
      // perpendicular at every tilt, so no two key pairs can collapse onto the
      // same world direction (the high-phi bug the previous mixed scheme had).
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

  return null
}
