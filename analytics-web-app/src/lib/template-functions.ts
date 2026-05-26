import { formatValueWithUnit } from './format-value'

export type TemplateFunction = (args: unknown[]) => string | undefined

export const TEMPLATE_FUNCTIONS: Record<string, TemplateFunction> = {
  format_value: (args) => {
    if (args.length !== 2) return undefined
    const [rawValue, rawUnit] = args
    // Reject empty strings explicitly — Number("") is 0 (finite), so without
    // this guard an empty-but-defined variable would silently render "0 B"
    // instead of surfacing as an unresolved-arg warning.
    if (rawValue === '') return undefined
    const value = Number(rawValue)
    if (!Number.isFinite(value)) return undefined
    return formatValueWithUnit(value, String(rawUnit ?? ''))
  },
}
