import {buildQueryString, checkCasing, formatColumns, formatWheres} from './utils'

describe('FlightSQL utils', () => {
  describe('buildQueryString', () => {
    it('should build basic query', () => {
      expect(buildQueryString('price', 'coindesk', "code = 'GBP'", 'time asc', 'time asc', '10')).toBe(
        "SELECT price FROM coindesk WHERE code = 'GBP' GROUP BY time asc ORDER BY time asc LIMIT 10"
      )
    })
    it('should ignore limit if not provided', () => {
      expect(buildQueryString('price', 'coindesk', "code = 'GBP'", 'time asc', 'time asc', '')).toBe(
        "SELECT price FROM coindesk WHERE code = 'GBP' GROUP BY time asc ORDER BY time asc"
      )
    })
    it('should ignore group by if not provided', () => {
      expect(buildQueryString('price', 'coindesk', "code = 'GBP'", 'time asc', '', '')).toBe(
        "SELECT price FROM coindesk WHERE code = 'GBP' ORDER BY time asc"
      )
    })
    it('should ignore order by if not provided', () => {
      expect(buildQueryString('price', 'coindesk', "code = 'GBP'", '', '', '')).toBe(
        "SELECT price FROM coindesk WHERE code = 'GBP'"
      )
    })

    it('should ignore where statements if not provided', () => {
      expect(buildQueryString('price', 'coindesk', '', '', '', '')).toBe('SELECT price FROM coindesk')
    })

    it('should join mutiple columns together', () => {
      expect(buildQueryString('price,time', 'coindesk', '', '', '', '')).toBe('SELECT price,time FROM coindesk')
    })
    it('should return quotes where text is camel cased', () => {
      let str = 'camelCase'
      str = checkCasing(str)
      expect(str).toBe('"camelCase"')
    })
    it('should not alter string if not camel cased', () => {
      let str = 'notcamel'
      str = checkCasing(str)
      expect(str).toBe('notcamel')
    })
    it('should not format columns correctly', () => {
      const columnInput = [{value: 'camelValue'}, {value: 'noncamelval'}]
      const output = formatColumns(columnInput)
      expect(output).toBe('"camelValue",noncamelval')
    })
    it('should not have a trailing comma if single column value', () => {
      const columnInput = [{value: 'foo'}]
      const output = formatColumns(columnInput)
      expect(output).toBe('foo')
    })
    it('should not format where correctly', () => {
      const whereInput = [{value: "code = 'GBP'"}, {value: 'price >= 10'}]
      const output = formatWheres(whereInput)
      expect(output).toBe("code = 'GBP' and price >= 10")
    })
    it('should not be a trailing and if only single where statement', () => {
      const whereInput = [{value: "code = 'GBP'"}]
      const output = formatWheres(whereInput)
      expect(output).toBe("code = 'GBP'")
    })
  })
})
