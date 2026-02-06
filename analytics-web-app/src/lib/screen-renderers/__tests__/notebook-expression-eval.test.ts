import { snapInterval, evaluateVariableExpression, ExpressionContext } from '../notebook-expression-eval'

describe('snapInterval', () => {
  it('should snap to 100ms for very small durations', () => {
    expect(snapInterval(50)).toBe('100ms')
    expect(snapInterval(100)).toBe('100ms')
  })

  it('should snap to 500ms', () => {
    expect(snapInterval(500)).toBe('500ms')
    expect(snapInterval(750)).toBe('500ms')
  })

  it('should snap to 1s', () => {
    expect(snapInterval(1000)).toBe('1s')
    expect(snapInterval(1200)).toBe('1s')
  })

  it('should snap to 5s', () => {
    expect(snapInterval(5000)).toBe('5s')
    expect(snapInterval(8000)).toBe('5s')
  })

  it('should snap to 15s', () => {
    expect(snapInterval(15000)).toBe('15s')
  })

  it('should snap to 30s', () => {
    expect(snapInterval(30000)).toBe('30s')
  })

  it('should snap to 1m', () => {
    expect(snapInterval(60000)).toBe('1m')
  })

  it('should snap to 5m', () => {
    expect(snapInterval(300000)).toBe('5m')
    expect(snapInterval(180000)).toBe('1m')
  })

  it('should snap to 15m', () => {
    expect(snapInterval(900000)).toBe('15m')
  })

  it('should snap to 30m', () => {
    expect(snapInterval(1800000)).toBe('30m')
  })

  it('should snap to 1h', () => {
    expect(snapInterval(3600000)).toBe('1h')
  })

  it('should snap to 6h', () => {
    expect(snapInterval(21600000)).toBe('6h')
  })

  it('should snap to 1d', () => {
    expect(snapInterval(86400000)).toBe('1d')
  })

  it('should snap to 7d', () => {
    expect(snapInterval(604800000)).toBe('7d')
  })

  it('should snap to 30d for very large durations', () => {
    expect(snapInterval(2592000000)).toBe('30d')
    expect(snapInterval(5000000000)).toBe('30d')
  })
})

