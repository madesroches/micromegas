/**
 * THREE.js + Canvas2D renderer for the flame graph.
 *
 * Extracted from FlameGraphCell.tsx (#1089). Owns the WebGL resources
 * (renderer, orthographic camera, scene, instanced mesh) and the stateless
 * "given this index + view state, paint a frame" logic. Interaction and view
 * state stay in the React shell (`FlameGraphView`), which passes a snapshot to
 * `render()` each frame. This module holds no React and no view state of its
 * own beyond the GPU buffers and current canvas dimensions.
 */
import * as THREE from 'three'
import {
  axisValue,
  formatAxisTick,
  laneYOffset,
  spanColor,
  LABEL_MIN_WIDTH_PX,
  LANE_HEADER_HEIGHT,
  SPAN_GAP,
  SPAN_HEIGHT,
  TIME_AXIS_HEIGHT,
  type FlameIndex,
} from './flame-model'

/** View state owned by the shell and passed to `render()` each frame. */
export interface FlameView {
  viewMinTime: number
  viewMaxTime: number
  scrollY: number
  isDragging: boolean
  isPanning: boolean
  dragStartX: number
  dragCurrentX: number
  width: number
  height: number
}

export interface FlameScene {
  /** Resize the WebGL + text canvases and the renderer. */
  resize(width: number, height: number, dpr: number): void
  /** Paint one frame for the given index and view state. */
  render(index: FlameIndex, view: FlameView): void
  dispose(): void
}

