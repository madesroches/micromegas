import { snapInterval, evaluateVariableExpression } from '../notebook-expression-eval'

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
  const baseContext = {
    begin: '2024-01-01T00:00:00Z',
    end: '2024-01-02T00:00:00Z',
    variables: {},
  }

  it('should evaluate simple arithmetic', () => {
    expect(evaluateVariableExpression('1 + 2', baseContext)).toBe('3')
  })

  it('should provide $begin and $end as strings', () => {
    expect(evaluateVariableExpression('$begin', baseContext)).toBe('2024-01-01T00:00:00Z')
    expect(evaluateVariableExpression('$end', baseContext)).toBe('2024-01-02T00:00:00Z')
  })

  it('should allow Date arithmetic with $begin and $end', () => {
    const result = evaluateVariableExpression(
      'new Date($end) - new Date($begin)',
      baseContext
    )
    // 24 hours in ms
    expect(result).toBe('86400000')
  })

  it('should provide snap_interval function', () => {
    const result = evaluateVariableExpression(
      'snap_interval(86400000)',
      baseContext
    )
    expect(result).toBe('1d')
  })

  it('should evaluate the full time_bin_duration expression', () => {
    // Mock window.innerWidth
    Object.defineProperty(window, 'innerWidth', { value: 1920, writable: true })

    const result = evaluateVariableExpression(
      'snap_interval((new Date($end) - new Date($begin)) / window.innerWidth)',
      baseContext
    )
    // 86400000ms / 1920px = 45000ms per pixel -> snaps to 30s
    expect(result).toBe('30s')
  })

  it('should provide access to Math functions', () => {
    expect(evaluateVariableExpression('Math.max(1, 2, 3)', baseContext)).toBe('3')
    expect(evaluateVariableExpression('Math.round(3.7)', baseContext)).toBe('4')
  })

  it('should pass upstream variables as bindings', () => {
    const result = evaluateVariableExpression('$myVar', {
      ...baseContext,
      variables: { myVar: 'hello' },
    })
    expect(result).toBe('hello')
  })

  it('should convert result to string', () => {
    expect(evaluateVariableExpression('true', baseContext)).toBe('true')
    expect(evaluateVariableExpression('null', baseContext)).toBe('null')
  })

  it('should throw on malformed expressions', () => {
    expect(() => evaluateVariableExpression('if (true) {', baseContext)).toThrow()
  })

  it('should throw on undefined variable references', () => {
    expect(() => evaluateVariableExpression('nonexistent', baseContext)).toThrow()
  })
})
