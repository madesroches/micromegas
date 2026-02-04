import {
  extractPropertiesFromRows,
  createPropertyTimelineGetter,
  aggregateIntoSegments,
} from '../property-utils'

describe('extractPropertiesFromRows', () => {
  it('returns empty results for empty input', () => {
    const result = extractPropertiesFromRows([])
    expect(result.availableKeys).toEqual([])
    expect(result.rawData.size).toBe(0)
    expect(result.errors).toEqual([])
  })

  it('extracts keys from valid JSON properties', () => {
    const rows = [
      { time: 1000, properties: '{"cpu": 50, "memory": 100}' },
      { time: 2000, properties: '{"cpu": 60, "disk": 200}' },
    ]
    const result = extractPropertiesFromRows(rows)
    expect(result.availableKeys).toEqual(['cpu', 'disk', 'memory'])
    expect(result.rawData.size).toBe(2)
    expect(result.errors).toEqual([])
  })

  it('handles null properties', () => {
    const rows = [
      { time: 1000, properties: '{"cpu": 50}' },
      { time: 2000, properties: null },
      { time: 3000, properties: '{"memory": 100}' },
    ]
    const result = extractPropertiesFromRows(rows)
    expect(result.availableKeys).toEqual(['cpu', 'memory'])
    expect(result.rawData.size).toBe(2)
    expect(result.errors).toEqual([])
  })

  it('collects errors for invalid JSON', () => {
    const rows = [
      { time: 1000, properties: '{"cpu": 50}' },
      { time: 2000, properties: 'invalid json' },
      { time: 3000, properties: '{broken' },
    ]
    const result = extractPropertiesFromRows(rows)
    expect(result.availableKeys).toEqual(['cpu'])
    expect(result.rawData.size).toBe(1)
    expect(result.errors).toHaveLength(2)
    expect(result.errors[0]).toContain('Invalid JSON at time 2000')
    expect(result.errors[1]).toContain('Invalid JSON at time 3000')
  })

  it('sorts available keys alphabetically', () => {
    const rows = [
      { time: 1000, properties: '{"zebra": 1, "apple": 2, "mango": 3}' },
    ]
    const result = extractPropertiesFromRows(rows)
    expect(result.availableKeys).toEqual(['apple', 'mango', 'zebra'])
  })
})

describe('aggregateIntoSegments', () => {
  it('returns empty array for empty input', () => {
    const result = aggregateIntoSegments([])
    expect(result).toEqual([])
  })

  it('creates single segment for single row', () => {
    const rows = [{ time: 1000, value: 'running' }]
    const result = aggregateIntoSegments(rows, { begin: 0, end: 5000 })
    expect(result).toEqual([
      { value: 'running', begin: 1000, end: 5000 },
    ])
  })

  it('merges adjacent rows with same value', () => {
    const rows = [
      { time: 1000, value: 'running' },
      { time: 2000, value: 'running' },
      { time: 3000, value: 'running' },
    ]
    const result = aggregateIntoSegments(rows, { begin: 0, end: 5000 })
    expect(result).toEqual([
      { value: 'running', begin: 1000, end: 5000 },
    ])
  })

  it('creates separate segments for different values', () => {
    const rows = [
      { time: 1000, value: 'running' },
      { time: 2000, value: 'paused' },
      { time: 3000, value: 'running' },
    ]
    const result = aggregateIntoSegments(rows, { begin: 0, end: 5000 })
    expect(result).toEqual([
      { value: 'running', begin: 1000, end: 2000 },
      { value: 'paused', begin: 2000, end: 3000 },
      { value: 'running', begin: 3000, end: 5000 },
    ])
  })

  it('uses next row time as segment end when no timeRange', () => {
    const rows = [
      { time: 1000, value: 'a' },
      { time: 2000, value: 'b' },
      { time: 3000, value: 'c' },
    ]
    const result = aggregateIntoSegments(rows)
    expect(result).toEqual([
      { value: 'a', begin: 1000, end: 2000 },
      { value: 'b', begin: 2000, end: 3000 },
      { value: 'c', begin: 3000, end: 3000 },
    ])
  })

  it('handles complex value change pattern', () => {
    const rows = [
      { time: 1000, value: 'a' },
      { time: 2000, value: 'a' },
      { time: 3000, value: 'b' },
      { time: 4000, value: 'b' },
      { time: 5000, value: 'a' },
    ]
    const result = aggregateIntoSegments(rows, { begin: 0, end: 6000 })
    expect(result).toEqual([
      { value: 'a', begin: 1000, end: 3000 },
      { value: 'b', begin: 3000, end: 5000 },
      { value: 'a', begin: 5000, end: 6000 },
    ])
  })
})

