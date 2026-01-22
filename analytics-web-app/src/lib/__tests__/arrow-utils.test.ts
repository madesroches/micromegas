/**
 * Tests for Arrow utilities including extractChartData
 */

// Mock Apache Arrow before importing
jest.mock('apache-arrow', () => {
  // Create mock type identifiers
  const TypeId = {
    Timestamp: 1,
    Date: 2,
    Time: 3,
    Int: 4,
    Float: 5,
    Decimal: 6,
    Utf8: 7,
    LargeUtf8: 8,
    Bool: 9,
    Binary: 10,
    LargeBinary: 11,
    FixedSizeBinary: 12,
    Dictionary: 13,
  }

  // Mock DataType class with static type checking methods
  class MockDataType {
    constructor(
      public typeId: number,
      public dictionary?: MockDataType
    ) {}

    static isTimestamp(dt: MockDataType): boolean {
      return dt.typeId === TypeId.Timestamp
    }
    static isDate(dt: MockDataType): boolean {
      return dt.typeId === TypeId.Date
    }
    static isTime(dt: MockDataType): boolean {
      return dt.typeId === TypeId.Time
    }
    static isInt(dt: MockDataType): boolean {
      return dt.typeId === TypeId.Int
    }
    static isFloat(dt: MockDataType): boolean {
      return dt.typeId === TypeId.Float
    }
    static isDecimal(dt: MockDataType): boolean {
      return dt.typeId === TypeId.Decimal
    }
    static isUtf8(dt: MockDataType): boolean {
      return dt.typeId === TypeId.Utf8
    }
    static isLargeUtf8(dt: MockDataType): boolean {
      return dt.typeId === TypeId.LargeUtf8
    }
    static isBinary(dt: MockDataType): boolean {
      return dt.typeId === TypeId.Binary
    }
    static isLargeBinary(dt: MockDataType): boolean {
      return dt.typeId === TypeId.LargeBinary
    }
    static isFixedSizeBinary(dt: MockDataType): boolean {
      return dt.typeId === TypeId.FixedSizeBinary
    }
    static isDictionary(dt: MockDataType): boolean {
      return dt.typeId === TypeId.Dictionary
    }
  }

  // Factory functions for creating typed DataTypes
  const createTimestampType = () => new MockDataType(TypeId.Timestamp)
  const createIntType = () => new MockDataType(TypeId.Int)
  const createFloatType = () => new MockDataType(TypeId.Float)
  const createUtf8Type = () => new MockDataType(TypeId.Utf8)
  const createBoolType = () => new MockDataType(TypeId.Bool)
  const createBinaryType = () => new MockDataType(TypeId.Binary)
  const createLargeBinaryType = () => new MockDataType(TypeId.LargeBinary)
  const createFixedSizeBinaryType = () => new MockDataType(TypeId.FixedSizeBinary)
  const createDictionaryType = (valueType: MockDataType) =>
    new MockDataType(TypeId.Dictionary, valueType)

  return {
    DataType: MockDataType,
    TimeUnit: { SECOND: 0, MILLISECOND: 1, MICROSECOND: 2, NANOSECOND: 3 },
    Timestamp: class {},
    Table: class {},
    // Export factory functions for tests
    __test__: {
      createTimestampType,
      createIntType,
      createFloatType,
      createUtf8Type,
      createBoolType,
      createBinaryType,
      createLargeBinaryType,
      createFixedSizeBinaryType,
      createDictionaryType,
    },
  }
})

import {
  extractChartData,
  validateChartColumns,
  detectXAxisMode,
  isTimeType,
  isNumericType,
  isStringType,
  unwrapDictionary,
  isBinaryType,
} from '../arrow-utils'

// Get test helpers from mock
const { __test__ } = jest.requireMock('apache-arrow')
const {
  createTimestampType,
  createIntType,
  createFloatType,
  createUtf8Type,
  createBoolType,
  createBinaryType,
  createLargeBinaryType,
  createFixedSizeBinaryType,
  createDictionaryType,
} = __test__

// Helper to create mock table
function createMockTable(
  fields: Array<{ name: string; type: unknown }>,
  rows: Record<string, unknown>[]
) {
  return {
    schema: { fields },
    numRows: rows.length,
    get: (i: number) => rows[i] ?? null,
  }
}

