import {render, screen} from '@testing-library/react'
import React from 'react'
import {DataSourcePluginOptionsEditorProps} from '@grafana/data'
import {FlightSQLDataSourceOptions} from '../types'

import {ConfigEditor} from './ConfigEditor'

jest.mock('lodash', () => {
  const uniqueId = (prefix: string) => `${prefix}42`

  const orig = jest.requireActual('lodash')

  return {
    ...orig,
    uniqueId,
  }
})

const setup = async (optionOverrides?: object) => {
  const props: DataSourcePluginOptionsEditorProps<FlightSQLDataSourceOptions> = {
    options: {
      id: 1,
      uid: '1',
      orgId: 1,
      name: 'Timestream',
      typeLogoUrl: '',
      type: '',
      access: '',
      url: '',
      user: '',
      basicAuth: false,
      basicAuthUser: '',
      database: '',
      isDefault: false,
      jsonData: {
        host: '',
        secure: true,
        username: '',
        selectedAuthType: '',
        metadata: [],
      },
      secureJsonFields: {
        token: false,
        password: false,
      },
      readOnly: false,
      withCredentials: false,
      typeName: '',
    },
    onOptionsChange: jest.fn(),
  }

  await render(<ConfigEditor {...props} />)
}

describe('ConfigEditor', () => {
  it('should render without throwing an error', () => {
    expect(() => setup()).not.toThrow()
  })

  it('should show the default form elements', () => {
    setup({
      jsonData: {
        selectedAuthType: 'token',
      },
    })
    expect(screen.getByText(/host:port/i)).toBeInTheDocument()
    expect(screen.getByText(/auth type/i)).toBeInTheDocument()
    expect(screen.getByText(/require tls \/ ssl/i)).toBeInTheDocument()
    expect(screen.getByText(/require tls \/ ssl/i)).toBeInTheDocument()
  })
})
