import { render, screen, fireEvent } from '@testing-library/react'
import {
  Binary,
  Dictionary,
  Int32,
  Table,
  Timestamp,
  TimeUnit,
  Utf8,
  tableFromArrays,
  vectorFromArray,
} from 'apache-arrow'
import { ChannelBindingControl, mapMetadata } from '../MapCell'
import {
  buildOverlay,
  columnTypeMap,
  resolveMappingScalars,
  rowValues,
  type ChannelBinding,
} from '@/components/map/overlay'
import { DEFAULT_MAP_DETAIL_TEMPLATE } from '../../notebook-utils'

describe('buildOverlay', () => {
  it('returns ok with a row-ordered positions buffer of length numRows * 3', () => {
    const table = tableFromArrays({
      x: new Float64Array([1.5, 4.5]),
      y: new Float64Array([2.5, 5.5]),
      z: new Float64Array([3.5, 6.5]),
    })
    const result = buildOverlay(table)
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(result.overlay.positions).toHaveLength(6)
    expect(Array.from(result.overlay.positions)).toEqual([1.5, 2.5, 3.5, 4.5, 5.5, 6.5])
    expect(result.overlay.table).toBe(table)
  })

  it('returns ok: false naming the offending row when x is non-finite', () => {
    const table = tableFromArrays({
      x: new Float64Array([1, NaN]),
      y: new Float64Array([1, 1]),
      z: new Float64Array([1, 1]),
    })
    const result = buildOverlay(table)
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toMatch(/Row 1/)
    expect(result.error).toMatch(/non-finite/)
  })

  it('returns ok: false when required columns are missing', () => {
    const table = tableFromArrays({
      x: new Float64Array([0]),
      y: new Float64Array([0]),
      // z missing
    })
    const result = buildOverlay(table)
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toMatch(/Missing required columns/)
    expect(result.error).toMatch(/z/)
  })

  it('returns ok: false when x/y/z exist but are not numeric', () => {
    const table = tableFromArrays({
      x: ['a', 'b'],
      y: new Float64Array([0, 0]),
      z: new Float64Array([0, 0]),
    })
    const result = buildOverlay(table)
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toMatch(/'x'/)
    expect(result.error).toMatch(/numeric/)
  })

  it('default mapping leaves colorsRGBA undefined and puts #bf360cff in constants.color', () => {
    // Scalar color is not materialized into a per-row buffer — the renderer
    // reads `constants.color` instead. This is what keeps editor-side color
    // scrubbing from triggering a full O(numRows) overlay rebuild.
    const table = tableFromArrays({
      x: new Float64Array([0, 0]),
      y: new Float64Array([0, 0]),
      z: new Float64Array([0, 0]),
    })
    const result = buildOverlay(table)
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(result.overlay.colorsRGBA).toBeUndefined()
    expect(result.constants.color).toBe(0xbf360cff)
    // Default mapping is sphere; no scales/sizes buffers when channels are scalar.
    expect(result.overlay.sizes).toBeUndefined()
    expect(result.overlay.scales).toBeUndefined()
    expect(result.constants.size).toBe(10)
  })

  it('writes per-instance scales only for column-bound channels (box mixed)', () => {
    // Mixed mapping: scaleX is column-bound, scaleY/scaleZ are scalar. The
    // baked buffer must NOT pin the scalar values into its slots — the
    // renderer reads `constants.scale[k]` for scalar channels via
    // `scaleColumnMask`, so editor edits to scaleY/scaleZ aren't lost.
    const table = tableFromArrays({
      x: new Float64Array([0, 0]),
      y: new Float64Array([0, 0]),
      z: new Float64Array([0, 0]),
      sx: new Float64Array([7, 11]),
    })
    const result = buildOverlay(table, {
      scaleX: { column: 'sx' },
      scaleY: { scalar: 100 },
      scaleZ: { scalar: 100 },
    })
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(result.overlay.scales).toBeDefined()
    // Only the X slots are written; Y/Z slots stay zero-initialized.
    expect(Array.from(result.overlay.scales!)).toEqual([7, 0, 0, 11, 0, 0])
    expect(result.overlay.scaleColumnMask).toEqual([true, false, false])
    // The scalar fallbacks live in constants for the renderer to pick up
    // on every render, untouched by buildOverlay's row walk.
    expect(result.constants.scale).toEqual([100, 100, 100])
  })

  it('reads Int32 color column as u32, including high-bit-set values', () => {
    // 0xbf360cff comes back from Arrow Int32 as a negative JS number
    // (-1086357249). The signed→unsigned coercion in writeRGBA must preserve
    // the bit pattern.
    const colorVec = vectorFromArray(new Int32Array([0xbf360cff | 0]))
    const xVec = vectorFromArray(new Float64Array([0]))
    const yVec = vectorFromArray(new Float64Array([0]))
    const zVec = vectorFromArray(new Float64Array([0]))
    const table = new Table({ x: xVec, y: yVec, z: zVec, c: colorVec })
    const result = buildOverlay(table, { color: { column: 'c' } })
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(Array.from(result.overlay.colorsRGBA)).toEqual([0xbf, 0x36, 0x0c, 0xff])
  })

  it('reads Int64 (bigint) color column as u32', () => {
    // DataFusion infers integer literals like 0xbf360cff as Int64 by default;
    // Arrow JS returns bigint from col.get(i). The coercion path must avoid
    // the TypeError that >>> would throw on a bigint.
    const colorVec = vectorFromArray(new BigInt64Array([0xbf360cffn]))
    const xVec = vectorFromArray(new Float64Array([0]))
    const yVec = vectorFromArray(new Float64Array([0]))
    const zVec = vectorFromArray(new Float64Array([0]))
    const table = new Table({ x: xVec, y: yVec, z: zVec, c: colorVec })
    const result = buildOverlay(table, { color: { column: 'c' } })
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(Array.from(result.overlay.colorsRGBA)).toEqual([0xbf, 0x36, 0x0c, 0xff])
  })

  it('parses string color column with #rrggbb (alpha defaults to 0xff)', () => {
    const table = tableFromArrays({
      x: new Float64Array([0]),
      y: new Float64Array([0]),
      z: new Float64Array([0]),
      c: ['#00ff80'],
    })
    const result = buildOverlay(table, { color: { column: 'c' } })
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(Array.from(result.overlay.colorsRGBA)).toEqual([0x00, 0xff, 0x80, 0xff])
  })

  it('parses string color column with #rrggbbaa', () => {
    const table = tableFromArrays({
      x: new Float64Array([0]),
      y: new Float64Array([0]),
      z: new Float64Array([0]),
      c: ['#11223344'],
    })
    const result = buildOverlay(table, { color: { column: 'c' } })
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(Array.from(result.overlay.colorsRGBA)).toEqual([0x11, 0x22, 0x33, 0x44])
  })

  it('parses dictionary-encoded Utf8 color column (the CASE WHEN case)', () => {
    // A literal '#rrggbbaa' in a CASE WHEN arrives as Dictionary<Int32, Utf8>
    // in Arrow IPC. A naked isStringType check would reject it; the unwrap
    // path must accept it.
    const dictType = new Dictionary(new Utf8(), new Int32())
    const colorVec = vectorFromArray(['#11223344', '#11223344'], dictType)
    const xVec = vectorFromArray(new Float64Array([0, 0]))
    const yVec = vectorFromArray(new Float64Array([0, 0]))
    const zVec = vectorFromArray(new Float64Array([0, 0]))
    const table = new Table({ x: xVec, y: yVec, z: zVec, c: colorVec })
    const result = buildOverlay(table, { color: { column: 'c' } })
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(Array.from(result.overlay.colorsRGBA)).toEqual([
      0x11, 0x22, 0x33, 0x44,
      0x11, 0x22, 0x33, 0x44,
    ])
  })

  it('returns ok: false naming the row for an unparseable color string', () => {
    const table = tableFromArrays({
      x: new Float64Array([0, 0]),
      y: new Float64Array([0, 0]),
      z: new Float64Array([0, 0]),
      c: ['#11223344', 'red'],
    })
    const result = buildOverlay(table, { color: { column: 'c' } })
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toMatch(/Row 1/)
    expect(result.error).toMatch(/unparseable/)
  })

  it('returns ok: false when color column is neither integer, string, nor binary', () => {
    const table = tableFromArrays({
      x: new Float64Array([0]),
      y: new Float64Array([0]),
      z: new Float64Array([0]),
      c: new Float64Array([1.5]),
    })
    const result = buildOverlay(table, { color: { column: 'c' } })
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toMatch(/'c'/)
    expect(result.error).toMatch(/must be integer/)
    expect(result.error).toMatch(/binary/)
  })

  it('reads Binary color column as packed R,G,B,A (the 0xrrggbbaa SQL literal case)', () => {
    // DataFusion parses `0xff0000ff` as a 4-byte Binary literal, not an int.
    // The bytes come back from Arrow JS as a Uint8Array in big-endian order,
    // which we copy straight into the RGBA buffer.
    const colorVec = vectorFromArray(
      [new Uint8Array([0xff, 0x00, 0x00, 0xff])],
      new Binary(),
    )
    const xVec = vectorFromArray(new Float64Array([0]))
    const yVec = vectorFromArray(new Float64Array([0]))
    const zVec = vectorFromArray(new Float64Array([0]))
    const table = new Table({ x: xVec, y: yVec, z: zVec, c: colorVec })
    const result = buildOverlay(table, { color: { column: 'c' } })
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(Array.from(result.overlay.colorsRGBA)).toEqual([0xff, 0x00, 0x00, 0xff])
  })

  it('returns ok: false naming the row when a Binary color cell is not exactly 4 bytes', () => {
    const colorVec = vectorFromArray(
      [
        new Uint8Array([0xff, 0x00, 0x00, 0xff]),
        new Uint8Array([0xff, 0x00]),
      ],
      new Binary(),
    )
    const xVec = vectorFromArray(new Float64Array([0, 0]))
    const yVec = vectorFromArray(new Float64Array([0, 0]))
    const zVec = vectorFromArray(new Float64Array([0, 0]))
    const table = new Table({ x: xVec, y: yVec, z: zVec, c: colorVec })
    const result = buildOverlay(table, { color: { column: 'c' } })
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toMatch(/Row 1/)
    expect(result.error).toMatch(/2 bytes/)
  })

  it('returns ok: false naming the row for a non-finite numeric channel', () => {
    const table = tableFromArrays({
      x: new Float64Array([0, 0]),
      y: new Float64Array([0, 0]),
      z: new Float64Array([0, 0]),
      sx: new Float64Array([1, NaN]),
    })
    const result = buildOverlay(table, {
      scaleX: { column: 'sx' },
      scaleY: { scalar: 1 },
      scaleZ: { scalar: 1 },
    })
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toMatch(/Row 1/)
    expect(result.error).toMatch(/non-finite/)
  })
})

