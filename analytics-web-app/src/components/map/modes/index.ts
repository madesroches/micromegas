import { perspectiveMode } from './PerspectiveMode'
import { orthographicMode } from './OrthographicMode'
import type { MapMode, MapModeKind } from './types'

export const MAP_MODES: Record<MapModeKind, MapMode> = {
  perspective: perspectiveMode,
  orthographic: orthographicMode,
}

export const MAP_MODE_KINDS = Object.keys(MAP_MODES) as MapModeKind[]

export { MAP_MODE_LABELS } from './types'
export type { MapMode, MapModeKind, MapModeRenderProps } from './types'