describe('evaluateVariableExpression', () => {
  const baseContext: ExpressionContext = {
    begin: '2024-01-01T00:00:00Z',
    end: '2024-01-02T00:00:00Z',
    durationMs: 86400000,
    innerWidth: 1920,
    devicePixelRatio: 2,
    variables: {},
  }

  describe('basic expressions', () => {
    it('should evaluate simple arithmetic', () => {
      expect(evaluateVariableExpression('1 + 2', baseContext)).toBe('3')
    })

    it('should evaluate multiplication and division', () => {
      expect(evaluateVariableExpression('10 * 3', baseContext)).toBe('30')
      expect(evaluateVariableExpression('10 / 4', baseContext)).toBe('2.5')
    })

    it('should evaluate modulo', () => {
      expect(evaluateVariableExpression('10 % 3', baseContext)).toBe('1')
    })

    it('should evaluate unary minus', () => {
      expect(evaluateVariableExpression('-5', baseContext)).toBe('-5')
    })

    it('should evaluate nested arithmetic', () => {
      expect(evaluateVariableExpression('(1 + 2) * 3', baseContext)).toBe('9')
    })
  })

  describe('bindings', () => {
    it('should provide $begin and $end as strings', () => {
      expect(evaluateVariableExpression('$begin', baseContext)).toBe('2024-01-01T00:00:00Z')
      expect(evaluateVariableExpression('$end', baseContext)).toBe('2024-01-02T00:00:00Z')
    })

    it('should provide $duration_ms', () => {
      expect(evaluateVariableExpression('$duration_ms', baseContext)).toBe('86400000')
    })

    it('should provide $innerWidth', () => {
      expect(evaluateVariableExpression('$innerWidth', baseContext)).toBe('1920')
    })

    it('should provide $devicePixelRatio', () => {
      expect(evaluateVariableExpression('$devicePixelRatio', baseContext)).toBe('2')
    })

    it('should pass upstream variables as bindings', () => {
      const result = evaluateVariableExpression('$myVar', {
        ...baseContext,
        variables: { myVar: 'hello' },
      })
      expect(result).toBe('hello')
    })
  })

  describe('allowed functions', () => {
    it('should allow Date arithmetic with $begin and $end', () => {
      const result = evaluateVariableExpression(
        'new Date($end) - new Date($begin)',
        baseContext
      )
      expect(result).toBe('86400000')
    })

    it('should provide snap_interval function', () => {
      expect(evaluateVariableExpression('snap_interval(86400000)', baseContext)).toBe('1d')
    })

    it('should provide access to Math methods', () => {
      expect(evaluateVariableExpression('Math.max(1, 2, 3)', baseContext)).toBe('3')
      expect(evaluateVariableExpression('Math.round(3.7)', baseContext)).toBe('4')
      expect(evaluateVariableExpression('Math.floor(3.9)', baseContext)).toBe('3')
      expect(evaluateVariableExpression('Math.abs(-5)', baseContext)).toBe('5')
    })

    it('should provide access to Math constants', () => {
      expect(evaluateVariableExpression('Math.PI', baseContext)).toBe(String(Math.PI))
    })
  })

  describe('end-to-end expressions', () => {
    it('should evaluate the canonical time_bin_duration expression', () => {
      const result = evaluateVariableExpression(
        'snap_interval($duration_ms / $innerWidth)',
        baseContext
      )
      // 86400000ms / 1920px = 45000ms per pixel -> snaps to 30s
      expect(result).toBe('30s')
    })

    it('should work with devicePixelRatio', () => {
      const result = evaluateVariableExpression(
        'snap_interval($duration_ms / ($innerWidth * $devicePixelRatio))',
        baseContext
      )
      // 86400000ms / (1920 * 2) = 22500ms per physical pixel -> snaps to 15s
      expect(result).toBe('15s')
    })
  })

  describe('error handling', () => {
    it('should throw on malformed expressions', () => {
      expect(() => evaluateVariableExpression('1 +', baseContext)).toThrow()
    })

    it('should throw on unknown identifiers', () => {
      expect(() => evaluateVariableExpression('nonexistent', baseContext)).toThrow(/Unknown identifier/)
    })
  })

  describe('security: blocked operations', () => {
    it('should reject access to document', () => {
      expect(() => evaluateVariableExpression('document', baseContext)).toThrow(/Unknown identifier/)
    })

    it('should reject access to window', () => {
      expect(() => evaluateVariableExpression('window', baseContext)).toThrow(/Unknown identifier/)
    })

    it('should reject access to globalThis', () => {
      expect(() => evaluateVariableExpression('globalThis', baseContext)).toThrow(/Unknown identifier/)
    })

    it('should reject unknown function calls', () => {
      expect(() => evaluateVariableExpression('fetch("/steal")', baseContext)).toThrow(/not allowed/)
    })

    it('should reject alert', () => {
      expect(() => evaluateVariableExpression('alert("xss")', baseContext)).toThrow(/not allowed/)
    })

    it('should reject eval', () => {
      expect(() => evaluateVariableExpression('eval("1")', baseContext)).toThrow(/not allowed/)
    })

    it('should reject property access on non-Math objects', () => {
      expect(() => evaluateVariableExpression('$begin.length', baseContext)).toThrow(/not allowed/)
    })

    it('should reject computed member access (bracket notation)', () => {
      expect(() => evaluateVariableExpression('Math["constructor"]', baseContext)).toThrow(/not allowed/)
    })

    it('should reject Math.constructor', () => {
      expect(() => evaluateVariableExpression('Math.constructor', baseContext)).toThrow(/not allowed/)
    })

    it('should reject Math.__proto__', () => {
      expect(() => evaluateVariableExpression('Math.__proto__', baseContext)).toThrow(/not allowed/)
    })

    it('should reject Math.prototype', () => {
      expect(() => evaluateVariableExpression('Math.prototype', baseContext)).toThrow(/not allowed/)
    })

    it('should reject new Function()', () => {
      expect(() => evaluateVariableExpression('new Function("return 1")', baseContext)).toThrow(/Only new Date/)
    })

    it('should reject conditional expressions', () => {
      expect(() => evaluateVariableExpression('1 ? 2 : 3', baseContext)).toThrow(/not allowed/)
    })

    it('should reject array expressions', () => {
      expect(() => evaluateVariableExpression('[1, 2, 3]', baseContext)).toThrow(/not allowed/)
    })

    it('should reject this', () => {
      expect(() => evaluateVariableExpression('this', baseContext)).toThrow(/not allowed/)
    })

    it('should reject bitwise operators', () => {
      expect(() => evaluateVariableExpression('1 | 2', baseContext)).toThrow(/not allowed/)
      expect(() => evaluateVariableExpression('1 & 2', baseContext)).toThrow(/not allowed/)
    })

    it('should reject logical operators', () => {
      expect(() => evaluateVariableExpression('1 || 2', baseContext)).toThrow(/not allowed/)
      expect(() => evaluateVariableExpression('1 && 2', baseContext)).toThrow(/not allowed/)
    })

    it('should reject comparison operators', () => {
      expect(() => evaluateVariableExpression('1 > 2', baseContext)).toThrow(/not allowed/)
      expect(() => evaluateVariableExpression('1 === 2', baseContext)).toThrow(/not allowed/)
    })

    it('should reject unary not', () => {
      expect(() => evaluateVariableExpression('!1', baseContext)).toThrow(/not allowed/)
    })

    it('should reject typeof', () => {
      expect(() => evaluateVariableExpression('typeof $begin', baseContext)).toThrow(/not allowed/)
    })
  })
})