describe('resolveMappingScalars', () => {
  const emptyCtx = {
    variables: {},
    timeRange: { begin: '', end: '' },
    cellResults: {},
    cellSelections: {},
  }

  it('passes legacy numeric scalars through unchanged', () => {
    const r = resolveMappingScalars(
      { size: { scalar: 10 }, color: { scalar: 0xbf360cff } },
      emptyCtx,
    )
    expect(r.ok).toBe(true)
    if (!r.ok) return
    expect(r.mapping.size).toEqual({ scalar: 10 })
    expect(r.mapping.color).toEqual({ scalar: 0xbf360cff })
  })

  it('parses a literal numeric string scalar', () => {
    const r = resolveMappingScalars({ size: { scalar: '42' } }, emptyCtx)
    expect(r.ok).toBe(true)
    if (!r.ok) return
    expect(r.mapping.size).toEqual({ scalar: 42 })
  })

  it('expands a $variable macro into a numeric scalar', () => {
    const r = resolveMappingScalars(
      { size: { scalar: '$mySize' } },
      { ...emptyCtx, variables: { mySize: '75' } },
    )
    expect(r.ok).toBe(true)
    if (!r.ok) return
    expect(r.mapping.size).toEqual({ scalar: 75 })
  })

  it('expands a $cell.selected.column macro into a numeric scalar', () => {
    const tbl = tableFromArrays({ radius: new Float64Array([25]) })
    const r = resolveMappingScalars(
      { size: { scalar: '$tbl.selected.radius' } },
      {
        ...emptyCtx,
        cellResults: { tbl },
        cellSelections: { tbl: { radius: 25 } },
      },
    )
    expect(r.ok).toBe(true)
    if (!r.ok) return
    expect(r.mapping.size).toEqual({ scalar: 25 })
  })

  it('returns an error when a numeric macro resolves to non-numeric text', () => {
    const r = resolveMappingScalars(
      { size: { scalar: '$label' } },
      { ...emptyCtx, variables: { label: 'big' } },
    )
    expect(r.ok).toBe(false)
    if (r.ok) return
    expect(r.error).toMatch(/size/)
    expect(r.error).toMatch(/not a number/)
  })

  it('rejects an empty/whitespace scalar as not a number', () => {
    // Number('') is 0 in JS — guard against silently producing zero from an
    // unset field by surfacing it as an error.
    const r = resolveMappingScalars({ size: { scalar: '   ' } }, emptyCtx)
    expect(r.ok).toBe(false)
  })

  it('parses a hex color scalar literal', () => {
    const r = resolveMappingScalars({ color: { scalar: '#bf360cff' } }, emptyCtx)
    expect(r.ok).toBe(true)
    if (!r.ok) return
    expect(r.mapping.color).toEqual({ scalar: 0xbf360cff })
  })

  it('expands a $variable macro into a hex color scalar', () => {
    const r = resolveMappingScalars(
      { color: { scalar: '$theme' } },
      { ...emptyCtx, variables: { theme: '#0080ffff' } },
    )
    expect(r.ok).toBe(true)
    if (!r.ok) return
    expect(r.mapping.color).toEqual({ scalar: 0x0080ffff })
  })

  it('returns an error when a color macro resolves to non-hex text', () => {
    const r = resolveMappingScalars(
      { color: { scalar: '$broken' } },
      { ...emptyCtx, variables: { broken: 'red' } },
    )
    expect(r.ok).toBe(false)
    if (r.ok) return
    expect(r.error).toMatch(/color/)
  })

  it('passes column bindings through unchanged', () => {
    const r = resolveMappingScalars(
      { size: { column: 'radius' }, color: { column: 'tint' } },
      emptyCtx,
    )
    expect(r.ok).toBe(true)
    if (!r.ok) return
    expect(r.mapping.size).toEqual({ column: 'radius' })
    expect(r.mapping.color).toEqual({ column: 'tint' })
  })
})

