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
  timeFilter?: boolean
  autoLimit?: boolean
}

export const DEFAULT_QUERY: Partial<SQLQuery> = {
	timeFilter: true,
	autoLimit: true
}

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

  // OAuth 2.0 Client Credentials (stored unencrypted)
  oauthIssuer?: string           // e.g., "https://accounts.google.com"
  oauthClientId?: string         // e.g., "grafana@project.iam.gserviceaccount.com"
  oauthAudience?: string         // Optional, for Auth0/Azure AD

  // Privacy Settings
  enableUserAttribution?: boolean // Enable sending user info to FlightSQL server
}

// Default values for privacy settings
export const DEFAULT_ENABLE_USER_ATTRIBUTION = true

/**
 * Get the effective value for enableUserAttribution with explicit default handling.
 * Returns true if undefined (default), otherwise returns the explicit value.
 */
export function getEnableUserAttribution(jsonData: FlightSQLDataSourceOptions): boolean {
  return jsonData.enableUserAttribution ?? DEFAULT_ENABLE_USER_ATTRIBUTION
}

export interface SecureJsonData {
  password?: string
  token?: string
  oauthClientSecret?: string    // OAuth client secret (encrypted by Grafana)
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
  {key: 3, label: 'oauth2-client-credentials', value: 'oauth2'},
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