describe('dictionary type utilities', () => {
  describe('unwrapDictionary', () => {
    it('should return inner type for dictionary-encoded Utf8', () => {
      const dictType = createDictionaryType(createUtf8Type())
      const result = unwrapDictionary(dictType)
      expect(result.typeId).toBe(createUtf8Type().typeId)
    })

    it('should return inner type for dictionary-encoded Binary', () => {
      const dictType = createDictionaryType(createBinaryType())
      const result = unwrapDictionary(dictType)
      expect(result.typeId).toBe(createBinaryType().typeId)
    })

    it('should return inner type for dictionary-encoded Int', () => {
      const dictType = createDictionaryType(createIntType())
      const result = unwrapDictionary(dictType)
      expect(result.typeId).toBe(createIntType().typeId)
    })

    it('should pass through non-dictionary Utf8 type unchanged', () => {
      const utf8Type = createUtf8Type()
      const result = unwrapDictionary(utf8Type)
      expect(result).toBe(utf8Type)
    })

    it('should pass through non-dictionary Binary type unchanged', () => {
      const binaryType = createBinaryType()
      const result = unwrapDictionary(binaryType)
      expect(result).toBe(binaryType)
    })

    it('should pass through non-dictionary Int type unchanged', () => {
      const intType = createIntType()
      const result = unwrapDictionary(intType)
      expect(result).toBe(intType)
    })
  })

  describe('isBinaryType', () => {
    it('should return true for Binary type', () => {
      expect(isBinaryType(createBinaryType())).toBe(true)
    })

    it('should return true for LargeBinary type', () => {
      expect(isBinaryType(createLargeBinaryType())).toBe(true)
    })

    it('should return true for FixedSizeBinary type', () => {
      expect(isBinaryType(createFixedSizeBinaryType())).toBe(true)
    })

    it('should return true for dictionary-encoded Binary', () => {
      const dictBinary = createDictionaryType(createBinaryType())
      expect(isBinaryType(dictBinary)).toBe(true)
    })

    it('should return true for dictionary-encoded LargeBinary', () => {
      const dictLargeBinary = createDictionaryType(createLargeBinaryType())
      expect(isBinaryType(dictLargeBinary)).toBe(true)
    })

    it('should return true for dictionary-encoded FixedSizeBinary', () => {
      const dictFixedBinary = createDictionaryType(createFixedSizeBinaryType())
      expect(isBinaryType(dictFixedBinary)).toBe(true)
    })

    it('should return false for Utf8 type', () => {
      expect(isBinaryType(createUtf8Type())).toBe(false)
    })

    it('should return false for Int type', () => {
      expect(isBinaryType(createIntType())).toBe(false)
    })

    it('should return false for dictionary-encoded Utf8', () => {
      const dictUtf8 = createDictionaryType(createUtf8Type())
      expect(isBinaryType(dictUtf8)).toBe(false)
    })
  })
})

describe('type detection functions', () => {
  describe('isTimeType', () => {
    it('should return true for timestamp type', () => {
      expect(isTimeType(createTimestampType())).toBe(true)
    })

    it('should return false for int type', () => {
      expect(isTimeType(createIntType())).toBe(false)
    })

    it('should return false for string type', () => {
      expect(isTimeType(createUtf8Type())).toBe(false)
    })
  })

  describe('isNumericType', () => {
    it('should return true for int type', () => {
      expect(isNumericType(createIntType())).toBe(true)
    })

    it('should return true for float type', () => {
      expect(isNumericType(createFloatType())).toBe(true)
    })

    it('should return false for timestamp type', () => {
      expect(isNumericType(createTimestampType())).toBe(false)
    })

    it('should return false for string type', () => {
      expect(isNumericType(createUtf8Type())).toBe(false)
    })
  })

  describe('isStringType', () => {
    it('should return true for utf8 type', () => {
      expect(isStringType(createUtf8Type())).toBe(true)
    })

    it('should return false for int type', () => {
      expect(isStringType(createIntType())).toBe(false)
    })

    it('should return false for timestamp type', () => {
      expect(isStringType(createTimestampType())).toBe(false)
    })
  })

  describe('detectXAxisMode', () => {
    it('should return time for timestamp type', () => {
      expect(detectXAxisMode(createTimestampType())).toBe('time')
    })

    it('should return numeric for int type', () => {
      expect(detectXAxisMode(createIntType())).toBe('numeric')
    })

    it('should return numeric for float type', () => {
      expect(detectXAxisMode(createFloatType())).toBe('numeric')
    })

    it('should return categorical for string type', () => {
      expect(detectXAxisMode(createUtf8Type())).toBe('categorical')
    })

    it('should return categorical for unsupported types', () => {
      expect(detectXAxisMode(createBoolType())).toBe('categorical')
    })
  })
})

