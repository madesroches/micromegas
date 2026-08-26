import { render, screen, fireEvent } from '@testing-library/react'
import type { StructRowProxy } from 'apache-arrow'
import { HistogramCell } from '../HistogramCell'
import { estimateHistogramQuantile } from '@/lib/histogram-utils'
import { resolveHistogramBarColor } from '@/lib/histogram-colors'
import {
  makeHistogramVector,
  SAMPLE_HISTOGRAM_ROW,
  type HistogramRowInput,
} from '@/lib/screen-renderers/__tests__/histogram-fixtures'

function rowFor(input: HistogramRowInput): StructRowProxy {
  return makeHistogramVector([input]).get(0) as StructRowProxy
}

describe('HistogramCell', () => {
  it('renders "-" for a null value', () => {
    render(<HistogramCell value={null} />)
    expect(screen.getByText('-')).toBeInTheDocument()
    expect(screen.queryByTestId('histogram-track')).not.toBeInTheDocument()
  })

  it('renders "-" for a degenerate (count === 0) histogram', () => {
    const row: HistogramRowInput = { start: 0, end: 10, min: 0, max: 0, sum: 0, sum_sq: 0, count: 0, bins: [0, 0, 0, 0] }
    render(<HistogramCell value={rowFor(row)} />)
    expect(screen.getByText('-')).toBeInTheDocument()
    expect(screen.queryByTestId('histogram-track')).not.toBeInTheDocument()
  })

  it('renders exactly bins.length <rect> children, no downsampling', () => {
    const { container } = render(<HistogramCell value={rowFor(SAMPLE_HISTOGRAM_ROW)} />)
    const rects = container.querySelectorAll('svg[data-testid="histogram-track"] rect')
    expect(rects.length).toBe(SAMPLE_HISTOGRAM_ROW.bins.length)
  })

  it('renders exactly bins.length rects for a large bin count (no cap)', () => {
    const bins = Array.from({ length: 200 }, (_, i) => (i % 7) + 1)
    const row: HistogramRowInput = { start: 0, end: 200, min: 0, max: 200, sum: 1000, sum_sq: 5000, count: bins.reduce((a, b) => a + b, 0), bins }
    const { container } = render(<HistogramCell value={rowFor(row)} />)
    const rects = container.querySelectorAll('svg[data-testid="histogram-track"] rect')
    expect(rects.length).toBe(200)
  })

  it('a 0-count bucket renders a rect with height >= 2 (visible-stub floor)', () => {
    const row: HistogramRowInput = { start: 0, end: 40, min: 0, max: 40, sum: 100, sum_sq: 1000, count: 40, bins: [0, 40, 0, 0] }
    const { container } = render(<HistogramCell value={rowFor(row)} />)
    const rects = container.querySelectorAll('svg[data-testid="histogram-track"] rect')
    // bucket 0 and buckets 2/3 all have count 0.
    const zeroCountRect = rects[0]
    expect(Number(zeroCountRect.getAttribute('height'))).toBeGreaterThanOrEqual(2)
  })

  it('median label matches estimateHistogramQuantile(h, 0.5)', () => {
    render(<HistogramCell value={rowFor(SAMPLE_HISTOGRAM_ROW)} />)
    const expected = estimateHistogramQuantile(
      {
        start: SAMPLE_HISTOGRAM_ROW.start,
        end: SAMPLE_HISTOGRAM_ROW.end,
        min: SAMPLE_HISTOGRAM_ROW.min,
        max: SAMPLE_HISTOGRAM_ROW.max,
        sum: SAMPLE_HISTOGRAM_ROW.sum,
        sum_sq: SAMPLE_HISTOGRAM_ROW.sum_sq,
        count: Number(SAMPLE_HISTOGRAM_ROW.count),
        bins: SAMPLE_HISTOGRAM_ROW.bins.map(Number),
      },
      0.5
    )
    expect(screen.getByTestId('histogram-median').textContent).toBe(expected.toFixed(1))
  })

  it('bar fill has no color prop -> default flat var(--chart-line)', () => {
    const { container } = render(<HistogramCell value={rowFor(SAMPLE_HISTOGRAM_ROW)} />)
    const rects = container.querySelectorAll('svg[data-testid="histogram-track"] rect')
    for (const rect of Array.from(rects)) {
      expect(rect.getAttribute('fill')).toBe('var(--chart-line)')
    }
  })

  it('bar fill uses a literal CSS color flat across all buckets', () => {
    const { container } = render(<HistogramCell value={rowFor(SAMPLE_HISTOGRAM_ROW)} color="#ff00ff" />)
    const rects = container.querySelectorAll('svg[data-testid="histogram-track"] rect')
    for (const rect of Array.from(rects)) {
      expect(rect.getAttribute('fill')).toBe('#ff00ff')
    }
  })

  it('bar fill matches resolveHistogramBarColor per-bucket for a colormap name', () => {
    const { container } = render(<HistogramCell value={rowFor(SAMPLE_HISTOGRAM_ROW)} color="viridis" />)
    const rects = container.querySelectorAll('svg[data-testid="histogram-track"] rect')
    const max = Math.max(...SAMPLE_HISTOGRAM_ROW.bins.map(Number))
    SAMPLE_HISTOGRAM_ROW.bins.forEach((v, i) => {
      const t = Number(v) / max
      expect(rects[i].getAttribute('fill')).toBe(resolveHistogramBarColor('viridis', t))
    })
  })

  describe('tooltip', () => {
    // jsdom has no PointerEvent implementation, so testing-library's own
    // fireEvent.pointerMove (which falls back to a bare `Event`, dropping
    // clientX/clientY entirely) can't carry a pointer position. Dispatch a
    // real MouseEvent (which jsdom DOES honor clientX/clientY on) typed as
    // 'pointermove'/'pointerleave' instead — React's listener is keyed on
    // the native event *type* string, not the constructor class, so this
    // still reaches the component's onPointerMove/onPointerLeave handlers.
    function pointerMove(el: Element, clientX: number, clientY: number) {
      fireEvent(el, new MouseEvent('pointermove', { clientX, clientY, bubbles: true }))
    }
    function pointerLeave(el: Element) {
      // React implements onPointerLeave via the bubbling 'pointerout' event
      // plus a relatedTarget check (native 'pointerleave' doesn't bubble, so
      // React doesn't listen for it directly) — dispatch 'pointerout' with a
      // relatedTarget outside the element to trigger the synthetic leave.
      fireEvent(el, new MouseEvent('pointerout', { bubbles: true, relatedTarget: document.body }))
    }
    function stubRect(el: Element, width: number) {
      el.getBoundingClientRect = () =>
        ({ left: 0, right: width, top: 0, bottom: 28, width, height: 28, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect
    }

    it('shows the hovered bucket range/count/percentage on pointermove', () => {
      const row: HistogramRowInput = { start: 0, end: 40, min: 0, max: 40, sum: 100, sum_sq: 1000, count: 40, bins: [10, 10, 10, 10] }
      const { container } = render(<HistogramCell value={rowFor(row)} />)
      const svg = container.querySelector('svg[data-testid="histogram-track"]')!
      stubRect(svg, 120)

      // bucket width = 30px for 4 bins over 120px track; clientX=45 -> bucket 1
      pointerMove(svg, 45, 10)

      const tooltip = screen.getByTestId('histogram-tooltip')
      expect(tooltip.textContent).toContain('10.0–20.0')
      expect(tooltip.textContent).toContain('count: 10 (25.0%)')
    })

    it('clears the tooltip on pointerleave', () => {
      const { container } = render(<HistogramCell value={rowFor(SAMPLE_HISTOGRAM_ROW)} />)
      const svg = container.querySelector('svg[data-testid="histogram-track"]')!
      stubRect(svg, 120)

      pointerMove(svg, 10, 10)
      expect(screen.getByTestId('histogram-tooltip')).toBeInTheDocument()

      pointerLeave(svg)
      expect(screen.queryByTestId('histogram-tooltip')).not.toBeInTheDocument()
    })

    it('clamps a bucket index that would fall outside [0, bins.length - 1]', () => {
      const row: HistogramRowInput = { start: 0, end: 40, min: 0, max: 40, sum: 100, sum_sq: 1000, count: 40, bins: [10, 10, 10, 10] }
      const { container } = render(<HistogramCell value={rowFor(row)} />)
      const svg = container.querySelector('svg[data-testid="histogram-track"]')!
      stubRect(svg, 120)

      // clientX far beyond the track width -> clamps to the last bucket.
      pointerMove(svg, 500, 10)
      const tooltip = screen.getByTestId('histogram-tooltip')
      expect(tooltip.textContent).toContain('30.0–40.0')
    })
  })
})
