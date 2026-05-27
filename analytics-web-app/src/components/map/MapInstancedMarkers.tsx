/**
 * Instanced marker rendering for the map viewer.
 *
 * Extracted from MapViewer.tsx (#1089). Owns the three-pass instancing
 * (matrix layout / color baseline / highlight diff) and its ref-held GPU
 * buffers, plus click/hover interaction. The buffers are intrinsically stateful
 * and stay here wholesale; the per-instance RGBA shader patch lives in
 * `shader-patches.ts`.
 */
import { useRef, useEffect, useLayoutEffect, useState, useMemo, useCallback } from 'react'
import { ThreeEvent } from '@react-three/fiber'
import * as THREE from 'three'
import type { Overlay, OverlayConstants, Shape } from './overlay'
import { patchInstanceColorRGBA } from './shader-patches'

interface InstancedMarkersProps {
  overlay: Overlay
  constants: OverlayConstants
  shape: Shape
  selectedRowIndex: number | null
  onSelect: (rowIndex: number | null) => void
  onHover?: (rowIndex: number | null, clientX: number, clientY: number) => void
}

const COLOR_SELECTED_RGBA = 0xff6b6bff
const COLOR_HOVERED_RGBA = 0xff8a65ff
const SCALE_SELECTED = 1.5
const SCALE_HOVERED = 1.2

