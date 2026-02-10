import { useState, useEffect, type Dispatch, type SetStateAction } from 'react'
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

  useEffect(() => {
    if (!dataSource && defaultDataSource) {
      setDataSource(defaultDataSource)
    }
  }, [defaultDataSource, dataSource])

  return { dataSource, setDataSource, error }
}
