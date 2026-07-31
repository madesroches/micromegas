import { formatValueWithUnit } from '../format-value'

describe('formatValueWithUnit', () => {
  describe('time units (non-abbreviated, matching chart stat formatter)', () => {
    it('formats nanoseconds adaptively to nanoseconds', () => {
      expect(formatValueWithUnit(500, 'nanoseconds')).toBe('500 nanoseconds')
    })

    it('formats nanoseconds adaptively to microseconds', () => {
      expect(formatValueWithUnit(1500, 'nanoseconds')).toBe('1.50 microseconds')
    })

    it('formats nanoseconds adaptively to milliseconds', () => {
      expect(formatValueWithUnit(2_500_000, 'nanoseconds')).toBe('2.50 milliseconds')
    })

    it('formats milliseconds adaptively to seconds', () => {
      expect(formatValueWithUnit(4070, 'milliseconds')).toBe('4.07 seconds')
    })

    it('formats seconds adaptively to milliseconds', () => {
      expect(formatValueWithUnit(0.00407, 'seconds')).toBe('4.07 milliseconds')
    })

    it('formats large seconds adaptively to minutes', () => {
      expect(formatValueWithUnit(120, 'seconds')).toBe('2.00 minutes')
    })

    it('formats minutes adaptively to hours', () => {
      expect(formatValueWithUnit(150, 'minutes')).toBe('2.50 hours')
    })

    it('formats hours adaptively to days', () => {
      expect(formatValueWithUnit(48, 'hours')).toBe('2.00 days')
    })

    it('accepts unit aliases (ns -> nanoseconds)', () => {
      expect(formatValueWithUnit(1500, 'ns')).toBe('1.50 microseconds')
    })
  })

  describe('size units', () => {
    it('formats bytes with no decimals at the base level', () => {
      expect(formatValueWithUnit(500, 'bytes')).toBe('500 B')
    })

    it('formats bytes adaptively to KB', () => {
      expect(formatValueWithUnit(2048, 'bytes')).toBe('2.0 KB')
    })

    it('formats bytes adaptively to MB', () => {
      expect(formatValueWithUnit(5 * 1024 * 1024, 'bytes')).toBe('5.0 MB')
    })

    it('formats bytes adaptively to GB', () => {
      // 3.4 * 1024^3 = 3650722201.6 ~ "3.4 GB"
      expect(formatValueWithUnit(3678630912, 'bytes')).toBe('3.4 GB')
    })

    it('formats bytes adaptively to TB', () => {
      const tb = 1024 * 1024 * 1024 * 1024
      expect(formatValueWithUnit(2.5 * tb, 'bytes')).toBe('2.5 TB')
    })

    it('accepts size unit aliases (KB)', () => {
      expect(formatValueWithUnit(2048, 'KB')).toBe('2.0 MB')
    })
  })

  describe('bit units', () => {
    it('formats bits adaptively to kbit', () => {
      expect(formatValueWithUnit(2000, 'bits')).toBe('2.0 kbit')
    })

    it('formats bits adaptively to Mbit', () => {
      expect(formatValueWithUnit(5_000_000, 'bits')).toBe('5.0 Mbit')
    })

    it('formats bits per second adaptively', () => {
      expect(formatValueWithUnit(3_000_000_000, 'bits/s')).toBe('3.0 Gbit/s')
    })

    it('scales the legacy plural rate spelling (Mbits/s) adaptively, not as a raw passthrough', () => {
      expect(formatValueWithUnit(3000, 'Mbits/s')).toBe('3.0 Gbit/s')
    })
  })

  describe('percent', () => {
    it('formats with one decimal and percent sign', () => {
      expect(formatValueWithUnit(42.567, 'percent')).toBe('42.6%')
    })

    it('accepts the % alias', () => {
      expect(formatValueWithUnit(50, '%')).toBe('50.0%')
    })
  })

  describe('degrees', () => {
    it('formats with one decimal and degree sign', () => {
      expect(formatValueWithUnit(180.5, 'degrees')).toBe('180.5°')
    })
  })

  describe('celsius', () => {
    it('formats with one decimal and °C suffix', () => {
      expect(formatValueWithUnit(21.5, 'Cel')).toBe('21.5°C')
    })

    it('accepts the spelled-out celsius alias', () => {
      expect(formatValueWithUnit(21.5, 'celsius')).toBe('21.5°C')
    })
  })

  describe('dimensionless (UCUM unity, count, none)', () => {
    it('formats none as a bare number', () => {
      expect(formatValueWithUnit(5, 'none')).toBe('5')
    })

    it('formats count as a grouped bare number', () => {
      expect(formatValueWithUnit(1234567, 'count')).toBe((1234567).toLocaleString())
    })

    it('formats a dimensionless rate with no space before the slash', () => {
      expect(formatValueWithUnit(1234, '{Count}/s')).toBe('1,234/s')
    })

    it("formats the issue's headline rate symptom (By/s -> GB/s)", () => {
      expect(formatValueWithUnit(1234567890, 'By/s')).toBe('1.1 GB/s')
    })
  })

  describe('centimeters', () => {
    it('displays the cm abbreviation rather than the spelled-out canonical name', () => {
      expect(formatValueWithUnit(42, 'cm')).toBe('42 cm')
    })
  })

  describe('boolean', () => {
    it('formats nonzero as true', () => {
      expect(formatValueWithUnit(1, 'boolean')).toBe('true')
    })

    it('formats zero as false', () => {
      expect(formatValueWithUnit(0, 'boolean')).toBe('false')
    })
  })

  describe('currency', () => {
    it('formats USD as currency', () => {
      const expected = new Intl.NumberFormat(undefined, { style: 'currency', currency: 'USD' }).format(1234.5)
      expect(formatValueWithUnit(1234.5, 'USD')).toBe(expected)
    })

    it('formats lowercase currency codes', () => {
      const expected = new Intl.NumberFormat(undefined, { style: 'currency', currency: 'CAD' }).format(1234.5)
      expect(formatValueWithUnit(1234.5, 'cad')).toBe(expected)
    })

    it('does not treat plausible non-currency 3-letter units as currency', () => {
      expect(formatValueWithUnit(42, 'MPH')).toBe(`${(42).toLocaleString()} MPH`)
    })
  })

  describe('unitless / unknown unit', () => {
    it('returns locale-formatted number when no unit is provided', () => {
      expect(formatValueWithUnit(1234567, '')).toBe((1234567).toLocaleString())
    })

    it('appends an unknown unit verbatim', () => {
      expect(formatValueWithUnit(42, 'widgets')).toBe(`${(42).toLocaleString()} widgets`)
    })

    it('attaches an unmapped, symbol-prefixed unit with no space, matching formatYAxisTick', () => {
      expect(formatValueWithUnit(72.5, '°F')).toBe(`${(72.5).toLocaleString()}°F`)
      expect(formatValueWithUnit(50, '%CPU')).toBe(`${(50).toLocaleString()}%CPU`)
    })
  })
})
