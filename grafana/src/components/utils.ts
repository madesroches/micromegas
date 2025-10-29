import {LanguageCompletionProvider, getStandardSQLCompletionProvider} from '@grafana/experimental'

export const buildQueryString = (
  columns: string,
  table: string,
  whereExp: string,
  orderBy: string | undefined,
  groupBy: string | undefined,
  limit: string | undefined
): string => {
  let queryStr = `SELECT ${columns} FROM ${table}`

  if (whereExp) {
    queryStr = queryStr + ` WHERE ${whereExp}`
  }

  if (groupBy) {
    queryStr = queryStr + ` GROUP BY ${groupBy}`
  }

  if (orderBy) {
    queryStr = queryStr + ` ORDER BY ${orderBy}`
  }

  if (limit) {
    queryStr = queryStr + ` LIMIT ${limit}`
  }

  return queryStr
}

export const checkCasing = (str: string) => {
  const camelCase = /[A-Z]/.test(str)
  if (camelCase) {
    str = `"${str}"`
  }

  return str
}

export const prefixDB = (table: string, dbSchema: string) => {
  let str = table
  if (dbSchema) {
    str = `${dbSchema}.${table}`
  }
  return str
}

export const handleColumnChange = (column: any, setColumnValues: any, columnValues: any) => {
  const lastIndex = columnValues.length - 1
  if (column.index === undefined) {
    column.index = lastIndex
  }
  const newColumnValues = [
    ...columnValues.slice(0, column.index),
    {
      value: column?.value
    },
  ]
  setColumnValues(newColumnValues)
}

export const addColumns = (setColumnValues: any, columnValues: any) => {
  setColumnValues([...columnValues, {value: ''}])
}

export const removeColumns = (i: any, setColumnValues: any, columnValues: any) => {
  let newColumnValues = [...columnValues]
  newColumnValues.splice(i, 1)
  setColumnValues(newColumnValues)
}

export const handleWhereChange = (where: any, setWhereValues: any, whereValues: any) => {
  const newWhereValues = [
    ...whereValues.slice(0, where.index),
    {
      value: where?.value
    },
  ]
  setWhereValues(newWhereValues)
}

export const addWheres = (setWhereValues: any, whereValues: any) => {
  setWhereValues([...whereValues, {value: ''}])
}

export const removeWheres = (i: any, setWhereValues: any, whereValues: any) => {
  let newWhereValues = [...whereValues]
  newWhereValues.splice(i, 1)
  setWhereValues(newWhereValues)
}

export const formatColumns = (columnArr: any) => {
  return columnArr
    .map((c: any) => checkCasing(c.value))
    .join(',')
    .replace(/,\s*$/, '')
}

export const formatWheres = (whereValues: any) => {
  return whereValues
    .map((w: any) => w.value)
    .filter(Boolean)
    .join(' and ')
}

export const formatCreateLabel = (v: string) => v

export const onHostChange = (event: any, options: any, onOptionsChange: any) => {
  const jsonData = {
    ...options.jsonData,
    host: event.target.value,
  }
  onOptionsChange({...options, jsonData})
}

export const onSecureChange = (options: any, onOptionsChange: any) => {
  const jsonData = {
    ...options.jsonData,
    secure: !options.jsonData.secure,
  }
  onOptionsChange({...options, jsonData})
}

export const onUsernameChange = (event: any, options: any, onOptionsChange: any) => {
  const jsonData = {
    ...options.jsonData,
    username: event.target.value,
  }
  onOptionsChange({...options, jsonData})
}

export const onPasswordChange = (event: any, options: any, onOptionsChange: any) => {
  const secureJsonData = {
    ...options.secureJsonData,
    password: event?.target?.value || '',
  }
  onOptionsChange({...options, secureJsonData})
}

export const onAuthTypeChange = (selectedAuthType: any, options: any, onOptionsChange: any) => {
  const notTokenType =  selectedAuthType?.label !== "token"
  const notPassType = selectedAuthType?.label !== "username/password"

  onOptionsChange({
    ...options,
    jsonData: {
      ...options.jsonData,
      selectedAuthType: selectedAuthType?.label,
      username: notPassType && '',
    },
    secureJsonFields: {
      ...options.secureJsonFields,
      token: notTokenType && false,
      password: notPassType && false,
    },
    secureJsonData: {
      ...options.secureJsonData,
      token: notTokenType && '',
      password: notPassType && '',
    },
  })
}

export const getSqlCompletionProvider: (args: any) => LanguageCompletionProvider =
  ({getTables, getColumns, sqlInfo, macros}) =>
  (monaco, language) => {
    return {
      ...(language &&
        getStandardSQLCompletionProvider(monaco, {
          ...language,
          builtinFunctions: sqlInfo?.builtinFunctions,
          keywords: sqlInfo?.keywords,
        })),
      triggerCharacters: ['.', ' ', '$', ',', '(', "'"],
      tables: {
        resolve: () => {
          return getTables()
        },
      },
      columns: {
        resolve: (t: string) => getColumns(t),
      },
      supportedMacros: () => macros,
    }
  }

export const addMetaData = (setMetaData: any, metaDataArr: any) => {
  setMetaData([...metaDataArr, {value: ''}])
}

export const removeMetaData = (i: any, setMetaData: any, metaDataArr: any) => {
  let newMetaValues = [...metaDataArr]
  newMetaValues.splice(i, 1)
  setMetaData(newMetaValues)
}

export const onKeyChange = (event: any, metaDataArr: any, index: any, setMetaData: any) => {
  let newMetaValues = [...metaDataArr]
  newMetaValues[index]['key'] = event.target.value
  setMetaData(newMetaValues)
}

export const onValueChange = (event: any, metaDataArr: any, index: any, setMetaData: any) => {
  let newMetaValues = [...metaDataArr]
  newMetaValues[index]['value'] = event.target.value
  setMetaData(newMetaValues)
}

export const onTokenChange = (event: any, options: any, onOptionsChange: any) => {
  const secureJsonData = {
    ...options.secureJsonData,
    token: event?.target?.value || '',
  }
  onOptionsChange({...options, secureJsonData})
}

export const onResetToken = (options: any, onOptionsChange: any) => {
  onOptionsChange({
    ...options,
    secureJsonFields: {
      ...options.secureJsonFields,
      token: false,
    },
    secureJsonData: {
      ...options.secureJsonData,
      token: '',
    },
  })
}

export const onResetPassword = (options: any, onOptionsChange: any) => {
  onOptionsChange({
    ...options,
    secureJsonFields: {
      ...options.secureJsonFields,
      password: false,
    },
    secureJsonData: {
      ...options.secureJsonData,
      password: '',
    },
  })
}

export const removeQuotes = (str: string) => {
  return str?.replace(/['"]+/g, '')
}
