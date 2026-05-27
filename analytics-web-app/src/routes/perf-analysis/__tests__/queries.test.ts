import { buildUrl, calculateBinInterval, DEFAULT_CONFIG } from '../queries'
import type { PerformanceAnalysisConfig } from '@/lib/screen-config'

describe('buildUrl', () => {
  it('returns empty string for the default config', () => {
    expect(buildUrl(DEFAULT_CONFIG)).toBe('')
  })

  it('omits default time range but keeps process_id', () => {
    const cfg: PerformanceAnalysisConfig = { ...DEFAULT_CONFIG, processId: 'p1' }
    expect(buildUrl(cfg)).toBe('?process_id=p1')
  })

  it('serializes non-default time range, measure, properties, and scale', () => {
    const cfg: PerformanceAnalysisConfig = {
      processId: 'p1',
      timeRangeFrom: 'now-6h',
      timeRangeTo: 'now-1h',
      selectedMeasure: 'DeltaTime',
      selectedProperties: ['cpu', 'gpu'],
      scaleMode: 'max',
    }
    const qs = buildUrl(cfg)
    const params = new URLSearchParams(qs.slice(1))
    expect(params.get('process_id')).toBe('p1')
    expect(params.get('from')).toBe('now-6h')
    expect(params.get('to')).toBe('now-1h')
    expect(params.get('measure')).toBe('DeltaTime')
    expect(params.get('properties')).toBe('cpu,gpu')
    expect(params.get('scale')).toBe('max')
  })

  it('omits scale when it is the p99 default and omits empty properties', () => {
    const cfg: PerformanceAnalysisConfig = {
      ...DEFAULT_CONFIG,
      processId: 'p1',
      scaleMode: 'p99',
      selectedProperties: [],
    }
    const params = new URLSearchParams(buildUrl(cfg).slice(1))
    expect(params.has('scale')).toBe(false)
    expect(params.has('properties')).toBe(false)
  })
})

describe('calculateBinInterval', () => {
  it('picks the smallest interval >= the per-bin span', () => {
    // 800px default width: 800ms span -> 1ms per bin -> "1 millisecond"
    expect(calculateBinInterval(800)).toBe('1 millisecond')
  })

  it('scales with the time span', () => {
    // 1 hour over 800 bins -> 4500ms/bin -> first interval >= that is 5s
    expect(calculateBinInterval(3_600_000)).toBe('5 seconds')
  })

  it('respects an explicit chart width', () => {
    // 8000ms over 8 bins -> 1000ms/bin -> "1 second"
    expect(calculateBinInterval(8000, 8)).toBe('1 second')
  })

  it('caps at 1 hour for very large spans', () => {
    // 1e10ms over 800 bins -> 12.5M ms/bin, above every listed interval
    expect(calculateBinInterval(10_000_000_000)).toBe('1 hour')
  })
})