describe('rowValues', () => {
  it('returns raw column values (no stringification)', () => {
    const table = tableFromArrays({
      process_id: ['p1'],
      x: new Float64Array([1.5]),
      y: new Float64Array([2.5]),
      z: new Float64Array([3.5]),
      event_type: ['hit'],
    })
    expect(rowValues(table, 0)).toEqual({
      process_id: 'p1',
      x: 1.5,
      y: 2.5,
      z: 3.5,
      event_type: 'hit',
    })
  })

  it('omits columns whose value is null', () => {
    const table = tableFromArrays({
      process_id: ['p1'],
      x: new Float64Array([0]),
      y: new Float64Array([0]),
      z: new Float64Array([0]),
      maybe_null: [null as string | null],
    })
    const row = rowValues(table, 0)
    expect(row).not.toHaveProperty('maybe_null')
    expect(row.process_id).toBe('p1')
  })

  it('returns a Timestamp column as its raw epoch value', () => {
    const timestampType = new Timestamp(TimeUnit.MILLISECOND, null)
    const timeVec = vectorFromArray([1705314600000], timestampType)
    const table = new Table({
      time: timeVec,
      x: vectorFromArray([0]),
      y: vectorFromArray([0]),
      z: vectorFromArray([0]),
    })
    // Raw value (not RFC3339) — the type map carries the DataType so the
    // template evaluator can format it at emission time.
    expect(rowValues(table, 0).time).toBe(1705314600000)
  })
})

