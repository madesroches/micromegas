import { format } from 'sql-formatter'

export function formatSQL(q: string): string {
  try {
    return format(q, { paramTypes: { named: ['$'] } })
  } catch {
    return q
  }
}