describe('validateChartColumns', () => {
  it('should reject tables with less than 2 columns', () => {
    const table = createMockTable([{ name: 'x', type: createIntType() }], [])
    const result = validateChartColumns(table as never)
    expect(result.valid).toBe(false)
    if (!result.valid) {
      expect(result.error).toContain('exactly 2 columns')
    }
  })

  it('should reject tables with more than 2 columns', () => {
    const table = createMockTable(
      [
        { name: 'x', type: createIntType() },
        { name: 'y', type: createFloatType() },
        { name: 'z', type: createFloatType() },
      ],
      []
    )
    const result = validateChartColumns(table as never)
    expect(result.valid).toBe(false)
    if (!result.valid) {
      expect(result.error).toContain('exactly 2 columns')
    }
  })

  it('should reject tables with non-numeric Y column', () => {
    const table = createMockTable(
      [
        { name: 'x', type: createIntType() },
        { name: 'y', type: createUtf8Type() },
      ],
      []
    )
    const result = validateChartColumns(table as never)
    expect(result.valid).toBe(false)
    if (!result.valid) {
      expect(result.error).toContain('Second column must be numeric')
    }
  })

  it('should reject tables with invalid X column type', () => {
    const table = createMockTable(
      [
        { name: 'x', type: createBoolType() },
        { name: 'y', type: createFloatType() },
      ],
      []
    )
    const result = validateChartColumns(table as never)
    expect(result.valid).toBe(false)
    if (!result.valid) {
      expect(result.error).toContain('First column must be timestamp, numeric, or string')
    }
  })

  it('should accept valid numeric X, numeric Y table', () => {
    const table = createMockTable(
      [
        { name: 'x', type: createIntType() },
        { name: 'y', type: createFloatType() },
      ],
      []
    )
    const result = validateChartColumns(table as never)
    expect(result.valid).toBe(true)
  })

  it('should accept valid timestamp X, numeric Y table', () => {
    const table = createMockTable(
      [
        { name: 'time', type: createTimestampType() },
        { name: 'value', type: createFloatType() },
      ],
      []
    )
    const result = validateChartColumns(table as never)
    expect(result.valid).toBe(true)
  })

  it('should accept valid string X, numeric Y table', () => {
    const table = createMockTable(
      [
        { name: 'category', type: createUtf8Type() },
        { name: 'count', type: createIntType() },
      ],
      []
    )
    const result = validateChartColumns(table as never)
    expect(result.valid).toBe(true)
  })
})

