import { zoomTimeRange } from '../time-range'

describe('zoomTimeRange', () => {
  it('zoom out doubles the duration, centered', () => {
    const from = '2025-01-15T10:00:00.000Z'
    const to = '2025-01-15T12:00:00.000Z' // 2h range
    const result = zoomTimeRange(from, to, 'out')

    const resultFrom = new Date(result.from).getTime()
    const resultTo = new Date(result.to).getTime()
    const duration = resultTo - resultFrom

    // Duration should be 4h (doubled)
    expect(duration).toBe(4 * 60 * 60 * 1000)

    // Center should be preserved at 11:00
    const center = resultFrom + duration / 2
    const originalCenter = new Date('2025-01-15T11:00:00.000Z').getTime()
    expect(center).toBe(originalCenter)
  })

  it('zoom in halves the duration, centered', () => {
    const from = '2025-01-15T10:00:00.000Z'
    const to = '2025-01-15T12:00:00.000Z' // 2h range
    const result = zoomTimeRange(from, to, 'in')

    const resultFrom = new Date(result.from).getTime()
    const resultTo = new Date(result.to).getTime()
    const duration = resultTo - resultFrom

    // Duration should be 1h (halved)
    expect(duration).toBe(1 * 60 * 60 * 1000)

    // Center should be preserved at 11:00
    const center = resultFrom + duration / 2
    const originalCenter = new Date('2025-01-15T11:00:00.000Z').getTime()
    expect(center).toBe(originalCenter)
  })

  it('zoom from relative range produces absolute ISO strings', () => {
    const result = zoomTimeRange('now-1h', 'now', 'out')

    // Both should be ISO date strings, not relative
    expect(result.from).toMatch(/^\d{4}-\d{2}-\d{2}T/)
    expect(result.to).toMatch(/^\d{4}-\d{2}-\d{2}T/)
    expect(result.from).not.toContain('now')
    expect(result.to).not.toContain('now')
  })

  it('"to" is clamped to not exceed current time', () => {
    // Use a range that ends at "now" - zooming out would push "to" into the future
    const now = Date.now()
    const from = new Date(now - 60 * 60 * 1000).toISOString() // 1h ago
    const to = new Date(now).toISOString()

    const result = zoomTimeRange(from, to, 'out')
    const resultTo = new Date(result.to).getTime()

    // "to" should not exceed now (with small tolerance for test execution time)
    expect(resultTo).toBeLessThanOrEqual(Date.now())
  })

  it('enforces minimum duration on zoom in', () => {
    // Start with a 2ms range
    const from = '2025-01-15T10:00:00.000Z'
    const to = '2025-01-15T10:00:00.002Z'
    const result = zoomTimeRange(from, to, 'in')

    const resultFrom = new Date(result.from).getTime()
    const resultTo = new Date(result.to).getTime()
    const duration = resultTo - resultFrom

    // Should not go below 1 millisecond
    expect(duration).toBeGreaterThanOrEqual(1)
  })

  it('enforces maximum duration on zoom out', () => {
    // Start with a 300-day range
    const from = '2024-01-01T00:00:00.000Z'
    const to = '2024-10-28T00:00:00.000Z'
    const result = zoomTimeRange(from, to, 'out')

    const resultFrom = new Date(result.from).getTime()
    const resultTo = new Date(result.to).getTime()
    const duration = resultTo - resultFrom

    // Should not exceed 365 days
    expect(duration).toBeLessThanOrEqual(365 * 24 * 60 * 60 * 1000)
  })

  it('handles zero-duration edge case', () => {
    const from = '2025-01-15T10:00:00.000Z'
    const to = '2025-01-15T10:00:00.000Z' // same time
    const result = zoomTimeRange(from, to, 'out')

    const resultFrom = new Date(result.from).getTime()
    const resultTo = new Date(result.to).getTime()
    const duration = resultTo - resultFrom

    // Should default to 30s then double to 60s
    expect(duration).toBe(60 * 1000)
  })
})
