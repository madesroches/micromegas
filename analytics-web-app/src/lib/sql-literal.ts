/**
 * Doubles a single quote inside a SQL string literal -- the same escaping rule
 * `substitute_macros` already follows server-side (`analytics-web-srv/src/stream_query.rs`).
 * Apply this at the one place a value is embedded into a SQL statement.
 */
export function escapeSqlLiteral(value: string): string {
  return value.replace(/'/g, "''")
}