describe('extractChartData', () => {
  describe('numeric mode', () => {
    it('should extract numeric x/y data', () => {
      const table = createMockTable(
        [
          { name: 'x', type: createIntType() },
          { name: 'y', type: createFloatType() },
        ],
        [
          { x: 1, y: 10 },
          { x: 2, y: 20 },
          { x: 3, y: 30 },
        ]
      )
      const result = extractChartData(table as never)
      expect(result.ok).toBe(true)
      if (result.ok) {
        expect(result.xAxisMode).toBe('numeric')
        expect(result.xColumnName).toBe('x')
        expect(result.yColumnName).toBe('y')
        expect(result.data).toEqual([
          { x: 1, y: 10 },
          { x: 2, y: 20 },
          { x: 3, y: 30 },
        ])
      }
    })

    it('should sort numeric data by X ascending', () => {
      const table = createMockTable(
        [
          { name: 'x', type: createIntType() },
          { name: 'y', type: createFloatType() },
        ],
        [
          { x: 3, y: 30 },
          { x: 1, y: 10 },
          { x: 2, y: 20 },
        ]
      )
      const result = extractChartData(table as never)
      expect(result.ok).toBe(true)
      if (result.ok) {
        expect(result.data).toEqual([
          { x: 1, y: 10 },
          { x: 2, y: 20 },
          { x: 3, y: 30 },
        ])
      }
    })

    it('should skip rows with null X values', () => {
      const table = createMockTable(
        [
          { name: 'x', type: createIntType() },
          { name: 'y', type: createFloatType() },
        ],
        [
          { x: 1, y: 10 },
          { x: null, y: 20 },
          { x: 3, y: 30 },
        ]
      )
      const result = extractChartData(table as never)
      expect(result.ok).toBe(true)
      if (result.ok) {
        expect(result.data).toHaveLength(2)
      }
    })

    it('should skip rows with null Y values', () => {
      const table = createMockTable(
        [
          { name: 'x', type: createIntType() },
          { name: 'y', type: createFloatType() },
        ],
        [
          { x: 1, y: 10 },
          { x: 2, y: null },
          { x: 3, y: 30 },
        ]
      )
      const result = extractChartData(table as never)
      expect(result.ok).toBe(true)
      if (result.ok) {
        expect(result.data).toHaveLength(2)
      }
    })

    it('should return error when all values are null', () => {
      const table = createMockTable(
        [
          { name: 'x', type: createIntType() },
          { name: 'y', type: createFloatType() },
        ],
        [
          { x: null, y: null },
          { x: null, y: null },
        ]
      )
      const result = extractChartData(table as never)
      expect(result.ok).toBe(false)
      if (!result.ok) {
        expect(result.error).toContain('No valid data points')
      }
    })
  })

  describe('time mode', () => {
    it('should extract timestamp x data as milliseconds', () => {
      const table = createMockTable(
        [
          { name: 'time', type: createTimestampType() },
          { name: 'value', type: createFloatType() },
        ],
        [
          { time: 1000, value: 10 },
          { time: 2000, value: 20 },
          { time: 3000, value: 30 },
        ]
      )
      const result = extractChartData(table as never)
      expect(result.ok).toBe(true)
      if (result.ok) {
        expect(result.xAxisMode).toBe('time')
        expect(result.xColumnName).toBe('time')
        expect(result.yColumnName).toBe('value')
        // timestampToMs returns number values directly
        expect(result.data).toEqual([
          { x: 1000, y: 10 },
          { x: 2000, y: 20 },
          { x: 3000, y: 30 },
        ])
      }
    })

    it('should sort time data by X ascending', () => {
      const table = createMockTable(
        [
          { name: 'time', type: createTimestampType() },
          { name: 'value', type: createFloatType() },
        ],
        [
          { time: 3000, value: 30 },
          { time: 1000, value: 10 },
          { time: 2000, value: 20 },
        ]
      )
      const result = extractChartData(table as never)
      expect(result.ok).toBe(true)
      if (result.ok) {
        expect(result.data[0].x).toBe(1000)
        expect(result.data[1].x).toBe(2000)
        expect(result.data[2].x).toBe(3000)
      }
    })
  })

  describe('categorical mode', () => {
    it('should extract string x data as indices with labels', () => {
      const table = createMockTable(
        [
          { name: 'category', type: createUtf8Type() },
          { name: 'count', type: createIntType() },
        ],
        [
          { category: 'A', count: 10 },
          { category: 'B', count: 20 },
          { category: 'C', count: 30 },
        ]
      )
      const result = extractChartData(table as never)
      expect(result.ok).toBe(true)
      if (result.ok) {
        expect(result.xAxisMode).toBe('categorical')
        expect(result.xLabels).toEqual(['A', 'B', 'C'])
        expect(result.data).toEqual([
          { x: 0, y: 10 },
          { x: 1, y: 20 },
          { x: 2, y: 30 },
        ])
      }
    })

    it('should preserve SQL order for categorical data', () => {
      const table = createMockTable(
        [
          { name: 'category', type: createUtf8Type() },
          { name: 'count', type: createIntType() },
        ],
        [
          { category: 'C', count: 30 },
          { category: 'A', count: 10 },
          { category: 'B', count: 20 },
        ]
      )
      const result = extractChartData(table as never)
      expect(result.ok).toBe(true)
      if (result.ok) {
        // Labels should be in SQL order, not sorted
        expect(result.xLabels).toEqual(['C', 'A', 'B'])
        expect(result.data).toEqual([
          { x: 0, y: 30 },
          { x: 1, y: 10 },
          { x: 2, y: 20 },
        ])
      }
    })

    it('should handle repeated categorical labels', () => {
      const table = createMockTable(
        [
          { name: 'category', type: createUtf8Type() },
          { name: 'count', type: createIntType() },
        ],
        [
          { category: 'A', count: 10 },
          { category: 'A', count: 15 },
          { category: 'B', count: 20 },
        ]
      )
      const result = extractChartData(table as never)
      expect(result.ok).toBe(true)
      if (result.ok) {
        expect(result.xLabels).toEqual(['A', 'B'])
        expect(result.data).toEqual([
          { x: 0, y: 10 },
          { x: 0, y: 15 },
          { x: 1, y: 20 },
        ])
      }
    })
  })

  describe('validation errors', () => {
    it('should return error for invalid column count', () => {
      const table = createMockTable([{ name: 'x', type: createIntType() }], [])
      const result = extractChartData(table as never)
      expect(result.ok).toBe(false)
      if (!result.ok) {
        expect(result.error).toContain('exactly 2 columns')
      }
    })
  })
})
