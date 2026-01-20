/**
 * Tests for URL parameter parsing utilities
 */
import { parseUrlParams } from '../url-params'

describe('parseUrlParams', () => {
  describe('string params', () => {
    it('should parse process_id', () => {
      const params = new URLSearchParams('process_id=abc-123')
      const result = parseUrlParams(params)
      expect(result.processId).toBe('abc-123')
    })

    it('should parse from and to as timeRangeFrom/To', () => {
      const params = new URLSearchParams('from=now-1h&to=now')
      const result = parseUrlParams(params)
      expect(result.timeRangeFrom).toBe('now-1h')
      expect(result.timeRangeTo).toBe('now')
    })

    it('should parse measure as selectedMeasure', () => {
      const params = new URLSearchParams('measure=DeltaTime')
      const result = parseUrlParams(params)
      expect(result.selectedMeasure).toBe('DeltaTime')
    })

    it('should parse scale as scaleMode', () => {
      const params = new URLSearchParams('scale=max')
      const result = parseUrlParams(params)
      expect(result.scaleMode).toBe('max')
    })

    it('should parse level as logLevel', () => {
      const params = new URLSearchParams('level=error')
      const result = parseUrlParams(params)
      expect(result.logLevel).toBe('error')
    })

    it('should parse search', () => {
      const params = new URLSearchParams('search=test%20query')
      const result = parseUrlParams(params)
      expect(result.search).toBe('test query')
    })

    it('should parse sort as sortField', () => {
      const params = new URLSearchParams('sort=exe')
      const result = parseUrlParams(params)
      expect(result.sortField).toBe('exe')
    })

    it('should parse dir as sortDirection', () => {
      const params = new URLSearchParams('dir=desc')
      const result = parseUrlParams(params)
      expect(result.sortDirection).toBe('desc')
    })
  })

  describe('number params', () => {
    it('should parse limit as logLimit', () => {
      const params = new URLSearchParams('limit=500')
      const result = parseUrlParams(params)
      expect(result.logLimit).toBe(500)
    })

    it('should ignore invalid limit values', () => {
      const params = new URLSearchParams('limit=not-a-number')
      const result = parseUrlParams(params)
      expect(result.logLimit).toBeUndefined()
    })

    it('should handle empty limit value', () => {
      const params = new URLSearchParams('limit=')
      const result = parseUrlParams(params)
      expect(result.logLimit).toBeUndefined()
    })
  })

  describe('array params', () => {
    it('should parse properties as comma-separated array', () => {
      const params = new URLSearchParams('properties=cpu,memory,disk')
      const result = parseUrlParams(params)
      expect(result.selectedProperties).toEqual(['cpu', 'memory', 'disk'])
    })

    it('should handle single property', () => {
      const params = new URLSearchParams('properties=cpu')
      const result = parseUrlParams(params)
      expect(result.selectedProperties).toEqual(['cpu'])
    })

    it('should handle empty properties value', () => {
      const params = new URLSearchParams('properties=')
      const result = parseUrlParams(params)
      expect(result.selectedProperties).toEqual([])
    })

    it('should filter out empty strings from properties', () => {
      const params = new URLSearchParams('properties=cpu,,memory')
      const result = parseUrlParams(params)
      expect(result.selectedProperties).toEqual(['cpu', 'memory'])
    })
  })

  describe('missing params', () => {
    it('should return empty object for empty params', () => {
      const params = new URLSearchParams('')
      const result = parseUrlParams(params)
      expect(Object.keys(result)).toHaveLength(0)
    })

    it('should only include params that are present', () => {
      const params = new URLSearchParams('from=now-1h')
      const result = parseUrlParams(params)
      expect(result.timeRangeFrom).toBe('now-1h')
      expect(result.timeRangeTo).toBeUndefined()
      expect(result.processId).toBeUndefined()
    })
  })

  describe('combined params', () => {
    it('should parse multiple params together', () => {
      const params = new URLSearchParams(
        'process_id=abc&from=now-24h&to=now&measure=DeltaTime&properties=cpu,memory&scale=p99&limit=200'
      )
      const result = parseUrlParams(params)
      expect(result).toEqual({
        processId: 'abc',
        timeRangeFrom: 'now-24h',
        timeRangeTo: 'now',
        selectedMeasure: 'DeltaTime',
        selectedProperties: ['cpu', 'memory'],
        scaleMode: 'p99',
        logLimit: 200,
      })
    })
  })
})
