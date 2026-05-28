/**
 * Orthographic mode: mounts a drei <OrthographicCamera> and its controller as
 * a wired pair. Drei auto-fits the frustum to the canvas viewport and re-fits
 * on resize, so the controller only manages camera.zoom.
 */
import { useRef } from 'react'
import { OrthographicCamera } from '@react-three/drei'
import * as THREE from 'three'
import type { MapMode, MapModeRenderProps } from './types'
import { OrthographicCameraController } from './OrthographicCameraController'

// eslint-disable-next-line react-refresh/only-export-components
function OrthographicModeRender({
  glbCamera,
  mapScene,
  mapBounds,
  resetViewTrigger,
}: MapModeRenderProps) {
  const cameraRef = useRef<THREE.OrthographicCamera>(null!)
  return (
    <>
      <OrthographicCamera ref={cameraRef} makeDefault near={1} far={100000} />
      <OrthographicCameraController
        cameraRef={cameraRef}
        glbCamera={glbCamera}
        mapScene={mapScene}
        mapBounds={mapBounds}
        resetViewTrigger={resetViewTrigger}
      />
    </>
  )
}

export const orthographicMode: MapMode = {
  kind: 'orthographic',
  Render: OrthographicModeRender,
}
