import {render, screen} from '@testing-library/react'
import React from 'react'

import {BuilderView} from './BuilderView'

const originalError = console.error
beforeAll(() => {
  console.error = (...args) => {
    if (/Warning.*not wrapped in act/.test(args[0])) {
      return
    }
    if (/Each child in a list should have a unique "key" prop/.test(args[0])) {
      return
    }
    originalError.call(console, ...args)
  }
})

afterAll(() => {
  console.error = originalError
})

const setup = async (optionOverrides?: object) => {
  const props = {
    query: '',
    datasource: {
      getTables: () => {},
      getColumns: () => {}
    },
    onChange: () => {},
    fromRawSql: true,
  }

  await render(<BuilderView {...props} />)
}

describe('BuilderView', () => {
  it('should render without throwing an error', () => {
    expect(() => setup()).not.toThrow()
  })
  it('should show the default form elements', () => {
    setup()
    expect(screen.getByText(/from/i)).toBeInTheDocument()
    // expect(screen.getByText(/select/i)).toBeInTheDocument()
    expect(screen.getByText(/where/i)).toBeInTheDocument()
    expect(screen.getByText(/group by/i)).toBeInTheDocument()
    expect(screen.getByText(/order by/i)).toBeInTheDocument()
    expect(screen.getByText(/limit/i)).toBeInTheDocument()
  })
})
