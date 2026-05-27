import { render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import type { DataType } from 'apache-arrow'
import { MapHoverTooltip } from '../MapHoverTooltip'

function buildRow(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return { x: '1', y: '2', z: '3', ...overrides }
}

function renderTooltip(props: {
  x?: number
  y?: number
  row?: Record<string, unknown>
  columnTypes?: Map<string, DataType>
  template: string
}) {
  return render(
    <MemoryRouter>
      <MapHoverTooltip
        x={props.x ?? 100}
        y={props.y ?? 100}
        row={props.row ?? buildRow()}
        columnTypes={props.columnTypes ?? new Map()}
        template={props.template}
        variables={{}}
        timeRange={{ begin: '2026-01-01T00:00:00Z', end: '2026-01-02T00:00:00Z' }}
        cellResults={{}}
        cellSelections={{}}
      />
    </MemoryRouter>,
  )
}

describe('MapHoverTooltip', () => {
  it('renders the resolved template content', () => {
    renderTooltip({
      template: 'Location: ($x, $y, $z)',
      row: buildRow({ x: '10.5', y: '-3', z: '7' }),
    })
    expect(screen.getByText('Location: (10.5, -3, 7)')).toBeInTheDocument()
  })

  it('does not intercept pointer events', () => {
    renderTooltip({ template: 'hi' })
    const el = screen.getByText('hi').closest('.fixed')
    expect(el).toHaveClass('pointer-events-none')
  })

  it('positions down-right of the cursor by default', () => {
    // jsdom getBoundingClientRect returns zeros, so no flip is triggered;
    // the tooltip sits at the default cursor offset.
    renderTooltip({ template: 'hi', x: 100, y: 100 })
    const el = screen.getByText('hi').closest('.fixed') as HTMLElement
    expect(el.style.left).toBe('114px')
    expect(el.style.top).toBe('114px')
  })

  it('flips left/up rather than overflowing the viewport', () => {
    // Stub a large tooltip in a small viewport so the default down-right
    // placement would overflow both axes.
    const origW = window.innerWidth
    const origH = window.innerHeight
    Object.defineProperty(window, 'innerWidth', { value: 200, configurable: true })
    Object.defineProperty(window, 'innerHeight', { value: 200, configurable: true })
    const spy = jest
      .spyOn(HTMLElement.prototype, 'getBoundingClientRect')
      .mockReturnValue({ width: 150, height: 150 } as DOMRect)
    try {
      renderTooltip({ template: 'hi', x: 180, y: 180 })
      const el = screen.getByText('hi').closest('.fixed') as HTMLElement
      // Flipped to the left/up of the cursor: 180 - 14 - 150 = 16.
      expect(el.style.left).toBe('16px')
      expect(el.style.top).toBe('16px')
    } finally {
      spy.mockRestore()
      Object.defineProperty(window, 'innerWidth', { value: origW, configurable: true })
      Object.defineProperty(window, 'innerHeight', { value: origH, configurable: true })
    }
  })
})
