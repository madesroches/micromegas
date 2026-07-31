import {
  normalizeUnit,
  UNIT_ALIASES,
  TIME_UNIT_NAMES,
  SIZE_UNIT_NAMES,
  BIT_UNIT_NAMES,
  isSizeUnit,
  getAdaptiveSizeUnit,
  isBitUnit,
  getAdaptiveBitUnit,
  isCurrencyUnit,
  formatCurrencyValue,
  unitScaleKey,
  unitDisplayAbbrev,
  unitSuffix,
} from '../units'

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
      expect(normalizeUnit('requests')).toBe('requests')
      expect(normalizeUnit('')).toBe('')
    })
  })

  describe('dimensionless units', () => {
    it('normalizes none/count/unit spellings to the empty string', () => {
      expect(normalizeUnit('none')).toBe('')
      expect(normalizeUnit('None')).toBe('')
      expect(normalizeUnit('count')).toBe('')
      expect(normalizeUnit('Count')).toBe('')
      expect(normalizeUnit('counts')).toBe('')
      expect(normalizeUnit('1')).toBe('')
      expect(normalizeUnit('units')).toBe('')
      expect(normalizeUnit('unit')).toBe('')
      expect(normalizeUnit('iterations')).toBe('')
    })

    it('normalizes dimensionless rates to /s', () => {
      expect(normalizeUnit('1/s')).toBe('/s')
      expect(normalizeUnit('count/s')).toBe('/s')
    })
  })

  describe('temperature and length', () => {
    it('normalizes Cel/celsius to celsius', () => {
      expect(normalizeUnit('Cel')).toBe('celsius')
      expect(normalizeUnit('celsius')).toBe('celsius')
    })

    it('normalizes cm/centimeters to centimeters', () => {
      expect(normalizeUnit('cm')).toBe('centimeters')
      expect(normalizeUnit('centimeters')).toBe('centimeters')
    })
  })

  describe('UCUM annotations', () => {
    it('strips annotations on otherwise dimensionless quantities', () => {
      expect(normalizeUnit('{Count}')).toBe('')
      expect(normalizeUnit('{request}')).toBe('')
      expect(normalizeUnit('{connection}')).toBe('')
      expect(normalizeUnit('1')).toBe('')
    })

    it('strips annotations on dimensionless rates', () => {
      expect(normalizeUnit('{request}/s')).toBe('/s')
    })

    it('strips annotations on real units, then resolves the remainder', () => {
      expect(normalizeUnit('By{net}')).toBe('bytes')
    })

    it('table lookup runs first: {Count} is resolved by the annotation rule, not a table entry', () => {
      // There is no literal '{Count}' key in the table; normalizeUnit only
      // reaches '' by stripping the annotation and re-looking-up 'Count'.
      expect(UNIT_ALIASES['{Count}']).toBeUndefined()
      expect(normalizeUnit('{Count}')).toBe('')
    })
  })

  describe('CloudWatch/OTLP UCUM codes', () => {
    const sizeCases: [string, string][] = [
      ['By', 'bytes'], ['B', 'bytes'],
      ['kBy', 'kilobytes'], ['KiBy', 'kilobytes'], ['KB', 'kilobytes'], ['kb', 'kilobytes'],
      ['MBy', 'megabytes'], ['MiBy', 'megabytes'], ['MB', 'megabytes'],
      ['GBy', 'gigabytes'], ['GiBy', 'gigabytes'], ['GB', 'gigabytes'],
      ['TBy', 'terabytes'], ['TiBy', 'terabytes'], ['TB', 'terabytes'],
    ]
    const bitCases: [string, string][] = [
      ['bit', 'bits'], ['bits', 'bits'],
      ['kBit', 'kilobits'], ['kbit', 'kilobits'],
      ['MBit', 'megabits'], ['Mbit', 'megabits'],
      ['GBit', 'gigabits'], ['Gbit', 'gigabits'],
      ['TBit', 'terabits'], ['Tbit', 'terabits'],
    ]

    it('normalizes every bare size/bit code to its canonical name', () => {
      for (const [code, canonical] of [...sizeCases, ...bitCases]) {
        expect(normalizeUnit(code)).toBe(canonical)
      }
    })

    it('normalizes every size/bit code with a /s suffix to the canonical rate', () => {
      for (const [code, canonical] of [...sizeCases, ...bitCases]) {
        expect(normalizeUnit(`${code}/s`)).toBe(`${canonical}/s`)
      }
    })

    it('normalizes legacy plural/capitalized byte and bit rate aliases (not just their bare forms)', () => {
      expect(normalizeUnit('Mbits/s')).toBe('megabits/s')
      expect(normalizeUnit('Mbits')).toBe('megabits')
      expect(normalizeUnit('Bytes/s')).toBe('bytes/s')
      expect(normalizeUnit('Kilobits/s')).toBe('kilobits/s')
    })

    it('both bit-prefix spellings map to the same canonical unit', () => {
      expect(normalizeUnit('MBit')).toBe('megabits')
      expect(normalizeUnit('Mbit')).toBe('megabits')
      expect(normalizeUnit('GBit')).toBe('gigabits')
      expect(normalizeUnit('Gbit')).toBe('gigabits')
      expect(normalizeUnit('TBit')).toBe('terabits')
      expect(normalizeUnit('Tbit')).toBe('terabits')
      expect(normalizeUnit('kBit')).toBe('kilobits')
      expect(normalizeUnit('kbit')).toBe('kilobits')
    })
  })

  describe('case sensitivity', () => {
    it('B (bel, legacy bytes alias) and By (UCUM byte) both resolve to bytes, as distinct keys', () => {
      expect(normalizeUnit('B')).toBe('bytes')
      expect(normalizeUnit('By')).toBe('bytes')
    })

    it('no lowercasing is introduced: B (bytes) and bit (bits) stay distinct', () => {
      expect(normalizeUnit('B')).toBe('bytes')
      expect(normalizeUnit('bit')).toBe('bits')
      expect(normalizeUnit('B')).not.toBe(normalizeUnit('bit'))
    })

    it('does not fold case before lookup', () => {
      expect(normalizeUnit('bY')).toBe('bY') // not a known alias, passthrough
    })
  })

  describe('currency collision check', () => {
    it('no alias normalizes to a value isCurrencyUnit accepts', () => {
      for (const canonical of Object.values(UNIT_ALIASES)) {
        expect(isCurrencyUnit(canonical)).toBe(false)
      }
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

  it('rejects dimensionless units', () => {
    expect(isSizeUnit('')).toBe(false)
    expect(isSizeUnit('none')).toBe(false)
    expect(isSizeUnit('{Count}')).toBe(false)
  })

  it('accepts every prefixed size rate', () => {
    for (const code of ['kBy/s', 'MBy/s', 'GBy/s', 'TBy/s', 'By/s']) {
      expect(isSizeUnit(code)).toBe(true)
    }
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

  describe('prefixed rate units (UCUM)', () => {
    it('scales MBy/s with the 1024^2 factor', () => {
      const result = getAdaptiveSizeUnit(1, 'MBy/s')
      expect(result.abbrev).toBe('MB/s')
      expect(result.conversionFactor).toBe(1)
    })

    it("the issue's headline case: a large By/s reference scales to GB/s", () => {
      const result = getAdaptiveSizeUnit(1_234_567_890, 'By/s')
      expect(result.unit).toBe('gigabytes')
      expect(result.abbrev).toBe('GB/s')
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

  it('rejects dimensionless units', () => {
    expect(isBitUnit('')).toBe(false)
    expect(isBitUnit('none')).toBe(false)
    expect(isBitUnit('percent')).toBe(false)
  })

  it('accepts every prefixed bit rate', () => {
    for (const code of ['kBit/s', 'kbit/s', 'MBit/s', 'Mbit/s', 'GBit/s', 'Gbit/s', 'TBit/s', 'Tbit/s']) {
      expect(isBitUnit(code)).toBe(true)
    }
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

  describe('prefixed rate units (UCUM)', () => {
    it('scales GBit/s with the decimal factor', () => {
      const result = getAdaptiveBitUnit(1, 'GBit/s')
      expect(result.abbrev).toBe('Gbit/s')
      expect(result.conversionFactor).toBe(1)
    })
  })
})

describe('isCurrencyUnit', () => {
  it('recognizes known currency codes', () => {
    expect(isCurrencyUnit('USD')).toBe(true)
    expect(isCurrencyUnit('CAD')).toBe(true)
    expect(isCurrencyUnit('EUR')).toBe(true)
  })

  it('is case-insensitive', () => {
    expect(isCurrencyUnit('usd')).toBe(true)
  })

  it('rejects non-currency units', () => {
    expect(isCurrencyUnit('count')).toBe(false)
    expect(isCurrencyUnit('percent')).toBe(false)
    expect(isCurrencyUnit('widgets')).toBe(false)
  })

  it('rejects plausible non-currency 3-letter unit codes', () => {
    expect(isCurrencyUnit('MPH')).toBe(false)
    expect(isCurrencyUnit('RPM')).toBe(false)
    expect(isCurrencyUnit('Cel')).toBe(false)
  })
})

describe('formatCurrencyValue', () => {
  it('formats USD to match Intl.NumberFormat currency style', () => {
    const expected = new Intl.NumberFormat(undefined, { style: 'currency', currency: 'USD' }).format(1234.5)
    expect(formatCurrencyValue(1234.5, 'USD')).toBe(expected)
  })

  it('formats CAD to match Intl.NumberFormat currency style', () => {
    const expected = new Intl.NumberFormat(undefined, { style: 'currency', currency: 'CAD' }).format(1234.5)
    expect(formatCurrencyValue(1234.5, 'CAD')).toBe(expected)
  })

  it('formats EUR to match Intl.NumberFormat currency style', () => {
    const expected = new Intl.NumberFormat(undefined, { style: 'currency', currency: 'EUR' }).format(1234.5)
    expect(formatCurrencyValue(1234.5, 'EUR')).toBe(expected)
  })

  it('accepts lowercase currency codes', () => {
    const expected = new Intl.NumberFormat(undefined, { style: 'currency', currency: 'USD' }).format(1234.5)
    expect(formatCurrencyValue(1234.5, 'usd')).toBe(expected)
  })
})

describe('unitScaleKey', () => {
  it('groups equivalent raw unit spellings onto the same scale key', () => {
    expect(unitScaleKey('bytes')).toBe(unitScaleKey('B'))
    expect(unitScaleKey('bytes')).toBe(unitScaleKey('By'))
    expect(unitScaleKey('B')).toBe(unitScaleKey('By'))
  })

  it('returns the empty string for absent/dimensionless units', () => {
    expect(unitScaleKey(undefined)).toBe('')
    expect(unitScaleKey(null)).toBe('')
    expect(unitScaleKey('')).toBe('')
    expect(unitScaleKey('none')).toBe('')
    expect(unitScaleKey('count')).toBe('')
  })
})

describe('unitDisplayAbbrev', () => {
  it('maps symbol-bearing canonical units to their short forms', () => {
    expect(unitDisplayAbbrev('percent')).toBe('%')
    expect(unitDisplayAbbrev('degrees')).toBe('°')
    expect(unitDisplayAbbrev('celsius')).toBe('°C')
    expect(unitDisplayAbbrev('centimeters')).toBe('cm')
    expect(unitDisplayAbbrev('')).toBe('')
  })

  it('maps time canonical names to their short forms', () => {
    expect(unitDisplayAbbrev('nanoseconds')).toBe('ns')
    expect(unitDisplayAbbrev('microseconds')).toBe('µs')
    expect(unitDisplayAbbrev('milliseconds')).toBe('ms')
    expect(unitDisplayAbbrev('seconds')).toBe('s')
    expect(unitDisplayAbbrev('minutes')).toBe('min')
    expect(unitDisplayAbbrev('hours')).toBe('h')
    expect(unitDisplayAbbrev('days')).toBe('d')
  })

  it('maps size/bit canonical names, and their /s rate forms, to their short forms', () => {
    expect(unitDisplayAbbrev('kilobytes')).toBe('KB')
    expect(unitDisplayAbbrev('megabits')).toBe('Mbit')
    expect(unitDisplayAbbrev('bytes/s')).toBe('B/s')
    expect(unitDisplayAbbrev('kilobits/s')).toBe('kbit/s')
  })

  it('passes through any other, genuinely unmapped canonical name unchanged', () => {
    expect(unitDisplayAbbrev('widgets')).toBe('widgets')
  })

  it('never falls through to the spelled-out canonical name for an adaptive family', () => {
    for (const name of TIME_UNIT_NAMES) {
      expect(unitDisplayAbbrev(name)).not.toBe(name)
    }
    for (const name of SIZE_UNIT_NAMES) {
      expect(unitDisplayAbbrev(name)).not.toBe(name)
      expect(unitDisplayAbbrev(`${name}/s`)).not.toBe(`${name}/s`)
    }
    for (const name of BIT_UNIT_NAMES) {
      expect(unitDisplayAbbrev(name)).not.toBe(name)
      expect(unitDisplayAbbrev(`${name}/s`)).not.toBe(`${name}/s`)
    }
  })
})

describe('unitSuffix', () => {
  it('attaches symbol-prefixed display units with no space', () => {
    expect(unitSuffix('/s')).toBe('/s')
    expect(unitSuffix('°')).toBe('°')
    expect(unitSuffix('°C')).toBe('°C')
    expect(unitSuffix('%')).toBe('%')
  })

  it('attaches an out-of-vocabulary symbol-prefixed unit with no space', () => {
    expect(unitSuffix('°F')).toBe('°F')
    expect(unitSuffix('%CPU')).toBe('%CPU')
  })

  it('adds a leading space for other display units', () => {
    expect(unitSuffix('ms')).toBe(' ms')
    expect(unitSuffix('KB')).toBe(' KB')
  })

  it('returns an empty string for an empty (dimensionless) display unit', () => {
    expect(unitSuffix('')).toBe('')
  })
})
