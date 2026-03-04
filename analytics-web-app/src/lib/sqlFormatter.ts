import { format } from 'sql-formatter'

export function formatSQL(q: string) {
  return format(q).replace(/(\$ \{ .*? \})|(\$ __)|(\$ \w+)/g, (m: string) => {
    return m.replace(/\s/g, '')
  })
}
