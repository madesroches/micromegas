/**
 * Map camera "mode" interface: a kind tag plus a React component that mounts
 * a camera element and its controller as a wired pair. The interface exists
 * only so `MapViewer` can switch on `cameraKind` and `MapCell`'s editor
 * dropdown can enumerate choices. Each mode owns a self-contained, typed
 * controller — there is no shared abstract camera interface to evolve.
 */
import type { ComponentType } from 'react'
import type * as THREE from 'three'

export type MapModeKind = 'perspective' | 'orthographic'

export interface MapModeRenderProps {
  // All three are non-nullable: `MapViewer` only mounts a mode when the GLB
  // has arrived *and* includes an embedded camera. `mapScene`, `mapBounds`,
  // and `glbCamera` are set together in `handleMapLoaded`, so the gate checks
  // all three inline. A missing camera is a contract violation that surfaces
  // as a red error banner with no map content rendered.
  glbCamera: THREE.PerspectiveCamera | THREE.OrthographicCamera
  mapScene: THREE.Object3D
  mapBounds: THREE.Box3
  resetViewTrigger: number
}

export interface MapMode {
  kind: MapModeKind
  /** Mounts the camera element and its controller as a wired pair. */
  Render: ComponentType<MapModeRenderProps>
}

export const MAP_MODE_LABELS: Record<MapModeKind, string> = {
  perspective: 'Perspective',
  orthographic: 'Orthographic',
}
