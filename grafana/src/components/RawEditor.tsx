import React, {useCallback, useEffect, useRef} from 'react'

import {LanguageDefinition, SQLEditor} from '@grafana/experimental'

type Props = {
  query: any
  onChange: (value: any, processQuery: boolean) => void
  children?: (props: {formatQuery: () => void}) => React.ReactNode
  width?: number
  height?: number
  editorLanguageDefinition: LanguageDefinition
}

export function RawEditor({children, onChange, query, width, height, editorLanguageDefinition}: Props) {
  const queryRef = useRef<any>(query)
  useEffect(() => {
    queryRef.current = query
  }, [query])

  const onRawQueryChange = useCallback(
    (rawSql: string, processQuery: boolean) => {
      const newQuery = {
        ...queryRef.current,
        rawQuery: true,
        queryText: rawSql,
      }
      onChange(newQuery, processQuery)
    },
    [onChange]
  )
  return (
    <div>
      <SQLEditor
        width={width}
        height={height}
        query={query.queryText!}
        onChange={onRawQueryChange}
        language={editorLanguageDefinition}
        key="1"
      >
        {children}
      </SQLEditor>
    </div>
  )
}
