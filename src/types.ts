import {DataQuery, DataSourceJsonData} from '@grafana/data'
import {formatSQL} from './components/sqlFormatter'

export interface SQLQuery extends DataQuery {
  queryText?: string
  format?: string
  rawEditor?: boolean
  table?: string
  columns?: string[]
  wheres?: string[]
  orderBy?: string
  groupBy?: string
  limit?: string
}

export const DEFAULT_QUERY: Partial<SQLQuery> = {}

/**
 * These are options configured for each DataSource instance
 */
export interface FlightSQLDataSourceOptions extends DataSourceJsonData {
  host?: string
  token?: string
  secure?: boolean
  username?: string
  password?: string
  selectedAuthType?: string
  metadata?: any
}

export interface SecureJsonData {
  password?: string
  token?: string
}

export type TablesResponse = {
  tables: string[]
}

export type ColumnsResponse = {
  columns: string[]
}

export const authTypeOptions = [
  {key: 0, label: 'none', value: 'none'},
  {key: 1, label: 'username/password', value: 'username/password'},
  {key: 2, label: 'token', value: 'token'},
]

export const sqlLanguageDefinition = {
  id: 'sql',
  formatter: formatSQL,
}

export enum QueryFormat {
  Timeseries = 'time_series',
  Table = 'table',
  Logs = 'logs',
}

export const QUERY_FORMAT_OPTIONS = [
  {label: 'Time series', value: QueryFormat.Timeseries},
  {label: 'Table', value: QueryFormat.Table},
  {label: 'Logs', value: QueryFormat.Logs},
]
