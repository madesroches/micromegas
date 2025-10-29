import {ScopedVars} from '@grafana/data'
import * as runtime from '@grafana/runtime'

import {mockDatasource, mockQuery} from './mock-datasource'

describe('DataSource', () => {
  describe('applyTemplateVariables', () => {
    const scopedVars: Record<string, any> = {
      $simple: 'orgID',
      $multiple: ['host', 'orgID'],
    }
    const replace = jest.fn((target?: string, scopedVars?: ScopedVars, format?: string | Function) => {
      let res = target ?? ''
      if (scopedVars && typeof format === 'function') {
        Object.keys(scopedVars).forEach((v) => (res = res.replace(v, format(scopedVars[v]))))
      }
      return res
    })
    beforeEach(() => {
      jest.spyOn(runtime, 'getTemplateSrv').mockImplementation(() => ({
        getVariables: jest.fn(),
        replace: replace,
        containsTemplate: jest.fn(),
        updateTimeRange: jest.fn(),
      }))
    })

    it('should replace a simple var', () => {
      const res = mockDatasource.applyTemplateVariables({...mockQuery, queryText: 'select * from $simple'}, scopedVars)
      expect(res.queryText).toEqual('select * from orgID')
    })

    it('should replace a multiple var', () => {
      const res = mockDatasource.applyTemplateVariables(
        {...mockQuery, queryText: 'select * from org where var in ($multiple)'},
        scopedVars
      )
      expect(res.queryText).toEqual(`select * from org where var in ('host','orgID')`)
    })
  })
})
