/**
 * Shared orbit/fly controller logic for the map viewer's camera modes.
 *
 * Owns the mode-agnostic pieces both the perspective and orthographic
 * controllers need: the orbit refs (target + spherical), the DOM event
 * wiring (mouse drag/rotate, ctrl-wheel, WASDQE, window-blur cleanup), and
 * the per-frame camera placement. Mode-specific behavior is injected through
 * callbacks (`onWheel`, `getPanSpeed`, `getFlyMoveSpeedPerFrame`,
 * `onRightDragReAnchor`). The coordinate/pan math lives in
 * `map-camera-math.ts`.
 *
 * The hook is mode-agnostic: it never touches `fov`/`zoom`/`fitRadius` — the
 * GLB-seed, reset-view, and zoom-invariant state stay in each per-mode
 * controller, which reads/writes the orbit refs returned here.
 */
import { useRef, useEffect } from 'react'
import { useFrame } from '@react-three/fiber'
import * as THREE from 'three'
import type { RefObject } from 'react'
import {
  cameraBasisFromSpherical,
  panTarget,
  sphericalToZUpOffset,
  zUpOffsetToSphericalInput,
} from '../map-camera-math'

interface UseMapOrbitControllerParams<
  C extends THREE.PerspectiveCamera | THREE.OrthographicCamera,
> {
  cameraRef: RefObject<C>
  mapSceneRef: RefObject<THREE.Object3D>
  domElement: HTMLElement | null
  onWheel: (e: WheelEvent) => void
  getPanSpeed: () => number
  getFlyMoveSpeedPerFrame: (delta: number) => number
  onRightDragReAnchor?: () => void
  // When provided, Q/E drive this instead of forward/back translation. Used by
  // ortho, where moving along the view axis is a no-op (distance-invariant
  // projection), so Q/E zoom the camera instead. `direction` is +1 for Q
  // (zoom in / forward) and -1 for E (zoom out / back).
  onFlyZoom?: (delta: number, direction: 1 | -1) => void
}

export function useMapOrbitController<
  C extends THREE.PerspectiveCamera | THREE.OrthographicCamera,
>({
  cameraRef,
  mapSceneRef,
  domElement,
  onWheel,
  getPanSpeed,
  getFlyMoveSpeedPerFrame,
  onRightDragReAnchor,
  onFlyZoom,
}: UseMapOrbitControllerParams<C>): {
  targetRef: RefObject<THREE.Vector3>
  sphericalRef: RefObject<THREE.Spherical>
} {
  const targetRef = useRef(new THREE.Vector3(0, 0, 0))
  const sphericalRef = useRef(new THREE.Spherical(5000, Math.PI / 4, 0))

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

  // Latest-callback refs so the DOM-binding effect can key on
  // [domElement, camera] rather than rebinding every render as callback
  // identities change. The useFrame body reads the latest from the same refs.
  const onWheelRef = useRef(onWheel)
  onWheelRef.current = onWheel
  const getPanSpeedRef = useRef(getPanSpeed)
  getPanSpeedRef.current = getPanSpeed
  const getFlyMoveSpeedPerFrameRef = useRef(getFlyMoveSpeedPerFrame)
  getFlyMoveSpeedPerFrameRef.current = getFlyMoveSpeedPerFrame
  const onRightDragReAnchorRef = useRef(onRightDragReAnchor)
  onRightDragReAnchorRef.current = onRightDragReAnchor
  const onFlyZoomRef = useRef(onFlyZoom)
  onFlyZoomRef.current = onFlyZoom

  useEffect(() => {
    // Read the camera inside the effect: refs are attached during commit
    // (before any effect fires), so the sibling camera's ref is populated
    // here even though it reads null during render. The camera identity is
    // fixed for the controller's mount lifetime (MapViewer remounts the
    // camera/controller pair on mapUrl or cameraKind change), so binding
    // once is correct.
    const camera = cameraRef.current
    if (!camera || !domElement) return

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
        const rc = new THREE.Raycaster()
        rc.setFromCamera(new THREE.Vector2(0, 0), camera)
        const hits = rc.intersectObject(scene, true)
        if (hits.length > 0) {
          const newTarget = hits[0].point
          const offset = new THREE.Vector3().copy(camera.position).sub(newTarget)
          const sphericalInput = zUpOffsetToSphericalInput(offset, new THREE.Vector3())
          sphericalRef.current.setFromVector3(sphericalInput)
          targetRef.current.copy(newTarget)
          // Let the mode preserve any zoom invariant derived from the new
          // radius (perspective recomputes its zoomFactor; ortho omits this).
          onRightDragReAnchorRef.current?.()
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
          panTarget(targetRef.current, sphericalRef.current.theta, getPanSpeedRef.current(), deltaX, deltaY)
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

    const onWheelHandler = (e: WheelEvent) => {
      onWheelRef.current(e)
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
    domElement.addEventListener('wheel', onWheelHandler, { passive: false })
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
      domElement.removeEventListener('wheel', onWheelHandler)
      window.removeEventListener('contextmenu', onContextMenu)
      domElement.removeEventListener('mouseenter', onMouseEnter)
      domElement.removeEventListener('mouseleave', onMouseLeave)
      window.removeEventListener('keydown', onKeyDown)
      window.removeEventListener('keyup', onKeyUp)
      window.removeEventListener('blur', onWindowBlur)
      // Restore cursor on unmount in case we tear down mid-drag — the
      // gate in MapViewer unmounts this controller on mapUrl changes, and a
      // mouseup we'd otherwise rely on never fires.
      domElement.style.cursor = 'auto'
    }
  }, [cameraRef, domElement, mapSceneRef])

  useFrame((_, delta) => {
    const camera = cameraRef.current
    if (!camera) return

    if (isHoveredRef.current) {
      const moveSpeed = getFlyMoveSpeedPerFrameRef.current(delta)

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

      // Q/E: a mode can take over the depth axis (ortho zooms, since forward
      // translation is invisible under an orthographic projection); otherwise
      // they fly the target forward/back along camera-forward.
      const flyZoom = onFlyZoomRef.current
      if (flyZoom) {
        if (keysRef.current.q) flyZoom(delta, 1)
        if (keysRef.current.e) flyZoom(delta, -1)
      } else {
        if (keysRef.current.q) targetRef.current.addScaledVector(forward, moveSpeed)
        if (keysRef.current.e) targetRef.current.addScaledVector(forward, -moveSpeed)
      }
    }

    const offset = new THREE.Vector3()
    sphericalToZUpOffset(sphericalRef.current, offset)
    camera.position.copy(targetRef.current).add(offset)
    // Theta-driven camera.up: at phi=0 the spherical offset is parallel to
    // world-up, so a static (0,0,1) up makes lookAt degenerate. Deriving up
    // from theta keeps lookAt well-defined at every phi and makes theta
    // rotate the screen orientation when looking straight down.
    camera.up.set(-Math.sin(sphericalRef.current.theta), Math.cos(sphericalRef.current.theta), 0)
    camera.lookAt(targetRef.current)
  })

  return { targetRef, sphericalRef }
}
