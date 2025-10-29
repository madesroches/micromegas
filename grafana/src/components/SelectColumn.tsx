import React from 'react'
import {Select} from '@grafana/ui'

export const SelectColumn = ({columns, index, setColumn, columnValues, formatCreateLabel}: any) => (
  <Select
    options={columns}
    onOpenMenu={() => {
      columns && columns.forEach((c: any) => (c.index = index))
    }}
    onChange={setColumn}
    key={index}
    value={columnValues[index].value}
    allowCustomValue={true}
    formatCreateLabel={formatCreateLabel}
    width={20}
    placeholder="column"
  />
)
