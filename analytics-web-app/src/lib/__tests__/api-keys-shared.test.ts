/**
 * `parseAllowlistInput` (#1611): the lenient free-text splitter shared by the mint dialogs and
 * `EditAllowlistDialog`. No CIDR validation here -- the server's `IpAllowlist::parse` is the one
 * validator (see the design plan's Trade-offs section).
 */
import { parseAllowlistInput } from '../api-keys-shared'

describe('parseAllowlistInput', () => {
  it('splits on newlines', () => {
    expect(parseAllowlistInput('10.0.0.0/8\n203.0.113.7')).toEqual(['10.0.0.0/8', '203.0.113.7'])
  })

  it('splits on commas', () => {
    expect(parseAllowlistInput('10.0.0.0/8,203.0.113.7')).toEqual(['10.0.0.0/8', '203.0.113.7'])
  })

  it('splits on mixed whitespace, newlines, and commas', () => {
    expect(parseAllowlistInput('10.0.0.0/8,  203.0.113.7\n  198.51.100.1')).toEqual([
      '10.0.0.0/8',
      '203.0.113.7',
      '198.51.100.1',
    ])
  })

  it('drops blank lines and trailing separators', () => {
    expect(parseAllowlistInput('10.0.0.0/8\n\n,203.0.113.7,\n')).toEqual([
      '10.0.0.0/8',
      '203.0.113.7',
    ])
  })

  it('returns [] for empty input', () => {
    expect(parseAllowlistInput('')).toEqual([])
  })

  it('returns [] for whitespace-only input', () => {
    expect(parseAllowlistInput('   \n\t  ,  ')).toEqual([])
  })

  it('passes entries through unchanged, with no normalization', () => {
    expect(parseAllowlistInput('  10.0.0.5/8  ')).toEqual(['10.0.0.5/8'])
  })
})
