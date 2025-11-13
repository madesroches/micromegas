import {DataQuery, DataSourceJsonData} from '@grafana/data'
import {formatSQL} from './components/sqlFormatter'

export interface SQLQuery extends DataQuery {
  query?: string
  queryText?: string  // Legacy v1 field - can be removed when v1 support is no longer needed
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
  version?: number  // Query schema version (undefined/missing = v1, 2 = current)
}

export const DEFAULT_QUERY: Partial<SQLQuery> = {
	query: '',
	format: 'table',
	timeFilter: true,
	autoLimit: true,
	version: 2
}

// Default values for query options
export const DEFAULT_TIME_FILTER = true
export const DEFAULT_AUTO_LIMIT = true

/**
 * Get the effective value for timeFilter with explicit default handling.
 * Returns true if undefined (default), otherwise returns the explicit value.
 */
export function getTimeFilter(query: SQLQuery): boolean {
  return query.timeFilter ?? DEFAULT_TIME_FILTER
}

/**
 * Get the effective value for autoLimit with explicit default handling.
 * Returns true if undefined (default), otherwise returns the explicit value.
 */
export function getAutoLimit(query: SQLQuery): boolean {
  return query.autoLimit ?? DEFAULT_AUTO_LIMIT
}

export enum QueryContext {
  Panel = 'panel',
  Variable = 'variable',
}

/**
 * Migrates queries from older schema versions to the current version (v2).
 * This ensures backwards compatibility with existing dashboards.
 *
 * @param query - The query to migrate (can be string, null/undefined/empty, or SQLQuery object)
 * @param context - The context where the query is used (panel or variable)
 * @returns A new migrated query object (does not mutate input)
 */
export function migrateQuery(query: SQLQuery | string | null | undefined, context: QueryContext): SQLQuery {
  // Handle legacy string format
  if (typeof query === 'string') {
    return {
      ...DEFAULT_QUERY,
      query: query,
      queryText: undefined,
      autoLimit: context === QueryContext.Variable ? false : true,
    } as SQLQuery;
  }

  // Defensive: return default query if input is invalid
  if (!query || Object.keys(query).length === 0) {
    return {
      ...DEFAULT_QUERY,
      refId: query?.refId || 'A',
      autoLimit: context === QueryContext.Variable ? false : true,
    } as SQLQuery;
  }

  // Detect version (undefined/missing = v1)
  const version = query.version ?? 1;

  // Forward compatibility: treat unknown versions as v2
  if (version >= 2) {
    // Already migrated, but ensure autoLimit is correct for variable context
    if (context === QueryContext.Variable && query.autoLimit !== false) {
      return {
        ...query,
        autoLimit: false,
      };
    }
    return query;
  }

  // V1 migration logic
  let migratedQuery: SQLQuery = { ...query };

  // Migrate query text field
  if (migratedQuery.query) {
    // query field exists and takes precedence
    migratedQuery.queryText = undefined;
  } else if (migratedQuery.queryText) {
    // Copy queryText to query field
    migratedQuery.query = migratedQuery.queryText;
    migratedQuery.queryText = undefined;
  } else {
    // Neither field exists, set to empty string
    migratedQuery.query = '';
    migratedQuery.queryText = undefined;
  }

  // Migrate format field (v1 default: 'table')
  if (migratedQuery.format === undefined) {
    migratedQuery.format = 'table';
  }

  // Migrate timeFilter field (v1 default: true)
  if (migratedQuery.timeFilter === undefined) {
    migratedQuery.timeFilter = true;
  }

  // Migrate autoLimit field (context-dependent)
  if (context === QueryContext.Variable) {
    // Variables: always force to false
    migratedQuery.autoLimit = false;
  } else {
    // Panels: default to true if undefined
    if (migratedQuery.autoLimit === undefined) {
      migratedQuery.autoLimit = true;
    }
  }

  // Mark as v2
  migratedQuery.version = 2;

  return migratedQuery;
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
