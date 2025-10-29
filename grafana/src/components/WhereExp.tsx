import React from 'react'
import {Input} from '@grafana/ui'

export const WhereExp = ({index, setWhere, whereValues}: any) => (
  <Input
    type="text"
    spellCheck={false}
    onBlur={() => {}}
    onKeyDown={(e: any) => {
      if (e.key === 'Enter') {
        setWhere({value: whereValues[index].value, index: index})
      }
    }}
    onChange={(e: any) => {
      setWhere({value: e.currentTarget.value, index: index})
    }}
    value={whereValues[index].value}
    width={20}
    placeholder="value = value"
  />
)
