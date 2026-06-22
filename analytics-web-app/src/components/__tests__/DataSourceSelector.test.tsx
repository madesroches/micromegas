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

  it('rewrites a literal data source that no longer exists to the first option', async () => {
    const onChange = jest.fn()
    render(
      <DataSourceSelector
        value="deleted-source"
        onChange={onChange}
        datasourceVariables={[]}
        showNotebookOption
      />,
    )
    await waitFor(() => expect(onChange).toHaveBeenCalledWith('notebook'))
  })
})
