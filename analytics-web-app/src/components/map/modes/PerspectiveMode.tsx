/**
 * Perspective mode: mounts a drei <PerspectiveCamera> and its controller as
 * a wired pair. This is the original map-viewer behavior (fov=60).
 */
import { useRef } from 'react'
import { PerspectiveCamera } from '@react-three/drei'
import * as THREE from 'three'
import type { MapMode, MapModeRenderProps } from './types'
import { PerspectiveCameraController } from './PerspectiveCameraController'

// eslint-disable-next-line react-refresh/only-export-components
function PerspectiveModeRender({
  glbCamera,
  mapScene,
  mapBounds,
  resetViewTrigger,
}: MapModeRenderProps) {
  const cameraRef = useRef<THREE.PerspectiveCamera>(null!)
  return (
    <>
      <PerspectiveCamera ref={cameraRef} makeDefault fov={60} near={1} far={100000} />
      <PerspectiveCameraController
        cameraRef={cameraRef}
        glbCamera={glbCamera}
        mapScene={mapScene}
        mapBounds={mapBounds}
        resetViewTrigger={resetViewTrigger}
      />
    </>
  )
}

export const perspectiveMode: MapMode = {
  kind: 'perspective',
  Render: PerspectiveModeRender,
}
