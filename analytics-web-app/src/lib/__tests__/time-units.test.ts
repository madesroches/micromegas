import {
  isTimeUnit,
  getAdaptiveTimeUnit,
  formatTimeValue,
  formatAdaptiveTime,
} from '../time-units'

describe('isTimeUnit', () => {
  describe('canonical names', () => {
    it('recognizes canonical time units', () => {
      expect(isTimeUnit('nanoseconds')).toBe(true)
      expect(isTimeUnit('microseconds')).toBe(true)
      expect(isTimeUnit('milliseconds')).toBe(true)
      expect(isTimeUnit('seconds')).toBe(true)
      expect(isTimeUnit('minutes')).toBe(true)
      expect(isTimeUnit('hours')).toBe(true)
      expect(isTimeUnit('days')).toBe(true)
    })
  })

  describe('aliases', () => {
    it('recognizes short aliases', () => {
      expect(isTimeUnit('ns')).toBe(true)
      expect(isTimeUnit('µs')).toBe(true)
      expect(isTimeUnit('us')).toBe(true)
      expect(isTimeUnit('ms')).toBe(true)
      expect(isTimeUnit('s')).toBe(true)
      expect(isTimeUnit('min')).toBe(true)
      expect(isTimeUnit('h')).toBe(true)
      expect(isTimeUnit('d')).toBe(true)
    })

    it('recognizes capitalized aliases', () => {
      expect(isTimeUnit('Nanoseconds')).toBe(true)
      expect(isTimeUnit('Microseconds')).toBe(true)
      expect(isTimeUnit('Milliseconds')).toBe(true)
      expect(isTimeUnit('Seconds')).toBe(true)
      expect(isTimeUnit('Minutes')).toBe(true)
      expect(isTimeUnit('Hours')).toBe(true)
      expect(isTimeUnit('Days')).toBe(true)
    })
  })

  describe('non-time units', () => {
    it('rejects non-time units', () => {
      expect(isTimeUnit('bytes')).toBe(false)
      expect(isTimeUnit('percent')).toBe(false)
      expect(isTimeUnit('count')).toBe(false)
      expect(isTimeUnit('custom')).toBe(false)
    })
  })
})

describe('getAdaptiveTimeUnit', () => {
  describe('with canonical units', () => {
    it('works with canonical nanoseconds', () => {
      const result = getAdaptiveTimeUnit(1000, 'nanoseconds')
      expect(result.unit).toBe('microseconds')
      expect(result.abbrev).toBe('µs')
    })

    it('works with canonical milliseconds', () => {
      const result = getAdaptiveTimeUnit(1000, 'milliseconds')
      expect(result.unit).toBe('seconds')
      expect(result.abbrev).toBe('s')
    })
  })

  describe('with aliases', () => {
    it('works with ns alias', () => {
      const result = getAdaptiveTimeUnit(1000, 'ns')
      expect(result.unit).toBe('microseconds')
      expect(result.abbrev).toBe('µs')
    })

    it('works with ms alias', () => {
      const result = getAdaptiveTimeUnit(1000, 'ms')
      expect(result.unit).toBe('seconds')
      expect(result.abbrev).toBe('s')
    })

    it('works with s alias', () => {
      const result = getAdaptiveTimeUnit(0.5, 's')
      expect(result.unit).toBe('milliseconds')
      expect(result.abbrev).toBe('ms')
    })

    it('works with Milliseconds alias', () => {
      const result = getAdaptiveTimeUnit(100, 'Milliseconds')
      expect(result.unit).toBe('milliseconds')
      expect(result.abbrev).toBe('ms')
    })

    it('works with Seconds alias', () => {
      const result = getAdaptiveTimeUnit(60, 'Seconds')
      expect(result.unit).toBe('minutes')
      expect(result.abbrev).toBe('min')
    })
  })
})

describe('formatTimeValue', () => {
  describe('with canonical units', () => {
    it('formats nanoseconds adaptively', () => {
      expect(formatTimeValue(1500, 'nanoseconds')).toBe('1.50 microseconds')
      expect(formatTimeValue(1500, 'nanoseconds', true)).toBe('1.50 µs')
    })

    it('formats milliseconds adaptively', () => {
      expect(formatTimeValue(1500, 'milliseconds')).toBe('1.50 seconds')
      expect(formatTimeValue(1500, 'milliseconds', true)).toBe('1.50 s')
    })
  })

  describe('with aliases', () => {
    it('formats ns alias adaptively', () => {
      expect(formatTimeValue(1500, 'ns', true)).toBe('1.50 µs')
    })

    it('formats ms alias adaptively', () => {
      expect(formatTimeValue(1500, 'ms', true)).toBe('1.50 s')
    })

    it('formats s alias adaptively', () => {
      expect(formatTimeValue(0.02, 's', true)).toBe('20.0 ms')
    })

    it('formats Milliseconds alias adaptively', () => {
      expect(formatTimeValue(100, 'Milliseconds', true)).toBe('100 ms')
    })

    it('formats Seconds alias adaptively', () => {
      expect(formatTimeValue(90, 'Seconds', true)).toBe('1.50 min')
    })
  })
})

describe('formatAdaptiveTime', () => {
  it('formats values with adaptive unit', () => {
    const adaptive = getAdaptiveTimeUnit(1500, 'nanoseconds')
    expect(formatAdaptiveTime(1500, adaptive)).toBe('1.50 microseconds')
    expect(formatAdaptiveTime(1500, adaptive, true)).toBe('1.50 µs')
  })

  it('handles zero values', () => {
    const adaptive = getAdaptiveTimeUnit(100, 'milliseconds')
    expect(formatAdaptiveTime(0, adaptive, true)).toBe('0 ms')
  })

  it('handles large values', () => {
    const adaptive = getAdaptiveTimeUnit(10000, 'milliseconds')
    expect(formatAdaptiveTime(10000, adaptive, true)).toBe('10.0 s')
  })
})
