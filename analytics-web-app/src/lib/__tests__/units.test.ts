import { normalizeUnit, UNIT_ALIASES, TIME_UNIT_NAMES, SIZE_UNIT_NAMES, isSizeUnit, getAdaptiveSizeUnit, isBitUnit, getAdaptiveBitUnit } from '../units'

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

    it('normalizes terabyte aliases', () => {
      expect(normalizeUnit('terabytes')).toBe('terabytes')
      expect(normalizeUnit('Terabytes')).toBe('terabytes')
      expect(normalizeUnit('TB')).toBe('terabytes')
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

describe('SIZE_UNIT_NAMES', () => {
  it('contains all canonical size units', () => {
    expect(SIZE_UNIT_NAMES.has('bytes')).toBe(true)
    expect(SIZE_UNIT_NAMES.has('kilobytes')).toBe(true)
    expect(SIZE_UNIT_NAMES.has('megabytes')).toBe(true)
    expect(SIZE_UNIT_NAMES.has('gigabytes')).toBe(true)
    expect(SIZE_UNIT_NAMES.has('terabytes')).toBe(true)
  })

  it('does not contain non-size units', () => {
    expect(SIZE_UNIT_NAMES.has('nanoseconds')).toBe(false)
    expect(SIZE_UNIT_NAMES.has('percent')).toBe(false)
  })
})

describe('isSizeUnit', () => {
  it('recognizes canonical size units', () => {
    expect(isSizeUnit('bytes')).toBe(true)
    expect(isSizeUnit('kilobytes')).toBe(true)
    expect(isSizeUnit('megabytes')).toBe(true)
    expect(isSizeUnit('gigabytes')).toBe(true)
    expect(isSizeUnit('terabytes')).toBe(true)
  })

  it('recognizes size unit aliases', () => {
    expect(isSizeUnit('B')).toBe(true)
    expect(isSizeUnit('KB')).toBe(true)
    expect(isSizeUnit('MB')).toBe(true)
    expect(isSizeUnit('GB')).toBe(true)
    expect(isSizeUnit('TB')).toBe(true)
    expect(isSizeUnit('Bytes')).toBe(true)
  })

  it('rejects non-size units', () => {
    expect(isSizeUnit('nanoseconds')).toBe(false)
    expect(isSizeUnit('percent')).toBe(false)
    expect(isSizeUnit('count')).toBe(false)
  })
})

describe('getAdaptiveSizeUnit', () => {
  // Binary size constants for test calculations
  const KB = 1024
  const MB = KB * 1024
  const GB = MB * 1024
  const TB = GB * 1024

  describe('from bytes', () => {
    it('stays in bytes for small values', () => {
      const result = getAdaptiveSizeUnit(500, 'bytes')
      expect(result.unit).toBe('bytes')
      expect(result.abbrev).toBe('B')
      expect(result.conversionFactor).toBe(1)
    })

    it('converts to KB for values >= 1024', () => {
      const result = getAdaptiveSizeUnit(5 * KB, 'bytes')
      expect(result.unit).toBe('kilobytes')
      expect(result.abbrev).toBe('KB')
      expect(result.conversionFactor).toBe(1 / KB)
    })

    it('converts to MB for values >= 1 MB', () => {
      const result = getAdaptiveSizeUnit(5 * MB, 'bytes')
      expect(result.unit).toBe('megabytes')
      expect(result.abbrev).toBe('MB')
      expect(result.conversionFactor).toBe(1 / MB)
    })

    it('converts to GB for values >= 1 GB', () => {
      const result = getAdaptiveSizeUnit(5 * GB, 'bytes')
      expect(result.unit).toBe('gigabytes')
      expect(result.abbrev).toBe('GB')
      expect(result.conversionFactor).toBe(1 / GB)
    })

    it('converts to TB for values >= 1 TB', () => {
      const result = getAdaptiveSizeUnit(5 * TB, 'bytes')
      expect(result.unit).toBe('terabytes')
      expect(result.abbrev).toBe('TB')
      expect(result.conversionFactor).toBe(1 / TB)
    })
  })

  describe('from kilobytes', () => {
    it('converts to MB for values >= 1024 KB', () => {
      const result = getAdaptiveSizeUnit(5 * KB, 'kilobytes')
      expect(result.unit).toBe('megabytes')
      expect(result.abbrev).toBe('MB')
      expect(result.conversionFactor).toBe(1 / KB)
    })

    it('converts to GB for values >= 1 GB in KB', () => {
      const result = getAdaptiveSizeUnit(5 * MB, 'kilobytes')
      expect(result.unit).toBe('gigabytes')
      expect(result.abbrev).toBe('GB')
      expect(result.conversionFactor).toBe(1 / MB)
    })
  })

  describe('with aliases', () => {
    it('works with B alias', () => {
      const result = getAdaptiveSizeUnit(5 * MB, 'B')
      expect(result.unit).toBe('megabytes')
      expect(result.abbrev).toBe('MB')
    })

    it('works with KB alias', () => {
      const result = getAdaptiveSizeUnit(5 * KB, 'KB')
      expect(result.unit).toBe('megabytes')
      expect(result.abbrev).toBe('MB')
    })

    it('works with Bytes alias', () => {
      const result = getAdaptiveSizeUnit(5 * GB, 'Bytes')
      expect(result.unit).toBe('gigabytes')
      expect(result.abbrev).toBe('GB')
    })
  })

  describe('bytes/s rate', () => {
    it('scales bytes/s and appends /s suffix', () => {
      const result = getAdaptiveSizeUnit(5 * MB, 'bytes/s')
      expect(result.unit).toBe('megabytes')
      expect(result.abbrev).toBe('MB/s')
      expect(result.conversionFactor).toBe(1 / MB)
    })

    it('stays in B/s for small rates', () => {
      const result = getAdaptiveSizeUnit(500, 'bytes/s')
      expect(result.abbrev).toBe('B/s')
      expect(result.conversionFactor).toBe(1)
    })
  })
})

describe('isBitUnit', () => {
  it('recognizes canonical bit units', () => {
    expect(isBitUnit('bits')).toBe(true)
    expect(isBitUnit('kilobits')).toBe(true)
    expect(isBitUnit('megabits')).toBe(true)
    expect(isBitUnit('gigabits')).toBe(true)
    expect(isBitUnit('terabits')).toBe(true)
  })

  it('recognizes the bits/s rate variant', () => {
    expect(isBitUnit('bits/s')).toBe(true)
    expect(isBitUnit('bps')).toBe(true)
    expect(isBitUnit('bit/s')).toBe(true)
  })

  it('rejects non-bit units', () => {
    expect(isBitUnit('bytes')).toBe(false)
    expect(isBitUnit('seconds')).toBe(false)
  })
})

describe('isSizeUnit bytes/s', () => {
  it('recognizes the bytes/s rate variant', () => {
    expect(isSizeUnit('bytes/s')).toBe(true)
    expect(isSizeUnit('B/s')).toBe(true)
    expect(isSizeUnit('BytesPerSecond')).toBe(true)
  })
})

describe('getAdaptiveBitUnit', () => {
  const KBIT = 1000
  const MBIT = KBIT * 1000
  const GBIT = MBIT * 1000

  it('stays in bits for small values', () => {
    const result = getAdaptiveBitUnit(500, 'bits')
    expect(result.unit).toBe('bits')
    expect(result.abbrev).toBe('bit')
    expect(result.conversionFactor).toBe(1)
  })

  it('scales to Mbit for values around 5,000,000 bits', () => {
    const result = getAdaptiveBitUnit(5 * MBIT, 'bits')
    expect(result.unit).toBe('megabits')
    expect(result.abbrev).toBe('Mbit')
    expect(result.conversionFactor).toBe(1 / MBIT)
  })

  it('scales to Gbit for values >= 1 Gbit', () => {
    const result = getAdaptiveBitUnit(2 * GBIT, 'bits')
    expect(result.unit).toBe('gigabits')
    expect(result.abbrev).toBe('Gbit')
  })

  describe('bits/s rate', () => {
    it('scales bits/s and appends /s suffix', () => {
      const result = getAdaptiveBitUnit(5 * MBIT, 'bits/s')
      expect(result.unit).toBe('megabits')
      expect(result.abbrev).toBe('Mbit/s')
      expect(result.conversionFactor).toBe(1 / MBIT)
    })

    it('uses kbit/s for kilobit-range rates', () => {
      const result = getAdaptiveBitUnit(50 * KBIT, 'bits/s')
      expect(result.abbrev).toBe('kbit/s')
      expect(result.conversionFactor).toBe(1 / KBIT)
    })
  })
})
