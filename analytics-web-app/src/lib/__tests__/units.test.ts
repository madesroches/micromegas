import { normalizeUnit, UNIT_ALIASES, TIME_UNIT_NAMES } from '../units'

describe('normalizeUnit', () => {
  describe('time units', () => {
    it('normalizes nanosecond aliases', () => {
      expect(normalizeUnit('ns')).toBe('nanoseconds')
      expect(normalizeUnit('nanoseconds')).toBe('nanoseconds')
      expect(normalizeUnit('Nanoseconds')).toBe('nanoseconds')
    })

    it('normalizes microsecond aliases', () => {
      expect(normalizeUnit('µs')).toBe('microseconds')
      expect(normalizeUnit('us')).toBe('microseconds')
      expect(normalizeUnit('microseconds')).toBe('microseconds')
      expect(normalizeUnit('Microseconds')).toBe('microseconds')
    })

    it('normalizes millisecond aliases', () => {
      expect(normalizeUnit('ms')).toBe('milliseconds')
      expect(normalizeUnit('milliseconds')).toBe('milliseconds')
      expect(normalizeUnit('Milliseconds')).toBe('milliseconds')
    })

    it('normalizes second aliases', () => {
      expect(normalizeUnit('s')).toBe('seconds')
      expect(normalizeUnit('seconds')).toBe('seconds')
      expect(normalizeUnit('Seconds')).toBe('seconds')
    })

    it('normalizes minute aliases', () => {
      expect(normalizeUnit('min')).toBe('minutes')
      expect(normalizeUnit('minutes')).toBe('minutes')
      expect(normalizeUnit('Minutes')).toBe('minutes')
    })

    it('normalizes hour aliases', () => {
      expect(normalizeUnit('h')).toBe('hours')
      expect(normalizeUnit('hours')).toBe('hours')
      expect(normalizeUnit('Hours')).toBe('hours')
    })

    it('normalizes day aliases', () => {
      expect(normalizeUnit('d')).toBe('days')
      expect(normalizeUnit('days')).toBe('days')
      expect(normalizeUnit('Days')).toBe('days')
    })
  })

  describe('size units', () => {
    it('normalizes byte aliases', () => {
      expect(normalizeUnit('bytes')).toBe('bytes')
      expect(normalizeUnit('Bytes')).toBe('bytes')
      expect(normalizeUnit('B')).toBe('bytes')
    })

    it('normalizes kilobyte aliases', () => {
      expect(normalizeUnit('kilobytes')).toBe('kilobytes')
      expect(normalizeUnit('Kilobytes')).toBe('kilobytes')
      expect(normalizeUnit('KB')).toBe('kilobytes')
      expect(normalizeUnit('kb')).toBe('kilobytes')
    })

    it('normalizes megabyte aliases', () => {
      expect(normalizeUnit('megabytes')).toBe('megabytes')
      expect(normalizeUnit('Megabytes')).toBe('megabytes')
      expect(normalizeUnit('MB')).toBe('megabytes')
    })

    it('normalizes gigabyte aliases', () => {
      expect(normalizeUnit('gigabytes')).toBe('gigabytes')
      expect(normalizeUnit('Gigabytes')).toBe('gigabytes')
      expect(normalizeUnit('GB')).toBe('gigabytes')
    })
  })

  describe('rate units', () => {
    it('normalizes bytes per second aliases', () => {
      expect(normalizeUnit('BytesPerSecond')).toBe('bytes/s')
      expect(normalizeUnit('BytesPerSeconds')).toBe('bytes/s')
      expect(normalizeUnit('B/s')).toBe('bytes/s')
      expect(normalizeUnit('bytes/s')).toBe('bytes/s')
    })
  })

  describe('other units', () => {
    it('normalizes percent aliases', () => {
      expect(normalizeUnit('percent')).toBe('percent')
      expect(normalizeUnit('%')).toBe('percent')
    })

    it('normalizes degree aliases', () => {
      expect(normalizeUnit('degrees')).toBe('degrees')
      expect(normalizeUnit('deg')).toBe('degrees')
    })

    it('normalizes boolean', () => {
      expect(normalizeUnit('boolean')).toBe('boolean')
    })
  })

  describe('unknown units', () => {
    it('returns unknown units unchanged', () => {
      expect(normalizeUnit('custom_unit')).toBe('custom_unit')
      expect(normalizeUnit('meters')).toBe('meters')
      expect(normalizeUnit('rpm')).toBe('rpm')
      expect(normalizeUnit('none')).toBe('none')
      expect(normalizeUnit('count')).toBe('count')
      expect(normalizeUnit('requests')).toBe('requests')
      expect(normalizeUnit('')).toBe('')
    })
  })
})

describe('TIME_UNIT_NAMES', () => {
  it('contains all canonical time units', () => {
    expect(TIME_UNIT_NAMES.has('nanoseconds')).toBe(true)
    expect(TIME_UNIT_NAMES.has('microseconds')).toBe(true)
    expect(TIME_UNIT_NAMES.has('milliseconds')).toBe(true)
    expect(TIME_UNIT_NAMES.has('seconds')).toBe(true)
    expect(TIME_UNIT_NAMES.has('minutes')).toBe(true)
    expect(TIME_UNIT_NAMES.has('hours')).toBe(true)
    expect(TIME_UNIT_NAMES.has('days')).toBe(true)
  })

  it('does not contain non-time units', () => {
    expect(TIME_UNIT_NAMES.has('bytes')).toBe(false)
    expect(TIME_UNIT_NAMES.has('percent')).toBe(false)
  })
})

describe('UNIT_ALIASES', () => {
  it('maps all time unit aliases to canonical names', () => {
    const timeAliases = ['ns', 'µs', 'us', 'ms', 's', 'min', 'h', 'd']
    for (const alias of timeAliases) {
      expect(TIME_UNIT_NAMES.has(UNIT_ALIASES[alias])).toBe(true)
    }
  })
})