export function MapInstancedMarkers({
  overlay,
  constants,
  shape,
  selectedRowIndex,
  onSelect,
  onHover,
}: InstancedMarkersProps) {
  const meshRef = useRef<THREE.InstancedMesh>(null)
  const [hoveredRowIndex, setHoveredRowIndex] = useState<number | null>(null)

  // Clear hover synchronously when the overlay changes. Without this, a
  // stale hoveredRowIndex from the previous table would index a different
  // point in the new one, briefly highlighting the wrong marker until the
  // next pointer event corrected it. Mirrors the selection-clear pattern
  // in MapCell.
  const [overlayForHover, setOverlayForHover] = useState(overlay)
  if (overlayForHover !== overlay) {
    setOverlayForHover(overlay)
    setHoveredRowIndex(null)
  }

  const tempObject = useMemo(() => new THREE.Object3D(), [])

  // Unit geometry per shape. Layout pass scales per-instance.
  const geometry = useMemo(() => {
    if (shape === 'box') return new THREE.BoxGeometry(1, 1, 1)
    return new THREE.SphereGeometry(1, 16, 16)
  }, [shape])

  // Material flags differ per shape:
  //  - sphere: depth-disabled, transparent (always visible regardless of Z;
  //    per-row alpha blends with background — sort order is draw order).
  //  - box:    depth-tested, transparent (per-row alpha contributes to
  //    blending; occlusion against the GLB is correct).
  const material = useMemo(() => {
    const mat =
      shape === 'box'
        ? new THREE.MeshBasicMaterial({
            transparent: true,
            depthTest: true,
            depthWrite: false,
          })
        : new THREE.MeshBasicMaterial({
            transparent: true,
            depthTest: false,
            depthWrite: false,
          })
    patchInstanceColorRGBA(mat)
    return mat
  }, [shape])

  // Runtime per-instance RGBA buffer the GPU reads. We never write through
  // overlay.colorsRGBA directly — it's the immutable baseline that the
  // highlight pass restores from on de-select / un-hover.
  const colorAttrRef = useRef<THREE.InstancedBufferAttribute | null>(null)
  const runtimeColorsRef = useRef<Uint8Array | null>(null)
  const prevHighlightRef = useRef<{ selected: number | null; hovered: number | null }>({
    selected: null,
    hovered: null,
  })

  // rAF-throttled hover reporting. Pointer moves stash the latest instance id
  // and cursor position in a ref; a single queued frame flushes them, so at
  // most one tooltip reposition lands per paint regardless of move frequency.
  // `onHover` lives in a ref so the pointer handlers stay stable (the prop is
  // an inline arrow from MapCell that would otherwise re-create them).
  const onHoverRef = useRef(onHover)
  onHoverRef.current = onHover
  const pendingHoverRef = useRef<{ rowIndex: number; x: number; y: number } | null>(null)
  const hoverRafRef = useRef<number | null>(null)

  // Destructure into primitives so the effect dep arrays compare by value,
  // not by `constants` object identity. Without this split, a fresh
  // `constants` object on every render (it's a useMemo output that re-runs on
  // editor edits) would invalidate effect deps that don't actually use the
  // mutated field — e.g. a color-scalar drag would needlessly trigger the
  // matrix pass (63K setMatrixAt + computeBoundingSphere).
  const csize = constants.size
  const cscale0 = constants.scale[0]
  const cscale1 = constants.scale[1]
  const cscale2 = constants.scale[2]
  const ccolor = constants.color

  // Matrix pass: writes positions + scales for every instance. Allocates and
  // attaches the runtime color attribute the GPU reads (always — the color
  // baseline effect below fills it).
  //
  // useLayoutEffect (not useEffect): on shape toggle the InstancedMesh is
  // re-created with default identity matrices and a fresh geometry that has
  // no instanceColorRGBA attribute. A plain useEffect runs *after* paint, so
  // R3F's next rAF would render markers snapped to origin/scale=1 with zero
  // RGBA for one frame. useLayoutEffect runs synchronously after commit,
  // before any rAF, eliminating the glitch.
  useLayoutEffect(() => {
    const mesh = meshRef.current
    const numRows = overlay.table.numRows
    if (!mesh || numRows === 0) return
    const { positions, scales, scaleColumnMask, sizes } = overlay

    const expectedColorLen = numRows * 4
    if (!runtimeColorsRef.current || runtimeColorsRef.current.length !== expectedColorLen) {
      runtimeColorsRef.current = new Uint8Array(expectedColorLen)
      colorAttrRef.current = new THREE.InstancedBufferAttribute(
        runtimeColorsRef.current,
        4,
        /* normalized */ true,
      )
    }
    // Re-attach unconditionally: on shape change the geometry is swapped
    // (see `geometry` useMemo above) and the old attribute would be orphaned.
    mesh.geometry.setAttribute('instanceColorRGBA', colorAttrRef.current!)

    for (let i = 0; i < numRows; i++) {
      const pBase = i * 3
      tempObject.position.set(positions[pBase], positions[pBase + 1], positions[pBase + 2])
      if (shape === 'box') {
        // Per-channel: read from `scales` only when the channel was
        // column-bound at bake time; otherwise fall back to the live scalar
        // in `constants.scale` so editor edits aren't pinned to bake-time
        // values when mixed with a column-bound sibling channel.
        const sx = scales && scaleColumnMask?.[0] ? scales[pBase] : cscale0
        const sy = scales && scaleColumnMask?.[1] ? scales[pBase + 1] : cscale1
        const sz = scales && scaleColumnMask?.[2] ? scales[pBase + 2] : cscale2
        tempObject.scale.set(sx, sy, sz)
      } else {
        const s = sizes ? sizes[i] : csize
        tempObject.scale.setScalar(s)
      }
      tempObject.updateMatrix()
      mesh.setMatrixAt(i, tempObject.matrix)
    }

    // Required for correct raycasting and frustum culling on InstancedMesh:
    // the default bounding sphere comes from the unit geometry at origin, so
    // raycasts that miss the origin (i.e. most of them) skip every instance.
    mesh.computeBoundingSphere()
    mesh.instanceMatrix.needsUpdate = true
    // We intentionally do NOT reset prevHighlightRef here. The matrix pass
    // rewrites every slot's transform but does not touch the runtime color
    // buffer (baseline lives in its own effect). If we cleared prev, a row
    // that was tinted by the highlight pass before this re-layout would keep
    // its tint in runtime; the next highlight pass would have no record of
    // it and skip the restoreNormal that would write the baseline color
    // back. Leaving prev intact lets restoreNormal clean up correctly.
  }, [overlay, csize, cscale0, cscale1, cscale2, shape, tempObject])

  // Color baseline pass: refills the runtime color buffer from either the
  // column-bound buffer or the scalar fallback. Split from the matrix pass so
  // editor-side color scrubbing only does a 0.4 MB buffer write, not a
  // 63K-row matrix re-layout.
  //
  // useLayoutEffect for the same reason as the matrix pass: on first mount
  // (and any overlay swap) the runtime buffer is zero-initialized, which
  // renders fully transparent under the patched shader. Running pre-paint
  // ensures the first drawn frame has correct colors.
  useLayoutEffect(() => {
    const mesh = meshRef.current
    const numRows = overlay.table.numRows
    const runtime = runtimeColorsRef.current
    const colorAttr = colorAttrRef.current
    if (!mesh || numRows === 0 || !runtime || !colorAttr) return
    const { colorsRGBA } = overlay

    if (colorsRGBA) {
      runtime.set(colorsRGBA)
    } else {
      const r = (ccolor >>> 24) & 0xff
      const g = (ccolor >>> 16) & 0xff
      const b = (ccolor >>> 8) & 0xff
      const a = ccolor & 0xff
      for (let i = 0; i < numRows; i++) {
        const base = i * 4
        runtime[base] = r
        runtime[base + 1] = g
        runtime[base + 2] = b
        runtime[base + 3] = a
      }
    }
    colorAttr.needsUpdate = true
    // We intentionally do NOT reset prevHighlightRef here. The baseline
    // overwrote the runtime color for every slot, but slot scales (matrices)
    // are untouched — if a previously-highlighted row is no longer selected
    // or hovered in this render, the highlight pass must still call
    // restoreNormal on it to revert its 1.5x scaled matrix. The currently-
    // highlighted row gets re-tinted unconditionally by the highlight pass's
    // paint() calls, so we don't need to force re-apply via a prev reset.
  }, [overlay, ccolor])

  // Highlight diff: restore the previously highlighted slots to normal,
  // then write selected/hovered slots. Touches O(1) instances per change.
  // Declared after the layout effect so it runs after it in commit order,
  // overriding any colors the layout pass painted into the highlight slots.
  // useLayoutEffect so a shape toggle that re-runs all three passes lands
  // the selection highlight in the first painted frame, not the second.
  useLayoutEffect(() => {
    const mesh = meshRef.current
    const numRows = overlay.table.numRows
    if (!mesh || numRows === 0) return
    const runtime = runtimeColorsRef.current
    const colorAttr = colorAttrRef.current
    if (!runtime || !colorAttr) return
    const { positions, scales, scaleColumnMask, sizes, colorsRGBA } = overlay
    const prev = prevHighlightRef.current

    const writeMatrix = (i: number, scaleMul: number) => {
      const pBase = i * 3
      tempObject.position.set(positions[pBase], positions[pBase + 1], positions[pBase + 2])
      if (shape === 'box') {
        const sx = scales && scaleColumnMask?.[0] ? scales[pBase] : cscale0
        const sy = scales && scaleColumnMask?.[1] ? scales[pBase + 1] : cscale1
        const sz = scales && scaleColumnMask?.[2] ? scales[pBase + 2] : cscale2
        tempObject.scale.set(sx * scaleMul, sy * scaleMul, sz * scaleMul)
      } else {
        const base = sizes ? sizes[i] : csize
        tempObject.scale.setScalar(base * scaleMul)
      }
      tempObject.updateMatrix()
      mesh.setMatrixAt(i, tempObject.matrix)
    }

    const writeColorFromBaseline = (i: number) => {
      const cBase = i * 4
      if (colorsRGBA) {
        runtime[cBase] = colorsRGBA[cBase]
        runtime[cBase + 1] = colorsRGBA[cBase + 1]
        runtime[cBase + 2] = colorsRGBA[cBase + 2]
        runtime[cBase + 3] = colorsRGBA[cBase + 3]
      } else {
        runtime[cBase] = (ccolor >>> 24) & 0xff
        runtime[cBase + 1] = (ccolor >>> 16) & 0xff
        runtime[cBase + 2] = (ccolor >>> 8) & 0xff
        runtime[cBase + 3] = ccolor & 0xff
      }
    }

    const writeColorRGBA = (i: number, rgba: number) => {
      const cBase = i * 4
      runtime[cBase] = (rgba >>> 24) & 0xff
      runtime[cBase + 1] = (rgba >>> 16) & 0xff
      runtime[cBase + 2] = (rgba >>> 8) & 0xff
      runtime[cBase + 3] = rgba & 0xff
    }

    const restoreNormal = (i: number | null) => {
      if (i === null || i < 0 || i >= numRows) return
      if (i === selectedRowIndex || i === hoveredRowIndex) return
      writeMatrix(i, 1)
      writeColorFromBaseline(i)
    }
    restoreNormal(prev.selected)
    restoreNormal(prev.hovered)

    const paint = (i: number | null, multiplier: number, rgba: number) => {
      if (i === null || i < 0 || i >= numRows) return
      writeMatrix(i, multiplier)
      writeColorRGBA(i, rgba)
    }
    // Hover first, then selection — selection wins if a row is both.
    if (hoveredRowIndex !== null && hoveredRowIndex !== selectedRowIndex) {
      paint(hoveredRowIndex, SCALE_HOVERED, COLOR_HOVERED_RGBA)
    }
    if (selectedRowIndex !== null) {
      paint(selectedRowIndex, SCALE_SELECTED, COLOR_SELECTED_RGBA)
    }

    mesh.instanceMatrix.needsUpdate = true
    colorAttr.needsUpdate = true
    prevHighlightRef.current = { selected: selectedRowIndex, hovered: hoveredRowIndex }
  }, [
    overlay,
    csize,
    cscale0,
    cscale1,
    cscale2,
    ccolor,
    shape,
    selectedRowIndex,
    hoveredRowIndex,
    tempObject,
  ])

  useEffect(() => {
    return () => {
      geometry.dispose()
      material.dispose()
    }
  }, [geometry, material])

  // Restore body cursor on unmount in case a marker is hovered when we tear
  // down — the {ready} gate in MapViewer unmounts this component on mapUrl
  // changes, and a pointer-out we'd otherwise rely on never fires. Also cancel
  // any queued hover-flush frame so it can't fire after unmount.
  useEffect(() => {
    return () => {
      document.body.style.cursor = 'auto'
      if (hoverRafRef.current !== null) {
        cancelAnimationFrame(hoverRafRef.current)
        hoverRafRef.current = null
      }
    }
  }, [])

  const handleClick = useCallback(
    (e: ThreeEvent<MouseEvent>) => {
      e.stopPropagation()
      const rowIdx = e.instanceId
      const numRows = overlay.table.numRows
      if (rowIdx === undefined || rowIdx < 0 || rowIdx >= numRows) return
      onSelect(rowIdx === selectedRowIndex ? null : rowIdx)
    },
    [overlay, selectedRowIndex, onSelect]
  )

  const handlePointerOver = useCallback(
    (e: ThreeEvent<PointerEvent>) => {
      e.stopPropagation()
      const rowIdx = e.instanceId
      const numRows = overlay.table.numRows
      if (rowIdx === undefined || rowIdx < 0 || rowIdx >= numRows) return
      setHoveredRowIndex(rowIdx)
      document.body.style.cursor = 'pointer'
      // Surface the tooltip on enter even before the first move.
      onHoverRef.current?.(rowIdx, e.clientX, e.clientY)
    },
    [overlay]
  )

  // rAF-throttled: stash the latest instance id + cursor position, then flush
  // once per frame. `setHoveredRowIndex` with an unchanged value is a no-op for
  // the highlight effect, so per-move events only reposition the tooltip while
  // the highlighted row stays put.
  const handlePointerMove = useCallback(
    (e: ThreeEvent<PointerEvent>) => {
      e.stopPropagation()
      const rowIdx = e.instanceId
      const numRows = overlay.table.numRows
      if (rowIdx === undefined || rowIdx < 0 || rowIdx >= numRows) return
      pendingHoverRef.current = { rowIndex: rowIdx, x: e.clientX, y: e.clientY }
      if (hoverRafRef.current !== null) return
      hoverRafRef.current = requestAnimationFrame(() => {
        hoverRafRef.current = null
        const pending = pendingHoverRef.current
        if (!pending) return
        setHoveredRowIndex(pending.rowIndex)
        onHoverRef.current?.(pending.rowIndex, pending.x, pending.y)
      })
    },
    [overlay]
  )

  const handlePointerOut = useCallback(() => {
    setHoveredRowIndex(null)
    document.body.style.cursor = 'auto'
    pendingHoverRef.current = null
    if (hoverRafRef.current !== null) {
      cancelAnimationFrame(hoverRafRef.current)
      hoverRafRef.current = null
    }
    onHoverRef.current?.(null, 0, 0)
  }, [])

  if (overlay.table.numRows === 0) return null

  return (
    <instancedMesh
      ref={meshRef}
      args={[geometry, material, overlay.table.numRows]}
      renderOrder={10}
      onClick={handleClick}
      onPointerOver={handlePointerOver}
      onPointerMove={handlePointerMove}
      onPointerOut={handlePointerOut}
    />
  )
}