export function createFlameScene(
  webglCanvas: HTMLCanvasElement,
  textCanvas: HTMLCanvasElement,
  initialCapacity: number,
): FlameScene {
  const dpr = window.devicePixelRatio || 1

  const renderer = new THREE.WebGLRenderer({ canvas: webglCanvas, antialias: false, alpha: true })
  renderer.setPixelRatio(dpr)

  const camera = new THREE.OrthographicCamera(0, 1, 1, 0, -1, 1)
  const scene = new THREE.Scene()

  const geo = new THREE.PlaneGeometry(1, 1)
  // White base color — instance colors from setColorAt() are multiplied with this.
  // Do NOT use vertexColors:true — PlaneGeometry has no vertex color attribute,
  // which zeroes out the color. InstancedMesh has its own USE_INSTANCING_COLOR path.
  const material = new THREE.MeshBasicMaterial({ color: 0xffffff })
  let mesh = new THREE.InstancedMesh(geo, material, Math.max(initialCapacity, 1))
  mesh.frustumCulled = false
  scene.add(mesh)
  let maxInstances = Math.max(initialCapacity, 1)

  function resize(width: number, height: number, resizeDpr: number): void {
    webglCanvas.width = width * resizeDpr
    webglCanvas.height = (height - TIME_AXIS_HEIGHT) * resizeDpr
    webglCanvas.style.width = `${width}px`
    webglCanvas.style.height = `${height - TIME_AXIS_HEIGHT}px`

    textCanvas.width = width * resizeDpr
    textCanvas.height = height * resizeDpr
    textCanvas.style.width = `${width}px`
    textCanvas.style.height = `${height}px`

    renderer.setSize(width, height - TIME_AXIS_HEIGHT, false)
  }

  function render(index: FlameIndex, view: FlameView): void {
    if (view.width === 0 || view.height === 0) return

    const canvasHeight = view.height - TIME_AXIS_HEIGHT
    const timeSpan = view.viewMaxTime - view.viewMinTime
    if (timeSpan <= 0) return

    const beginCol = index.table.getChild('begin')!
    const endCol = index.table.getChild('end')!
    const nameCol = index.table.getChild('name')!
    const beginType = beginCol.type
    const endType = endCol.type
    const xAxisMode = index.xAxisMode

    const pxPerMs = view.width / timeSpan
    const mat = new THREE.Matrix4()
    const col = new THREE.Color()
    let instanceIdx = 0

    // Ensure mesh has enough capacity
    const estimatedMax = index.table.numRows
    if (estimatedMax > maxInstances) {
      // Recreate mesh with larger capacity
      scene.remove(mesh)
      mesh.dispose()
      const newGeo = new THREE.PlaneGeometry(1, 1)
      const newMaterial = new THREE.MeshBasicMaterial({ color: 0xffffff })
      mesh = new THREE.InstancedMesh(newGeo, newMaterial, estimatedMax)
      mesh.frustumCulled = false
      scene.add(mesh)
      maxInstances = estimatedMax
    }

    // Populate instances for visible spans
    for (let li = 0; li < index.lanes.length; li++) {
      const lane = index.lanes[li]
      const laneTop = laneYOffset(index.lanes, li) + LANE_HEADER_HEIGHT - view.scrollY

      // Skip lanes entirely off-screen
      const laneContentHeight = (lane.maxDepth + 1) * (SPAN_HEIGHT + SPAN_GAP)
      if (laneTop + laneContentHeight < 0 || laneTop > canvasHeight) continue

      for (let i = 0; i < lane.rowIndices.length; i++) {
        const row = lane.rowIndices[i]
        const begin = axisValue(beginCol.get(row), beginType, xAxisMode)
        if (begin >= view.viewMaxTime) break // sorted by begin — nothing further can be visible

        const end = axisValue(endCol.get(row), endType, xAxisMode)
        if (end <= view.viewMinTime) continue // ends before viewport

        const depth = lane.visualDepths[i]
        const name = String(nameCol.get(row) ?? '')

        // Pixel coordinates
        const x1 = (begin - view.viewMinTime) * pxPerMs
        const x2 = (end - view.viewMinTime) * pxPerMs
        const w = Math.max(x2 - x1, 1) // min 1px width
        const y = laneTop + depth * (SPAN_HEIGHT + SPAN_GAP)

        // Skip if off-screen vertically
        if (y + SPAN_HEIGHT < 0 || y > canvasHeight) continue

        // Set instance transform: translate to center, scale to size
        mat.makeScale(w, SPAN_HEIGHT, 1)
        mat.setPosition(x1 + w / 2, canvasHeight - y - SPAN_HEIGHT / 2, 0)
        mesh.setMatrixAt(instanceIdx, mat)

        // Color
        const [hex] = spanColor(name)
        col.set(hex)
        mesh.setColorAt(instanceIdx, col)

        instanceIdx++
      }
    }

    mesh.count = instanceIdx
    mesh.instanceMatrix.needsUpdate = true
    if (mesh.instanceColor) mesh.instanceColor.needsUpdate = true

    // Camera: orthographic pixel-space
    camera.left = 0
    camera.right = view.width
    camera.top = canvasHeight
    camera.bottom = 0
    camera.near = -1
    camera.far = 1
    camera.updateProjectionMatrix()

    renderer.render(scene, camera)

    // --- Canvas2D overlay: labels + time axis + selection ---
    const ctx = textCanvas.getContext('2d')
    if (!ctx) return

    const overlayDpr = window.devicePixelRatio || 1
    ctx.clearRect(0, 0, textCanvas.width, textCanvas.height)
    ctx.save()
    ctx.scale(overlayDpr, overlayDpr)

    // Draw span labels
    ctx.font = '11px monospace'
    ctx.textBaseline = 'middle'

    for (let li = 0; li < index.lanes.length; li++) {
      const lane = index.lanes[li]
      const laneTop = laneYOffset(index.lanes, li) + LANE_HEADER_HEIGHT - view.scrollY

      const laneContentHeight = (lane.maxDepth + 1) * (SPAN_HEIGHT + SPAN_GAP)
      if (laneTop + laneContentHeight < 0 || laneTop > canvasHeight) continue

      for (let i = 0; i < lane.rowIndices.length; i++) {
        const row = lane.rowIndices[i]
        const begin = axisValue(beginCol.get(row), beginType, xAxisMode)
        if (begin > view.viewMaxTime) break

        const end = axisValue(endCol.get(row), endType, xAxisMode)
        if (end < view.viewMinTime) continue

        const depth = lane.visualDepths[i]
        const name = String(nameCol.get(row) ?? '')

        const x1 = (begin - view.viewMinTime) * pxPerMs
        const x2 = (end - view.viewMinTime) * pxPerMs
        const w = x2 - x1
        const y = laneTop + depth * (SPAN_HEIGHT + SPAN_GAP)

        if (y + SPAN_HEIGHT < 0 || y > canvasHeight) continue
        if (w < LABEL_MIN_WIDTH_PX) continue

        const [, textLight] = spanColor(name)
        ctx.fillStyle = textLight ? '#ffffff' : '#000000'

        ctx.save()
        ctx.beginPath()
        ctx.rect(Math.max(x1 + 2, 0), y, Math.min(w - 4, view.width), SPAN_HEIGHT)
        ctx.clip()
        ctx.fillText(name, x1 + 4, y + SPAN_HEIGHT / 2 + 1)
        ctx.restore()
      }
    }

    // Draw lane headers
    ctx.font = 'bold 11px sans-serif'
    ctx.textBaseline = 'middle'
    ctx.fillStyle = '#9ca3af' // gray-400
    for (let li = 0; li < index.lanes.length; li++) {
      const lane = index.lanes[li]
      const headerY = laneYOffset(index.lanes, li) - view.scrollY
      if (headerY + LANE_HEADER_HEIGHT < 0 || headerY > canvasHeight) continue
      ctx.fillText(lane.name, 4, headerY + LANE_HEADER_HEIGHT / 2)
    }

    // Draw time axis
    const axisY = canvasHeight
    ctx.fillStyle = '#1a1a2e'
    ctx.fillRect(0, axisY, view.width, TIME_AXIS_HEIGHT)

    ctx.font = '10px monospace'
    ctx.fillStyle = '#9ca3af'
    ctx.textBaseline = 'top'
    const tickCount = Math.max(2, Math.floor(view.width / 120))
    const tickStep = timeSpan / (tickCount - 1)
    for (let t = 0; t < tickCount; t++) {
      const tickValue = view.viewMinTime + t * tickStep
      const x = t * (view.width / (tickCount - 1))
      ctx.fillText(formatAxisTick(tickValue, xAxisMode), t === tickCount - 1 ? Math.max(0, x - 80) : x + 2, axisY + 4)
      ctx.strokeStyle = '#374151'
      ctx.beginPath()
      ctx.moveTo(x, axisY)
      ctx.lineTo(x, axisY + 4)
      ctx.stroke()
    }

    // Draw selection overlay
    if (view.isDragging && !view.isPanning) {
      const selLeft = Math.min(view.dragStartX, view.dragCurrentX)
      const selWidth = Math.abs(view.dragCurrentX - view.dragStartX)
      if (selWidth > 2) {
        ctx.fillStyle = 'rgba(59, 130, 246, 0.2)'
        ctx.fillRect(selLeft, 0, selWidth, canvasHeight)
        ctx.strokeStyle = 'rgba(59, 130, 246, 0.6)'
        ctx.lineWidth = 2
        ctx.beginPath()
        ctx.moveTo(selLeft, 0)
        ctx.lineTo(selLeft, canvasHeight)
        ctx.moveTo(selLeft + selWidth, 0)
        ctx.lineTo(selLeft + selWidth, canvasHeight)
        ctx.stroke()
      }
    }

    ctx.restore()
  }

  function dispose(): void {
    scene.remove(mesh)
    mesh.dispose()
    geo.dispose()
    material.dispose()
    renderer.dispose()
  }

  return { resize, render, dispose }
}
