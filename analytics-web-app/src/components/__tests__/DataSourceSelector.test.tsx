import { render, waitFor } from '@testing-library/react'
import { DataSourceSelector } from '../DataSourceSelector'

jest.mock('lucide-react', () => ({
  Database: () => null,
  AlertCircle: () => null,
}))

const getDataSourceList = jest.fn()
jest.mock('@/lib/data-sources-api', () => ({
  getDataSourceList: (...args: unknown[]) => getDataSourceList(...args),
}))

describe('DataSourceSelector value sync', () => {
  beforeEach(() => {
    getDataSourceList.mockReset()
    getDataSourceList.mockResolvedValue([
      { name: 'prod', is_default: true },
      { name: 'staging', is_default: false },
    ])
  })

  it('does not rewrite a variable reference that is missing from the known variables', async () => {
    const onChange = jest.fn()
    render(
      <DataSourceSelector
        value="$source"
        onChange={onChange}
        datasourceVariables={[]}
        showNotebookOption
      />,
    )
    // Wait for the async source list to load and the sync effect to run.
    await waitFor(() => expect(getDataSourceList).toHaveBeenCalled())
    await new Promise((r) => setTimeout(r, 0))
    expect(onChange).not.toHaveBeenCalled()
  })

  it('keeps a variable reference even when it is a known variable', async () => {
    const onChange = jest.fn()
    render(
      <DataSourceSelector
        value="$source"
        onChange={onChange}
        datasourceVariables={['source']}
        showNotebookOption
      />,
    )
    await waitFor(() => expect(getDataSourceList).toHaveBeenCalled())
    await new Promise((r) => setTimeout(r, 0))
    expect(onChange).not.toHaveBeenCalled()
  })

  it('surfaces a literal data source that no longer exists instead of rewriting it', async () => {
    const onChange = jest.fn()
    const { findByText } = render(
      <DataSourceSelector
        value="deleted-source"
        onChange={onChange}
        datasourceVariables={[]}
        showNotebookOption
      />,
    )
    // The unknown value is shown as its own option, marked unavailable...
    expect(await findByText('deleted-source (unavailable)')).toBeInTheDocument()
    // ...and the config is never silently rewritten.
    await new Promise((r) => setTimeout(r, 0))
    expect(onChange).not.toHaveBeenCalled()
  })

  it('surfaces an out-of-scope variable reference as a selectable option', async () => {
    const onChange = jest.fn()
    const { findByText } = render(
      <DataSourceSelector
        value="$source"
        onChange={onChange}
        datasourceVariables={[]}
        showNotebookOption
      />,
    )
    expect(await findByText('$source')).toBeInTheDocument()
    await new Promise((r) => setTimeout(r, 0))
    expect(onChange).not.toHaveBeenCalled()
  })
})