describe('createPropertyTimelineGetter', () => {
  it('returns empty segments for non-existent property', () => {
    const rawData = new Map<number, Record<string, unknown>>([
      [1000, { cpu: 50 }],
      [2000, { cpu: 60 }],
    ])
    const getTimeline = createPropertyTimelineGetter(rawData)
    const result = getTimeline('memory')
    expect(result.propertyName).toBe('memory')
    expect(result.segments).toEqual([])
  })

  it('creates timeline for existing property', () => {
    const rawData = new Map<number, Record<string, unknown>>([
      [1000, { status: 'running' }],
      [2000, { status: 'paused' }],
      [3000, { status: 'running' }],
    ])
    const getTimeline = createPropertyTimelineGetter(rawData, { begin: 0, end: 5000 })
    const result = getTimeline('status')
    expect(result.propertyName).toBe('status')
    expect(result.segments).toEqual([
      { value: 'running', begin: 1000, end: 2000 },
      { value: 'paused', begin: 2000, end: 3000 },
      { value: 'running', begin: 3000, end: 5000 },
    ])
  })

  it('skips rows where property is undefined', () => {
    const rawData = new Map<number, Record<string, unknown>>([
      [1000, { status: 'running' }],
      [2000, { other: 'value' }],
      [3000, { status: 'paused' }],
    ])
    const getTimeline = createPropertyTimelineGetter(rawData, { begin: 0, end: 5000 })
    const result = getTimeline('status')
    expect(result.segments).toEqual([
      { value: 'running', begin: 1000, end: 3000 },
      { value: 'paused', begin: 3000, end: 5000 },
    ])
  })

  it('skips rows where property is null', () => {
    const rawData = new Map<number, Record<string, unknown>>([
      [1000, { status: 'running' }],
      [2000, { status: null }],
      [3000, { status: 'paused' }],
    ])
    const getTimeline = createPropertyTimelineGetter(rawData, { begin: 0, end: 5000 })
    const result = getTimeline('status')
    expect(result.segments).toEqual([
      { value: 'running', begin: 1000, end: 3000 },
      { value: 'paused', begin: 3000, end: 5000 },
    ])
  })

  it('converts non-string values to strings', () => {
    const rawData = new Map<number, Record<string, unknown>>([
      [1000, { count: 42 }],
      [2000, { count: 100 }],
    ])
    const getTimeline = createPropertyTimelineGetter(rawData, { begin: 0, end: 3000 })
    const result = getTimeline('count')
    expect(result.segments).toEqual([
      { value: '42', begin: 1000, end: 2000 },
      { value: '100', begin: 2000, end: 3000 },
    ])
  })

  it('sorts entries by time', () => {
    const rawData = new Map<number, Record<string, unknown>>([
      [3000, { status: 'c' }],
      [1000, { status: 'a' }],
      [2000, { status: 'b' }],
    ])
    const getTimeline = createPropertyTimelineGetter(rawData, { begin: 0, end: 4000 })
    const result = getTimeline('status')
    expect(result.segments).toEqual([
      { value: 'a', begin: 1000, end: 2000 },
      { value: 'b', begin: 2000, end: 3000 },
      { value: 'c', begin: 3000, end: 4000 },
    ])
  })
})
