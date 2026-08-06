import { computeTabState } from '../useTabExecutionState'

describe('computeTabState', () => {
  it('is busy whenever isExecuting is true, regardless of hasError', () => {
    expect(computeTabState(true, false)).toBe('busy')
    expect(computeTabState(true, true)).toBe('busy')
  })

  it('is error when idle with an error', () => {
    expect(computeTabState(false, true)).toBe('error')
  })

  it('is idle when idle with no error', () => {
    expect(computeTabState(false, false)).toBe('idle')
  })
})
