import { useState, type Dispatch, type SetStateAction } from 'react'
import { useDefaultDataSource } from './useDefaultDataSource'

interface DataSourceState {
  dataSource: string
  setDataSource: Dispatch<SetStateAction<string>>
  error: string | null
}

/**
 * Hook that provides a mutable data source initialized from the default.
 * Use this when a page needs a DataSourceSelector that the user can change.
 * For read-only access to the default, use useDefaultDataSource instead.
 */
export function useDataSourceState(): DataSourceState {
  const { name: defaultDataSource, error } = useDefaultDataSource()
  const [dataSource, setDataSource] = useState('')

  // useDefaultDataSource starts at '' until it resolves, and dataSource
  // starts at '' too, so a dropped mount-time run has nothing to do yet.
  const [prevDefaultDataSource, setPrevDefaultDataSource] = useState(defaultDataSource)
  const [prevDataSource, setPrevDataSource] = useState(dataSource)
  if (defaultDataSource !== prevDefaultDataSource || dataSource !== prevDataSource) {
    setPrevDefaultDataSource(defaultDataSource)
    setPrevDataSource(dataSource)
    if (!dataSource && defaultDataSource) {
      setDataSource(defaultDataSource)
    }
  }

  return { dataSource, setDataSource, error }
}