describe('columnTypeMap', () => {
  it('maps each column name to its Arrow DataType', () => {
    const timestampType = new Timestamp(TimeUnit.MILLISECOND, null)
    const table = new Table({
      time: vectorFromArray([1705314600000], timestampType),
      x: vectorFromArray([0]),
    })
    const types = columnTypeMap(table)
    expect(types.get('time')).toBe(timestampType)
    expect(types.get('x')).toBeDefined()
    expect(types.has('missing')).toBe(false)
  })
})

describe('mapMetadata', () => {
  it('seeds detailTemplate in createDefaultConfig', () => {
    const config = mapMetadata.createDefaultConfig() as { options?: Record<string, unknown> }
    expect(config.options?.detailTemplate).toBe(DEFAULT_MAP_DETAIL_TEMPLATE)
  })

  it('seeds shape=sphere with size/color scalars in createDefaultConfig', () => {
    const config = mapMetadata.createDefaultConfig() as {
      options?: { shape?: string; mapping?: Record<string, unknown> }
    }
    expect(config.options?.shape).toBe('sphere')
    expect(config.options?.mapping).toEqual({
      size: { scalar: 10 },
      color: { scalar: 0xbf360cff },
    })
  })

  it('declares single-row selection mode by default', () => {
    expect(mapMetadata.defaultSelectionMode).toBe('single')
  })
})

