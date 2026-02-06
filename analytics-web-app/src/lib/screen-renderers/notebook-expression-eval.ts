import jsep from 'jsep'
import jsepNew from '@jsep-plugin/new'
import type { VariableValue } from './notebook-types'
import { getVariableString } from './notebook-types'

// Register the `new` plugin so jsep can parse `new Date(...)` expressions
jsep.plugins.register(jsepNew)

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

// Property names that must never be accessed (prototype chain escape vectors)
const BLOCKED_PROPERTIES = new Set(['constructor', '__proto__', 'prototype'])

// Allowed Math properties (methods and constants)
const ALLOWED_MATH_PROPERTIES = new Set([
  // Methods
  'abs', 'ceil', 'floor', 'round', 'trunc',
  'max', 'min', 'pow', 'sqrt', 'cbrt',
  'log', 'log2', 'log10', 'exp',
  'sign', 'clz32', 'fround',
  'random',
  'sin', 'cos', 'tan', 'asin', 'acos', 'atan', 'atan2',
  'sinh', 'cosh', 'tanh', 'asinh', 'acosh', 'atanh',
  'hypot', 'imul',
  // Constants
  'PI', 'E', 'LN2', 'LN10', 'LOG2E', 'LOG10E', 'SQRT2', 'SQRT1_2',
])

const ALLOWED_BINARY_OPS = new Set(['+', '-', '*', '/', '%'])
const ALLOWED_UNARY_OPS = new Set(['-', '+'])

/**
 * Recursive AST evaluator with allowlist-based security.
 * Default-deny: throws on any unrecognized node type.
 */
function evaluateNode(
  node: jsep.Expression,
  bindings: Record<string, unknown>
): unknown {
  switch (node.type) {
    case 'Literal': {
      const lit = node as jsep.Literal
      if (lit.value instanceof RegExp) {
        throw new Error('Regex literals are not allowed in expressions')
      }
      return lit.value
    }

    case 'Identifier': {
      const id = node as jsep.Identifier
      if (!(id.name in bindings)) {
        throw new Error(`Unknown identifier: ${id.name}`)
      }
      return bindings[id.name]
    }

    case 'BinaryExpression': {
      const bin = node as jsep.BinaryExpression
      if (!ALLOWED_BINARY_OPS.has(bin.operator)) {
        throw new Error(`Operator not allowed: ${bin.operator}`)
      }
      const left = evaluateNode(bin.left, bindings) as number
      const right = evaluateNode(bin.right, bindings) as number
      switch (bin.operator) {
        case '+': return left + right
        case '-': return left - right
        case '*': return left * right
        case '/': return left / right
        case '%': return left % right
        default: throw new Error(`Operator not allowed: ${bin.operator}`)
      }
    }

    case 'UnaryExpression': {
      const un = node as jsep.UnaryExpression
      if (!ALLOWED_UNARY_OPS.has(un.operator)) {
        throw new Error(`Unary operator not allowed: ${un.operator}`)
      }
      const arg = evaluateNode(un.argument, bindings) as number
      return un.operator === '-' ? -arg : +arg
    }

    case 'CallExpression': {
      const call = node as jsep.CallExpression
      const args = call.arguments.map(a => evaluateNode(a, bindings))

      // snap_interval(ms)
      if (call.callee.type === 'Identifier') {
        const name = (call.callee as jsep.Identifier).name
        if (name === 'snap_interval') {
          return snapInterval(args[0] as number)
        }
        throw new Error(`Function not allowed: ${name}`)
      }

      // Math.method(...)
      if (call.callee.type === 'MemberExpression') {
        const member = call.callee as jsep.MemberExpression
        if (
          member.object.type === 'Identifier' &&
          (member.object as jsep.Identifier).name === 'Math' &&
          !member.computed &&
          member.property.type === 'Identifier'
        ) {
          const propName = (member.property as jsep.Identifier).name
          if (BLOCKED_PROPERTIES.has(propName)) {
            throw new Error(`Property access not allowed: Math.${propName}`)
          }
          if (!ALLOWED_MATH_PROPERTIES.has(propName)) {
            throw new Error(`Math method not allowed: ${propName}`)
          }
          const fn = Math[propName as keyof typeof Math]
          if (typeof fn !== 'function') {
            throw new Error(`Math.${propName} is not a function`)
          }
          return (fn as (...args: number[]) => number)(...(args as number[]))
        }
      }

      throw new Error('Function call not allowed')
    }

    case 'MemberExpression': {
      const member = node as jsep.MemberExpression
      // Only allow Math.property (dot notation, not computed)
      if (member.computed) {
        throw new Error('Computed property access not allowed')
      }
      if (
        member.object.type === 'Identifier' &&
        (member.object as jsep.Identifier).name === 'Math' &&
        member.property.type === 'Identifier'
      ) {
        const propName = (member.property as jsep.Identifier).name
        if (BLOCKED_PROPERTIES.has(propName)) {
          throw new Error(`Property access not allowed: Math.${propName}`)
        }
        if (!ALLOWED_MATH_PROPERTIES.has(propName)) {
          throw new Error(`Math property not allowed: ${propName}`)
        }
        return Math[propName as keyof typeof Math]
      }
      throw new Error('Property access not allowed')
    }

    case 'NewExpression': {
      const newExpr = node as jsep.CallExpression // NewExpression has same shape
      // Only allow: new Date(...)
      if (
        newExpr.callee.type === 'Identifier' &&
        (newExpr.callee as jsep.Identifier).name === 'Date'
      ) {
        const args = newExpr.arguments.map(a => evaluateNode(a, bindings))
        return new Date(...(args as [string | number]))
      }
      throw new Error('Only new Date() is allowed')
    }

    default:
      throw new Error(`Expression type not allowed: ${node.type}`)
  }
}

export interface ExpressionContext {
  begin: string
  end: string
  durationMs: number
  innerWidth: number
  devicePixelRatio: number
  variables: Record<string, VariableValue>
}

/**
 * Evaluates an expression using an allowlist-based AST evaluator.
 *
 * Available bindings:
 * - `$begin`, `$end`: ISO 8601 timestamp strings
 * - `$duration_ms`: time range duration in milliseconds
 * - `$innerWidth`: viewport width in CSS pixels
 * - `$devicePixelRatio`: device pixel ratio (e.g., 2 for retina)
 * - `snap_interval(ms)`: snaps a ms duration to a human-friendly SQL interval
 * - `$<name>` for each upstream variable
 *
 * Allowed operations: arithmetic (+, -, *, /, %), Math.*, new Date(), snap_interval().
 * All other operations are rejected (no window, document, fetch, eval, etc.).
 */
export function evaluateVariableExpression(
  expression: string,
  context: ExpressionContext
): string {
  const { begin, end, durationMs, innerWidth, devicePixelRatio, variables } = context

  const bindings: Record<string, unknown> = {
    $begin: begin,
    $end: end,
    $duration_ms: durationMs,
    $innerWidth: innerWidth,
    $devicePixelRatio: devicePixelRatio,
    snap_interval: snapInterval,
    Math: Math,
  }

  for (const [name, value] of Object.entries(variables)) {
    bindings[`$${name}`] = typeof value === 'string' ? value : getVariableString(value)
  }

  const ast = jsep(expression)
  const result = evaluateNode(ast, bindings)
  return String(result)
}
