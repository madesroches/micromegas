import { useState } from 'react'
import { render, screen, fireEvent } from '@testing-library/react'
import { Utf8 } from 'apache-arrow'
import type { DataType } from 'apache-arrow'
import { OverrideEditor } from '../OverrideEditor'
import { type ColumnOverride } from '@/lib/screen-renderers/table-utils'
import { HISTOGRAM_STRUCT_TYPE } from '@/lib/screen-renderers/__tests__/histogram-fixtures'

const AVAILABLE_COLUMN_TYPES: Record<string, DataType> = {
  dist: HISTOGRAM_STRUCT_TYPE as unknown as DataType,
  name: new Utf8(),
}

/** A controlled wrapper mirroring how a real editor holds `overrides` state,
 *  so interacting through the component re-renders with the updated value —
 *  needed to observe toggle/swatch state after a click. */
function Harness({ initial }: { initial: ColumnOverride[] }) {
  const [overrides, setOverrides] = useState<ColumnOverride[]>(initial)
  return (
    <OverrideEditor
      overrides={overrides}
      availableColumns={['dist', 'name']}
      availableColumnTypes={AVAILABLE_COLUMN_TYPES}
      onChange={setOverrides}
    />
  )
}

describe('OverrideEditor — histogram "Render as" toggle', () => {
  it('shows the toggle only for a histogram-typed column', () => {
    const overrides: ColumnOverride[] = [{ column: 'dist' }, { column: 'name', format: 'x' }]
    render(
      <OverrideEditor
        overrides={overrides}
        availableColumns={['dist', 'name']}
        availableColumnTypes={AVAILABLE_COLUMN_TYPES}
        onChange={vi.fn()}
      />,
    )
    expect(screen.getAllByText('Render as')).toHaveLength(1)
  })

  it('does not show the toggle at all when availableColumnTypes is missing and kind is unset', () => {
    const overrides: ColumnOverride[] = [{ column: 'dist' }]
    render(
      <OverrideEditor overrides={overrides} availableColumns={['dist', 'name']} onChange={vi.fn()} />,
    )
    expect(screen.queryByText('Render as')).not.toBeInTheDocument()
  })

  it('still shows the toggle for a card already saved as kind: histogram, even with no availableColumnTypes', () => {
    const overrides: ColumnOverride[] = [{ column: 'dist', kind: 'histogram' }]
    render(
      <OverrideEditor overrides={overrides} availableColumns={['dist', 'name']} onChange={vi.fn()} />,
    )
    expect(screen.getByText('Render as')).toBeInTheDocument()
  })

  it('switching to Histogram hides the Format textarea and shows the swatch row', () => {
    render(<Harness initial={[{ column: 'dist', format: 'x' }]} />)
    expect(screen.getByPlaceholderText('[View](/path?id=$row.column_name)')).toBeInTheDocument()
    expect(screen.queryByText('Bar Color')).not.toBeInTheDocument()

    fireEvent.click(screen.getByText('Histogram'))

    expect(screen.queryByPlaceholderText('[View](/path?id=$row.column_name)')).not.toBeInTheDocument()
    expect(screen.getByText('Bar Color')).toBeInTheDocument()
  })

  it('switching back to Markdown restores the Format textarea', () => {
    render(<Harness initial={[{ column: 'dist', kind: 'histogram' }]} />)
    expect(screen.getByText('Bar Color')).toBeInTheDocument()

    fireEvent.click(screen.getByText('Markdown'))

    expect(screen.getByPlaceholderText('[View](/path?id=$row.column_name)')).toBeInTheDocument()
    expect(screen.queryByText('Bar Color')).not.toBeInTheDocument()
  })

  it('the column dropdown always lists every available column, not just histogram-typed ones', () => {
    render(<Harness initial={[{ column: 'dist', kind: 'histogram' }]} />)
    const select = screen.getByRole('combobox') as HTMLSelectElement
    const optionValues = Array.from(select.options).map((o) => o.value)
    expect(optionValues).toEqual(['dist', 'name'])
  })
})

