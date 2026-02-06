import type { VariableValue } from './notebook-types'
import { getVariableString } from './notebook-types'

/**
 * Snap levels: human-friendly SQL interval strings ordered by duration.
 */
const SNAP_LEVELS = [
  { ms: 100, label: '100ms' },
  { ms: 500, label: '500ms' },
  { ms: 1_000, label: '1s' },
  { ms: 5_000, label: '5s' },
  { ms: 15_000, label: '15s' },
  { ms: 30_000, label: '30s' },
  { ms: 60_000, label: '1m' },
  { ms: 300_000, label: '5m' },
  { ms: 900_000, label: '15m' },
  { ms: 1_800_000, label: '30m' },
  { ms: 3_600_000, label: '1h' },
  { ms: 21_600_000, label: '6h' },
  { ms: 86_400_000, label: '1d' },
  { ms: 604_800_000, label: '7d' },
  { ms: 2_592_000_000, label: '30d' },
]

/**
 * Snaps a millisecond duration to the nearest human-friendly SQL interval string.
 * Picks the largest snap level that is <= the input duration.
 * Falls back to the smallest level if the input is below all thresholds.
 */
export function snapInterval(ms: number): string {
  let best = SNAP_LEVELS[0].label
  for (const level of SNAP_LEVELS) {
    if (ms >= level.ms) {
      best = level.label
    } else {
      break
    }
  }
  return best
}

/**
 * Evaluates a JavaScript expression with variable bindings.
 *
 * Available bindings:
 * - `$begin`, `$end`: ISO 8601 timestamp strings
 * - `snap_interval(ms)`: snaps a ms duration to a human-friendly SQL interval
 * - `$<name>` for each upstream variable
 *
 * Standard JS globals (Date, Math, window, etc.) are accessible.
 * Uses `new Function()` — same trust boundary as the SQL queries the notebook author writes.
 */
export function evaluateVariableExpression(
  expression: string,
  context: {
    begin: string
    end: string
    variables: Record<string, VariableValue>
  }
): string {
  const { begin, end, variables } = context

  const paramNames: string[] = ['$begin', '$end', 'snap_interval']
  const paramValues: unknown[] = [begin, end, snapInterval]

  for (const [name, value] of Object.entries(variables)) {
    paramNames.push(`$${name}`)
    // Pass multi-column variables as their string representation
    paramValues.push(typeof value === 'string' ? value : getVariableString(value))
  }

  // eslint-disable-next-line @typescript-eslint/no-implied-eval
  const fn = new Function(...paramNames, `"use strict"; return (${expression})`)
  const result = fn(...paramValues)
  return String(result)
}