describe('ChannelBindingControl', () => {
  // Common props for the numeric variant. Tests override `binding` and `onChange`.
  const baseProps = {
    label: 'Size',
    kind: 'numeric' as const,
    fallbackScalar: 10,
    columns: ['x', 'y', 'radius'],
  }

  it('does not fire onChange while the user types', () => {
    const onChange = jest.fn()
    render(
      <ChannelBindingControl
        {...baseProps}
        binding={{ scalar: '10' }}
        onChange={onChange}
      />,
    )
    const input = screen.getByDisplayValue('10') as HTMLInputElement
    fireEvent.change(input, { target: { value: '42' } })
    fireEvent.change(input, { target: { value: '425' } })
    // The text input is local-draft; nothing should reach the model yet.
    expect(onChange).not.toHaveBeenCalled()
    expect(input.value).toBe('425')
  })

  it('commits the draft on blur', () => {
    const onChange = jest.fn()
    render(
      <ChannelBindingControl
        {...baseProps}
        binding={{ scalar: '10' }}
        onChange={onChange}
      />,
    )
    const input = screen.getByDisplayValue('10') as HTMLInputElement
    fireEvent.change(input, { target: { value: '42' } })
    fireEvent.blur(input)
    expect(onChange).toHaveBeenCalledTimes(1)
    expect(onChange).toHaveBeenCalledWith({ scalar: '42' })
  })

  it('commits the draft on Enter', () => {
    const onChange = jest.fn()
    render(
      <ChannelBindingControl
        {...baseProps}
        binding={{ scalar: '10' }}
        onChange={onChange}
      />,
    )
    const input = screen.getByDisplayValue('10') as HTMLInputElement
    // Focus is required for `.blur()` to actually fire a blur event in jsdom.
    input.focus()
    fireEvent.change(input, { target: { value: '99' } })
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(onChange).toHaveBeenCalledTimes(1)
    expect(onChange).toHaveBeenCalledWith({ scalar: '99' })
  })

  it('Escape discards the draft without committing', () => {
    // Regression: keydown's setDraft is batched in React 18+, so the
    // synchronous blur triggered by `.blur()` reads the typed value from
    // closure. A skip-next-commit flag is required.
    const onChange = jest.fn()
    render(
      <ChannelBindingControl
        {...baseProps}
        binding={{ scalar: '10' }}
        onChange={onChange}
      />,
    )
    const input = screen.getByDisplayValue('10') as HTMLInputElement
    input.focus()
    fireEvent.change(input, { target: { value: 'foo' } })
    fireEvent.keyDown(input, { key: 'Escape' })
    expect(onChange).not.toHaveBeenCalled()
    expect(input.value).toBe('10')
  })

  it('does not fire onChange on blur when the draft equals the prop', () => {
    const onChange = jest.fn()
    render(
      <ChannelBindingControl
        {...baseProps}
        binding={{ scalar: '10' }}
        onChange={onChange}
      />,
    )
    const input = screen.getByDisplayValue('10') as HTMLInputElement
    fireEvent.focus(input)
    fireEvent.blur(input)
    expect(onChange).not.toHaveBeenCalled()
  })

  it('adopts an external prop change over the in-flight draft', () => {
    // Mode switches, color-picker picks, and saved-config reloads update
    // the binding from outside. The text input should pick up the new
    // value instead of keeping a stale uncommitted draft.
    const onChange = jest.fn()
    const { rerender } = render(
      <ChannelBindingControl
        {...baseProps}
        binding={{ scalar: '10' }}
        onChange={onChange}
      />,
    )
    const input = screen.getByDisplayValue('10') as HTMLInputElement
    fireEvent.change(input, { target: { value: 'typing...' } })
    expect(input.value).toBe('typing...')
    rerender(
      <ChannelBindingControl
        {...baseProps}
        binding={{ scalar: '50' }}
        onChange={onChange}
      />,
    )
    expect(input.value).toBe('50')
  })

  it('renders legacy numeric scalar in canonical string form', () => {
    render(
      <ChannelBindingControl
        {...baseProps}
        binding={{ scalar: 25 } as ChannelBinding}
        onChange={jest.fn()}
      />,
    )
    expect(screen.getByDisplayValue('25')).toBeInTheDocument()
  })

  it('renders legacy numeric color scalar as #rrggbbaa', () => {
    render(
      <ChannelBindingControl
        label="Color"
        kind="color"
        fallbackScalar={0xbf360cff}
        columns={[]}
        binding={{ scalar: 0xbf360cff } as ChannelBinding}
        onChange={jest.fn()}
      />,
    )
    expect(screen.getByDisplayValue('#bf360cff')).toBeInTheDocument()
  })
})