describe('OverrideEditor — histogram color swatch picker', () => {
  it('the Default swatch is ring-highlighted when histogramColor is unset (a brand-new histogram card)', () => {
    render(<Harness initial={[{ column: 'dist', kind: 'histogram' }]} />)
    const defaultSwatch = screen.getByTitle('Default')
    expect(defaultSwatch.className).toContain('border-accent-link')
  })

  it('clicking a colormap swatch sets histogramColor to that name and ring-highlights it', () => {
    render(<Harness initial={[{ column: 'dist', kind: 'histogram' }]} />)
    fireEvent.click(screen.getByTitle('viridis'))

    const viridisSwatch = screen.getByTitle('viridis')
    const defaultSwatch = screen.getByTitle('Default')
    expect(viridisSwatch.className).toContain('border-accent-link')
    expect(defaultSwatch.className).not.toContain('border-accent-link')
  })

  it('changing the custom-color swatch input sets histogramColor to the picked hex and moves the highlight', () => {
    render(<Harness initial={[{ column: 'dist', kind: 'histogram' }]} />)
    const colorInput = document.querySelector('input[type="color"]') as HTMLInputElement
    fireEvent.change(colorInput, { target: { value: '#ff00ff' } })

    const customSwatch = screen.getByTitle('Custom color')
    const defaultSwatch = screen.getByTitle('Default')
    expect(customSwatch.className).toContain('border-accent-link')
    expect(defaultSwatch.className).not.toContain('border-accent-link')
    expect(colorInput.value.toLowerCase()).toBe('#ff00ff')
  })

  it('clicking Default after a colormap pick clears histogramColor and leaves format untouched', () => {
    // Start already toggled to Histogram with a colormap chosen; a `format`
    // value carried over from an earlier Markdown stint should survive.
    render(<Harness initial={[{ column: 'dist', kind: 'histogram', histogramColor: 'magma', format: 'stale $row.dist' }]} />)
    const magmaSwatch = screen.getByTitle('magma')
    expect(magmaSwatch.className).toContain('border-accent-link')

    fireEvent.click(screen.getByTitle('Default'))

    const defaultSwatch = screen.getByTitle('Default')
    expect(defaultSwatch.className).toContain('border-accent-link')
    expect(screen.getByTitle('magma').className).not.toContain('border-accent-link')

    // Switch back to Markdown to confirm the stale format survived untouched.
    fireEvent.click(screen.getByText('Markdown'))
    expect(screen.getByDisplayValue('stale $row.dist')).toBeInTheDocument()
  })
})

describe('OverrideEditor — Add Override seeds histogram columns differently', () => {
  it('adds a kind: histogram entry with no format for a histogram-typed column', () => {
    const onChange = vi.fn()
    render(
      <OverrideEditor
        overrides={[]}
        availableColumns={['dist', 'name']}
        availableColumnTypes={AVAILABLE_COLUMN_TYPES}
        onChange={onChange}
      />,
    )
    fireEvent.click(screen.getByText('Overrides')) // expand — starts collapsed when overrides is empty
    fireEvent.click(screen.getByText('Add Override'))
    expect(onChange).toHaveBeenCalledWith([{ column: 'dist', kind: 'histogram' }])
  })

  it('adds a markdown entry with the default link template when the first available column is not histogram-typed', () => {
    const onChange = vi.fn()
    render(
      <OverrideEditor
        overrides={[]}
        availableColumns={['name', 'dist']}
        availableColumnTypes={AVAILABLE_COLUMN_TYPES}
        onChange={onChange}
      />,
    )
    fireEvent.click(screen.getByText('Overrides')) // expand — starts collapsed when overrides is empty
    fireEvent.click(screen.getByText('Add Override'))
    expect(onChange).toHaveBeenCalledWith([{ column: 'name', format: '[Link](/path?id=$row.name)' }])
  })
})
